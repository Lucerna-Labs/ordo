//! `WebhookService` â€” register / update / delete subscriptions +
//! expose the active list to the dispatcher.

use std::sync::Arc;

use chrono::Utc;
use ordo_protocol::WebhookSubscription;
use parking_lot::Mutex;
use uuid::Uuid;

use crate::store::WebhookStore;
use crate::types::{NewSubscription, SubscriptionUpdate, WebhookError, WebhookResult};

pub const DEFAULT_WORKSPACE_ID: &str = "local";

#[derive(Clone)]
pub struct WebhookService {
    store: Arc<Mutex<WebhookStore>>,
}

impl WebhookService {
    pub fn new(store: WebhookStore) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
        }
    }

    /// Register a new subscription. When the caller doesn't supply a
    /// `secret`, one is generated using the process RNG and returned
    /// in the response â€” the caller must capture it then, subsequent
    /// reads REDACT the secret.
    pub fn register(&self, new_sub: NewSubscription) -> WebhookResult<WebhookSubscription> {
        if new_sub.target_url.trim().is_empty() {
            return Err(WebhookError::InvalidArgument(
                "target_url is required".into(),
            ));
        }
        if !new_sub.target_url.starts_with("http://") && !new_sub.target_url.starts_with("https://")
        {
            return Err(WebhookError::InvalidArgument(
                "target_url must be http:// or https://".into(),
            ));
        }
        let secret = match new_sub.secret {
            Some(s) if !s.is_empty() => s,
            _ => generate_secret(),
        };
        let workspace_id = new_sub
            .workspace_id
            .unwrap_or_else(|| DEFAULT_WORKSPACE_ID.to_string());
        let topics = dedupe_topics(new_sub.topics);
        let id = Uuid::new_v4();
        let now = Utc::now();
        self.store.lock().insert(
            id,
            &workspace_id,
            &new_sub.target_url,
            &secret,
            &topics,
            &new_sub.description,
            now,
        )
    }

    /// Redact-safe read. Returns the subscription with the `secret`
    /// field cleared so API callers listing subscriptions never
    /// receive it. Dispatcher code uses `get_with_secret` instead.
    pub fn get(&self, id: Uuid) -> WebhookResult<Option<WebhookSubscription>> {
        Ok(self.store.lock().get(id)?.map(redact))
    }

    pub fn list(&self, workspace_id: Option<&str>) -> WebhookResult<Vec<WebhookSubscription>> {
        Ok(self
            .store
            .lock()
            .list(workspace_id)?
            .into_iter()
            .map(redact)
            .collect())
    }

    /// Active subscriptions WITH their secrets. Only the dispatcher
    /// calls this â€” never exposed over HTTP.
    pub fn active_with_secrets(&self) -> WebhookResult<Vec<WebhookSubscription>> {
        self.store.lock().active()
    }

    pub fn update(
        &self,
        id: Uuid,
        patch: SubscriptionUpdate,
    ) -> WebhookResult<WebhookSubscription> {
        // Acquire the current state, apply the patch, persist via
        // set_active/delete/re-insert combos. For the MVP we only
        // support toggling active + editing topics/description/url
        // through a coarse replace: delete + re-insert keeps the SQL
        // trivial and the audit trail clear.
        let mut store = self.store.lock();
        let mut current = store.get(id)?.ok_or(WebhookError::NotFound(id))?;
        if let Some(url) = patch.target_url {
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err(WebhookError::InvalidArgument(
                    "target_url must be http:// or https://".into(),
                ));
            }
            current.target_url = url;
        }
        if let Some(topics) = patch.topics {
            current.topics = dedupe_topics(topics);
        }
        if let Some(description) = patch.description {
            current.description = description;
        }
        if let Some(active) = patch.active {
            store.set_active(id, active)?;
            current.active = active;
        }
        // Write edits that aren't bool-toggle by replacing the row
        // in place via delete + insert. Atomicity isn't a concern â€”
        // the process holds the store mutex for the duration.
        store.delete(id)?;
        let replaced = store.insert(
            id,
            &current.workspace_id,
            &current.target_url,
            &current.secret,
            &current.topics,
            &current.description,
            current.created_at,
        )?;
        if !current.active {
            store.set_active(id, false)?;
        }
        let mut final_state = replaced;
        final_state.active = current.active;
        Ok(redact(final_state))
    }

    pub fn delete(&self, id: Uuid) -> WebhookResult<bool> {
        self.store.lock().delete(id)
    }

    pub(crate) fn record_delivery(&self, id: Uuid, status: u16) -> WebhookResult<()> {
        self.store.lock().record_delivery(id, Utc::now(), status)
    }
}

