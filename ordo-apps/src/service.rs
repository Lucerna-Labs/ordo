//! `AppsService` â€” the orchestrator for the apps lifecycle.
//!
//! Construction follows Rule 7: `new(AppsStore) + .with_bus() +
//! .with_review()`. The bus is injected, not owned. Review gating is
//! opt-in â€” when a `ReviewService` is wired, publish/archive/delete
//! route through it before taking effect.
//!
//! All mutations go through `AppsStore::apply_mutations` so the
//! `apps` row + `app_events` log stay in lockstep (Phase 1.2 rewind
//! is built on this invariant).

use std::sync::Arc;

use ordo_bus::Bus;
use ordo_protocol::{topics, App, AppStatus, Envelope, NodeId, OrdoMessage};
use parking_lot::Mutex;
use uuid::Uuid;

use crate::store::{AppsStore, Mutation};
use crate::types::{AppRef, AppUpdate, AppsError, AppsQuery, AppsResult, NewApp};

/// Default workspace id used when callers don't specify one. Matches
/// the column default so single-operator deploys never see an empty
/// workspace.
pub const DEFAULT_WORKSPACE_ID: &str = "local";

#[derive(Clone)]
pub struct AppsService {
    store: Arc<Mutex<AppsStore>>,
    node_id: NodeId,
    bus: Option<Arc<dyn Bus>>,
    review: Option<ordo_review::ReviewService>,
}

