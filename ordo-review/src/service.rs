//! `ReviewService` — the in-memory orchestrator sitting on top of the
//! SQLite store.
//!
//! Responsibilities:
//! - Accept new review requests and persist them.
//! - Keep an `Arc<Mutex<HashMap<id, oneshot::Sender<ReviewRequest>>>>`
//!   of live waiters so an agent calling `await_decision` is woken the
//!   instant the operator acts.
//! - Broadcast `ReviewEvent`s on a `tokio::sync::broadcast` channel the
//!   WebSocket endpoint subscribes to.
//! - Apply `approve` / `deny` / `edit` decisions atomically, updating
//!   SQLite and resolving any waiter in lockstep.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::{broadcast, oneshot};
use tokio::time::timeout;
use uuid::Uuid;

use crate::store::ReviewStore;
use crate::types::{
    NewReviewRequest, ReviewDecisionKind, ReviewError, ReviewEvent, ReviewRequest, ReviewResult,
    ReviewState,
};

const BROADCAST_CAPACITY: usize = 256;

type WaiterMap = Arc<Mutex<HashMap<Uuid, oneshot::Sender<ReviewRequest>>>>;

#[derive(Clone)]
pub struct ReviewService {
    store: Arc<Mutex<ReviewStore>>,
    waiters: WaiterMap,
    broadcast: broadcast::Sender<ReviewEvent>,
}