fn dedupe_topics(mut topics: Vec<String>) -> Vec<String> {
    topics.retain(|t| !t.trim().is_empty());
    topics.sort();
    topics.dedup();
    topics
}

fn redact(mut sub: WebhookSubscription) -> WebhookSubscription {
    sub.secret = String::from("<redacted>");
    sub
}

fn generate_secret() -> String {
    // 32 random bytes encoded as hex. Uses the OS randomness via
    // std::process info â€” we avoid pulling `rand` for one call. The
    // approach: concatenate a timestamp + a uuid + another uuid and
    // SHA-256 the result, taking the first 32 bytes hex. The uuids
    // use v4 which is OS-RNG-backed on every platform we ship to.
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(Uuid::new_v4().as_bytes());
    hasher.update(Uuid::new_v4().as_bytes());
    hasher.update(Utc::now().timestamp_nanos_opt().unwrap_or(0).to_le_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc() -> WebhookService {
        let store = WebhookStore::in_memory().expect("store");
        WebhookService::new(store)
    }

    #[test]
    fn register_generates_a_secret_when_absent() {
        let svc = svc();
        let sub = svc
            .register(NewSubscription {
                target_url: "https://example.com/hook".into(),
                secret: None,
                topics: vec!["ordo.apps.event".into()],
                description: "test".into(),
                workspace_id: None,
            })
            .expect("register");
        assert!(!sub.secret.is_empty());
        assert_ne!(sub.secret, "<redacted>");
        assert_eq!(sub.topics, vec!["ordo.apps.event"]);
        assert!(sub.active);
    }

    #[test]
    fn get_redacts_secret_in_read_responses() {
        let svc = svc();
        let sub = svc
            .register(NewSubscription {
                target_url: "https://example.com".into(),
                secret: Some("plaintext-secret".into()),
                topics: vec![],
                description: String::new(),
                workspace_id: None,
            })
            .expect("register");
        let fetched = svc.get(sub.id).expect("get").expect("present");
        assert_eq!(fetched.secret, "<redacted>");
    }

    #[test]
    fn register_rejects_non_http_urls() {
        let svc = svc();
        let err = svc
            .register(NewSubscription {
                target_url: "file:///etc/passwd".into(),
                secret: None,
                topics: vec![],
                description: String::new(),
                workspace_id: None,
            })
            .expect_err("should fail");
        assert!(matches!(err, WebhookError::InvalidArgument(_)));
    }

    #[test]
    fn active_with_secrets_returns_real_secret_for_dispatcher() {
        let svc = svc();
        let sub = svc
            .register(NewSubscription {
                target_url: "https://example.com".into(),
                secret: Some("keep-me".into()),
                topics: vec![],
                description: String::new(),
                workspace_id: None,
            })
            .expect("register");
        let active = svc.active_with_secrets().expect("active");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, sub.id);
        assert_eq!(active[0].secret, "keep-me");
    }

    #[test]
    fn update_toggles_active_flag() {
        let svc = svc();
        let sub = svc
            .register(NewSubscription {
                target_url: "https://example.com".into(),
                secret: None,
                topics: vec![],
                description: String::new(),
                workspace_id: None,
            })
            .expect("register");
        let updated = svc
            .update(
                sub.id,
                SubscriptionUpdate {
                    active: Some(false),
                    ..Default::default()
                },
            )
            .expect("update");
        assert!(!updated.active);
        let active = svc.active_with_secrets().expect("active");
        assert!(active.is_empty());
    }
}