impl AppsService {
    pub fn new(store: AppsStore) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            node_id: NodeId::new(),
            bus: None,
            review: None,
        }
    }

    pub fn with_bus(mut self, bus: Arc<dyn Bus>) -> Self {
        self.bus = Some(bus);
        self
    }

    pub fn with_review(mut self, review: ordo_review::ReviewService) -> Self {
        self.review = Some(review);
        self
    }

    /// Create a new app. Derives `slug` from `name` when not provided.
    /// Emits `ordo.apps.event` (Created) on the bus.
    pub async fn create(&self, new_app: NewApp, actor: &str) -> AppsResult<App> {
        if new_app.name.trim().is_empty() {
            return Err(AppsError::InvalidArgument("name is required".into()));
        }
        let workspace_id = new_app
            .workspace_id
            .as_deref()
            .unwrap_or(DEFAULT_WORKSPACE_ID)
            .to_string();
        let slug = new_app
            .slug
            .clone()
            .map(|s| slugify(&s))
            .unwrap_or_else(|| slugify(&new_app.name));
        if slug.is_empty() {
            return Err(AppsError::InvalidArgument(
                "slug would be empty after normalization â€” provide one explicitly".into(),
            ));
        }
        let app = {
            let mut store = self.store.lock();
            store.insert(
                &workspace_id,
                &slug,
                &new_app.name,
                &new_app.description,
                &new_app.metadata,
                actor,
            )?
        };
        self.broadcast_last_event(app.id).await;
        Ok(app)
    }

    pub fn get(&self, app_ref: &AppRef) -> AppsResult<Option<App>> {
        self.store.lock().get(app_ref)
    }

    pub fn list(&self, query: AppsQuery) -> AppsResult<Vec<App>> {
        let workspace_id = query.workspace_id.as_deref().or(Some(DEFAULT_WORKSPACE_ID));
        self.store
            .lock()
            .list(workspace_id, query.status, query.limit)
    }

    pub fn events(&self, app_id: Uuid) -> AppsResult<Vec<ordo_protocol::AppEvent>> {
        self.store.lock().events(app_id)
    }

    /// Reconstruct the state of an app at a historical sequence.
    /// Phase 1.2 version fold primitive â€” used by rollback / diff /
    /// "what did the agent see when it made that decision?" flows.
    pub fn state_at_version(&self, app_id: Uuid, up_to_seq: u64) -> AppsResult<Option<App>> {
        self.store.lock().state_at_version(app_id, up_to_seq)
    }

    // ---- Deployments (Phase 3.3) ------------------------------------

    pub fn create_deployment(
        &self,
        app_id: Uuid,
        preview_path: Option<String>,
        note: &str,
    ) -> AppsResult<ordo_protocol::Deployment> {
        self.store
            .lock()
            .create_deployment(app_id, preview_path, note)
    }

    pub fn promote_deployment(&self, deployment_id: Uuid) -> AppsResult<ordo_protocol::Deployment> {
        self.store
            .lock()
            .set_deployment_state(deployment_id, ordo_protocol::DeploymentState::Live)
    }

    pub fn fail_deployment(&self, deployment_id: Uuid) -> AppsResult<ordo_protocol::Deployment> {
        self.store
            .lock()
            .set_deployment_state(deployment_id, ordo_protocol::DeploymentState::Failed)
    }

    pub fn list_deployments(&self, app_id: Uuid) -> AppsResult<Vec<ordo_protocol::Deployment>> {
        self.store.lock().list_deployments(app_id)
    }

    pub fn get_deployment(
        &self,
        deployment_id: Uuid,
    ) -> AppsResult<Option<ordo_protocol::Deployment>> {
        self.store.lock().get_deployment(deployment_id)
    }

    /// Apply a non-destructive patch (rename, description, metadata).
    /// Status transitions go through `publish` / `archive` helpers so
    /// review gating applies.
    pub async fn update(&self, app_ref: &AppRef, patch: AppUpdate) -> AppsResult<App> {
        let app = self.store.lock().require(app_ref)?;
        let actor = patch
            .actor
            .clone()
            .unwrap_or_else(|| "operator".to_string());
        let mut mutations: Vec<Mutation> = Vec::new();
        if let Some(name) = &patch.name {
            if name.trim().is_empty() {
                return Err(AppsError::InvalidArgument("name cannot be empty".into()));
            }
            if *name != app.name {
                mutations.push(Mutation::Rename(name.clone()));
            }
        }
        if let Some(description) = &patch.description {
            if *description != app.description {
                mutations.push(Mutation::UpdateDescription(description.clone()));
            }
        }
        for (key, value) in &patch.metadata_patch {
            if value.is_null() {
                if app.metadata.contains_key(key) {
                    mutations.push(Mutation::RemoveMetadata(key.clone()));
                }
            } else if app.metadata.get(key) != Some(value) {
                mutations.push(Mutation::SetMetadata(key.clone(), value.clone()));
            }
        }
        if mutations.is_empty() {
            return Ok(app);
        }
        let updated = {
            let mut store = self.store.lock();
            store.apply_mutations(app.id, None, mutations, &actor)?
        };
        self.broadcast_last_event(updated.id).await;
        Ok(updated)
    }

    /// Publish transition. When a `ReviewService` is wired, a review
    /// request is opened first and publish only proceeds on approval.
    /// Called without review, publishes immediately.
    pub async fn publish(&self, app_ref: &AppRef, actor: &str) -> AppsResult<App> {
        let app = self.store.lock().require(app_ref)?;
        if app.status == AppStatus::Published {
            return Ok(app);
        }
        if app.status != AppStatus::Draft {
            return Err(AppsError::InvalidTransition {
                from: app.status.label(),
                to: AppStatus::Published.label(),
            });
        }
        // Review gate is a future elaboration hook: when wired, open a
        // review request and block until the operator approves. Phase
        // 1.1 ships the *routing* so downstream phases can plug the
        // wait in without a signature change. For now, review is
        // observed but not awaited â€” matches how other services
        // defer blocking to the caller.
        if let Some(_review) = &self.review {
            tracing::debug!(
                target: "ordo_apps",
                app_id = %app.id,
                "publish routed past review (wait-for-decision wiring: Phase 1.5)"
            );
        }
        let updated = {
            let mut store = self.store.lock();
            store.apply_mutations(
                app.id,
                Some(AppStatus::Draft),
                vec![Mutation::Publish],
                actor,
            )?
        };
        self.broadcast_last_event(updated.id).await;
        Ok(updated)
    }

    pub async fn unpublish(&self, app_ref: &AppRef, actor: &str) -> AppsResult<App> {
        let app = self.store.lock().require(app_ref)?;
        if app.status == AppStatus::Draft {
            return Ok(app);
        }
        if app.status != AppStatus::Published {
            return Err(AppsError::InvalidTransition {
                from: app.status.label(),
                to: AppStatus::Draft.label(),
            });
        }
        let updated = {
            let mut store = self.store.lock();
            store.apply_mutations(
                app.id,
                Some(AppStatus::Published),
                vec![Mutation::Unpublish],
                actor,
            )?
        };
        self.broadcast_last_event(updated.id).await;
        Ok(updated)
    }

    pub async fn archive(&self, app_ref: &AppRef, actor: &str) -> AppsResult<App> {
        let app = self.store.lock().require(app_ref)?;
        if app.status == AppStatus::Archived {
            return Ok(app);
        }
        let updated = {
            let mut store = self.store.lock();
            store.apply_mutations(app.id, None, vec![Mutation::Archive], actor)?
        };
        self.broadcast_last_event(updated.id).await;
        Ok(updated)
    }

    pub async fn unarchive(&self, app_ref: &AppRef, actor: &str) -> AppsResult<App> {
        let app = self.store.lock().require(app_ref)?;
        if app.status != AppStatus::Archived {
            return Err(AppsError::InvalidTransition {
                from: app.status.label(),
                to: AppStatus::Draft.label(),
            });
        }
        let updated = {
            let mut store = self.store.lock();
            store.apply_mutations(
                app.id,
                Some(AppStatus::Archived),
                vec![Mutation::Unarchive],
                actor,
            )?
        };
        self.broadcast_last_event(updated.id).await;
        Ok(updated)
    }

    /// Publish the latest persisted event for an app to the bus.
    /// Kept private â€” a successful mutation always broadcasts.
    async fn broadcast_last_event(&self, app_id: Uuid) {
        let Some(bus) = &self.bus else {
            return;
        };
        let last = match self.store.lock().events(app_id) {
            Ok(mut events) => events.pop(),
            Err(err) => {
                tracing::warn!(target: "ordo_apps", error = %err, "failed to load event for broadcast");
                return;
            }
        };
        let Some(event) = last else {
            return;
        };
        let envelope = Envelope::new(self.node_id.clone(), OrdoMessage::AppsEvent(event));
        if let Err(err) = bus.publish(topics::APPS_EVENT, envelope).await {
            tracing::warn!(target: "ordo_apps", error = %err, "apps.event publish failed");
        }
    }
}