impl ReviewService {
    pub fn new(store: ReviewStore) -> Self {
        let (tx, _rx) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            store: Arc::new(Mutex::new(store)),
            waiters: Arc::new(Mutex::new(HashMap::new())),
            broadcast: tx,
        }
    }

    /// Subscribe to the event stream. Every caller gets its own
    /// receiver; slow receivers may see `RecvError::Lagged`.
    pub fn subscribe(&self) -> broadcast::Receiver<ReviewEvent> {
        self.broadcast.subscribe()
    }

    /// Snapshot of everything currently awaiting a decision.
    pub fn pending(&self) -> ReviewResult<Vec<ReviewRequest>> {
        self.store.lock().pending()
    }

    pub fn recent(&self, limit: usize) -> ReviewResult<Vec<ReviewRequest>> {
        self.store.lock().recent(limit)
    }

    pub fn get(&self, id: Uuid) -> ReviewResult<Option<ReviewRequest>> {
        self.store.lock().get(id)
    }

    /// Queue a new request. Fire-and-forget variant: returns the stored
    /// request immediately; callers that need to wait for the decision
    /// should use `request_and_wait`.
    pub fn request(&self, new_request: NewReviewRequest) -> ReviewResult<ReviewRequest> {
        let mut store = self.store.lock();
        let request = store.insert(new_request)?;
        drop(store);
        let _ = self.broadcast.send(ReviewEvent::Opened {
            request: request.clone(),
        });
        Ok(request)
    }

    /// Register a waiter on an already-submitted request and block
    /// until the operator resolves it or `wait` elapses. Useful when
    /// a caller (e.g. the assistant turn loop) needs to emit a
    /// \"requested\" event with the concrete id *before* awaiting.
    /// Safe to call even if the request has already resolved — in
    /// that case we return the stored row immediately.
    pub async fn wait_for(&self, id: Uuid, wait: Duration) -> ReviewResult<ReviewRequest> {
        // Fast path: already terminal.
        if let Some(existing) = self.store.lock().get(id)? {
            if existing.state.is_terminal() {
                return Ok(existing);
            }
        } else {
            return Err(ReviewError::NotFound(id));
        }
        let (tx, rx) = oneshot::channel();
        self.waiters.lock().insert(id, tx);

        match timeout(wait, rx).await {
            Ok(Ok(resolved)) => Ok(resolved),
            Ok(Err(_canceled)) => {
                self.waiters.lock().remove(&id);
                self.expire(id)?;
                Err(ReviewError::AlreadyResolved(id, "expired"))
            }
            Err(_elapsed) => {
                self.waiters.lock().remove(&id);
                let expired = self.expire(id)?;
                Err(ReviewError::AlreadyResolved(id, expired.state.label()))
            }
        }
    }

    /// Queue a new request and wait up to `wait` for the operator to
    /// act. Returns the final (possibly edited) request. When the wait
    /// elapses, the request is marked `Expired` and the error returned.
    pub async fn request_and_wait(
        &self,
        new_request: NewReviewRequest,
        wait: Duration,
    ) -> ReviewResult<ReviewRequest> {
        let request = self.request(new_request)?;
        let id = request.id;
        let (tx, rx) = oneshot::channel();
        self.waiters.lock().insert(id, tx);

        match timeout(wait, rx).await {
            Ok(Ok(resolved)) => Ok(resolved),
            Ok(Err(_canceled)) => {
                // Sender dropped without completing — shouldn't happen,
                // but treat as expired for safety.
                self.waiters.lock().remove(&id);
                self.expire(id)?;
                Err(ReviewError::AlreadyResolved(id, "expired"))
            }
            Err(_elapsed) => {
                self.waiters.lock().remove(&id);
                let expired = self.expire(id)?;
                Err(ReviewError::AlreadyResolved(id, expired.state.label()))
            }
        }
    }

    fn expire(&self, id: Uuid) -> ReviewResult<ReviewRequest> {
        let mut store = self.store.lock();
        match store.resolve(id, ReviewState::Expired, None, Some("waiter timed out")) {
            Ok(resolved) => {
                drop(store);
                let _ = self.broadcast.send(ReviewEvent::Resolved {
                    request: resolved.clone(),
                });
                Ok(resolved)
            }
            Err(ReviewError::AlreadyResolved(_, state)) => {
                // Someone resolved the request between our timeout and
                // our expire attempt. Pick up the actual terminal
                // state.
                drop(store);
                let _ = state;
                Ok(self
                    .store
                    .lock()
                    .get(id)?
                    .ok_or(ReviewError::NotFound(id))?)
            }
            Err(err) => Err(err),
        }
    }

    pub fn decide(&self, id: Uuid, decision: ReviewDecisionKind) -> ReviewResult<ReviewRequest> {
        let (state, edited_content, note) = match &decision {
            ReviewDecisionKind::Approve { note } => (ReviewState::Approved, None, note.clone()),
            ReviewDecisionKind::Deny { note } => (ReviewState::Denied, None, note.clone()),
            ReviewDecisionKind::Edit { content, note } => (
                ReviewState::EditedAndApproved,
                Some(content.clone()),
                note.clone(),
            ),
            ReviewDecisionKind::Expire => {
                return self.expire(id);
            }
        };
        let mut store = self.store.lock();
        let resolved = store.resolve(id, state, edited_content.as_deref(), note.as_deref())?;
        drop(store);
        // Wake any waiter tracking this id; ignore the no-waiter case.
        let waiter = self.waiters.lock().remove(&id);
        if let Some(tx) = waiter {
            let _ = tx.send(resolved.clone());
        }
        let _ = self.broadcast.send(ReviewEvent::Resolved {
            request: resolved.clone(),
        });
        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tokio::time::Duration;

    fn sample() -> NewReviewRequest {
        NewReviewRequest {
            origin_capability: "workflow.generate_copy".into(),
            origin_plugin: None,
            title: "Draft".into(),
            content_type: "text/markdown".into(),
            content: "# Draft".into(),
            metadata: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn request_then_approve_wakes_waiter() {
        let service = ReviewService::new(ReviewStore::in_memory().expect("store"));
        let queue = service.pending().expect("pending");
        assert!(queue.is_empty());

        let waiter_service = service.clone();
        let waiter = tokio::spawn(async move {
            waiter_service
                .request_and_wait(sample(), Duration::from_secs(2))
                .await
        });

        // Give the waiter a moment to insert itself.
        tokio::time::sleep(Duration::from_millis(10)).await;
        let pending = service.pending().expect("pending");
        assert_eq!(pending.len(), 1);

        let resolved = service
            .decide(
                pending[0].id,
                ReviewDecisionKind::Approve {
                    note: Some("ship it".into()),
                },
            )
            .expect("decide");
        assert_eq!(resolved.state, ReviewState::Approved);

        let waited = waiter.await.expect("join").expect("waited");
        assert_eq!(waited.state, ReviewState::Approved);
        assert_eq!(waited.decision_note.as_deref(), Some("ship it"));
    }

    #[tokio::test]
    async fn edit_returns_edited_content() {
        let service = ReviewService::new(ReviewStore::in_memory().expect("store"));
        let waiter_service = service.clone();
        let waiter = tokio::spawn(async move {
            waiter_service
                .request_and_wait(sample(), Duration::from_secs(2))
                .await
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        let pending = service.pending().expect("pending");
        service
            .decide(
                pending[0].id,
                ReviewDecisionKind::Edit {
                    content: "# Punched-up draft".into(),
                    note: Some("tighter opener".into()),
                },
            )
            .expect("decide");
        let waited = waiter.await.expect("join").expect("waited");
        assert_eq!(waited.state, ReviewState::EditedAndApproved);
        assert_eq!(waited.effective_content(), "# Punched-up draft");
    }

    #[tokio::test]
    async fn timeout_expires_request() {
        let service = ReviewService::new(ReviewStore::in_memory().expect("store"));
        let err = service
            .request_and_wait(sample(), Duration::from_millis(50))
            .await
            .expect_err("should time out");
        assert!(matches!(err, ReviewError::AlreadyResolved(_, "expired")));
        let pending = service.pending().expect("pending");
        assert!(
            pending.is_empty(),
            "expired requests should not stay pending"
        );
    }

    #[tokio::test]
    async fn broadcast_emits_opened_and_resolved() {
        let service = ReviewService::new(ReviewStore::in_memory().expect("store"));
        let mut rx = service.subscribe();
        let request = service.request(sample()).expect("request");
        let first = rx.recv().await.expect("opened event");
        assert!(matches!(first, ReviewEvent::Opened { .. }));
        service
            .decide(request.id, ReviewDecisionKind::Deny { note: None })
            .expect("decide");
        let second = rx.recv().await.expect("resolved event");
        assert!(matches!(second, ReviewEvent::Resolved { .. }));
    }
}
