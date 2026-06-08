//! Ordo webhooks (Phase 3.1).
//!
//! An external subscriber registers a URL + shared secret + topic
//! filter. When any matching bus envelope fires, the `WebhookService`
//! POSTs a JSON body to the URL, signed with HMAC-SHA256 over the raw
//! body.
//!
//! Signature header: `X-Ordo-Signature: sha256=<hex>`.
//! Timestamp header: `X-Ordo-Timestamp: <rfc3339>`.
//!
//! Delivery is **best-effort at-most-once**. Phase 3.1 intentionally
//! skips retry queues â€” if the subscriber is down, the event is
//! logged and the delivery metadata on the subscription is updated
//! so the operator can see failing endpoints. Retries are a Phase 4
//! elaboration.
//!
//! Follows the architecture contract:
//!   - Rule 6 (workspace_id from day one in the subscriptions table)
//!   - Rule 10 (uses existing channel primitives; the dispatcher is
//!     a bus subscriber that spawns per-event tokio tasks)
//!   - Rule 11 (wire type lives in `ordo-protocol`)

pub mod dispatcher;
pub mod service;
pub mod signing;
pub mod store;
pub mod types;

pub use dispatcher::WebhookDispatcher;
pub use service::{WebhookService, DEFAULT_WORKSPACE_ID};
pub use store::WebhookStore;
pub use types::{NewSubscription, SubscriptionUpdate, WebhookError, WebhookResult};