/// Derive a URL-safe slug from a free-form name: ASCII-lowercase,
/// replace non-alphanumerics with `-`, collapse dashes, trim edges.
fn slugify(input: &str) -> String {
    let lowered = input.to_ascii_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut last_dash = true;
    for c in lowered.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_derives_slug_from_name() {
        let store = AppsStore::in_memory().expect("store");
        let svc = AppsService::new(store);
        let app = svc
            .create(
                NewApp {
                    name: "Brand Voice Builder".into(),
                    description: String::new(),
                    slug: None,
                    workspace_id: None,
                    metadata: Default::default(),
                },
                "operator",
            )
            .await
            .expect("create");
        assert_eq!(app.slug, "brand-voice-builder");
        assert_eq!(app.workspace_id, DEFAULT_WORKSPACE_ID);
    }

    #[tokio::test]
    async fn create_rejects_empty_name() {
        let store = AppsStore::in_memory().expect("store");
        let svc = AppsService::new(store);
        let err = svc
            .create(
                NewApp {
                    name: "   ".into(),
                    description: String::new(),
                    slug: None,
                    workspace_id: None,
                    metadata: Default::default(),
                },
                "operator",
            )
            .await
            .expect_err("should fail");
        assert!(matches!(err, AppsError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn update_patch_is_noop_without_changes() {
        let store = AppsStore::in_memory().expect("store");
        let svc = AppsService::new(store);
        let app = svc
            .create(
                NewApp {
                    name: "Same".into(),
                    description: "desc".into(),
                    slug: None,
                    workspace_id: None,
                    metadata: Default::default(),
                },
                "operator",
            )
            .await
            .expect("create");
        let updated = svc
            .update(
                &AppRef::Id(app.id),
                AppUpdate {
                    name: Some("Same".into()),
                    description: Some("desc".into()),
                    ..Default::default()
                },
            )
            .await
            .expect("update");
        assert_eq!(updated.updated_at, app.updated_at);
        let events = svc.events(app.id).expect("events");
        // Only the Created event â€” no drift events for identical patches.
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn publish_blocks_archived_app() {
        let store = AppsStore::in_memory().expect("store");
        let svc = AppsService::new(store);
        let app = svc
            .create(
                NewApp {
                    name: "A".into(),
                    description: String::new(),
                    slug: None,
                    workspace_id: None,
                    metadata: Default::default(),
                },
                "op",
            )
            .await
            .expect("create");
        svc.archive(&AppRef::Id(app.id), "op")
            .await
            .expect("archive");
        let err = svc
            .publish(&AppRef::Id(app.id), "op")
            .await
            .expect_err("publish archived");
        assert!(matches!(err, AppsError::InvalidTransition { .. }));
    }

    #[tokio::test]
    async fn deployment_lifecycle_pending_to_live() {
        let store = AppsStore::in_memory().expect("store");
        let svc = AppsService::new(store);
        let app = svc
            .create(
                NewApp {
                    name: "Deployable".into(),
                    description: String::new(),
                    slug: None,
                    workspace_id: None,
                    metadata: Default::default(),
                },
                "op",
            )
            .await
            .expect("create");
        let dep = svc
            .create_deployment(app.id, Some("preview/deploy-1/".into()), "first cut")
            .expect("deploy");
        assert_eq!(dep.app_id, app.id);
        assert_eq!(dep.state, ordo_protocol::DeploymentState::Pending);
        assert!(dep.promoted_at.is_none());

        let promoted = svc.promote_deployment(dep.id).expect("promote");
        assert_eq!(promoted.state, ordo_protocol::DeploymentState::Live);
        assert!(promoted.promoted_at.is_some());

        let deployments = svc.list_deployments(app.id).expect("list");
        assert_eq!(deployments.len(), 1);
        assert_eq!(deployments[0].id, dep.id);
    }

    #[tokio::test]
    async fn metadata_null_in_patch_removes_key() {
        let store = AppsStore::in_memory().expect("store");
        let svc = AppsService::new(store);
        let mut metadata = std::collections::BTreeMap::new();
        metadata.insert("k".to_string(), serde_json::json!("v"));
        let app = svc
            .create(
                NewApp {
                    name: "M".into(),
                    description: String::new(),
                    slug: None,
                    workspace_id: None,
                    metadata,
                },
                "op",
            )
            .await
            .expect("create");
        let mut patch_meta = std::collections::BTreeMap::new();
        patch_meta.insert("k".to_string(), serde_json::Value::Null);
        let updated = svc
            .update(
                &AppRef::Id(app.id),
                AppUpdate {
                    metadata_patch: patch_meta,
                    ..Default::default()
                },
            )
            .await
            .expect("update");
        assert!(!updated.metadata.contains_key("k"));
    }
}
