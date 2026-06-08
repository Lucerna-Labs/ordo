//! Bus bridge for cloud-credential operations.
//!
//! Subscribes to the five `cloud_topics::*_REQUEST` topics, dispatches
//! to [`CloudCredentialTask`], publishes responses (re-using the
//! request's correlation id) and — for mutations — relies on the
//! task's own publish-after-commit hook for events.
//!
//! ## Concurrency model
//!
//! Single-threaded subscriber loop. The underlying `StorageTask`
//! actor already serializes mutations against the SQLite store;
//! parallelism in the bridge would just queue inside the task
//! anyway, and a serial bridge has no race surface to worry about.
//!
//! ## Error semantics
//!
//! - **List / Test**: publish the response variant either way;
//!   `TestResult.ok=false` and `error: Some(message)` carry the
//!   failure shape. List failure is rare (store-level only) and
//!   publishes an empty list with a tracing warning.
//! - **Upsert / Remove / SetDefault**: fire-and-forget. On
//!   success, the *task* publishes the event (`*Upserted`,
//!   `*Removed`, `*DefaultChanged`). On failure, the bridge logs
//!   via tracing and does *not* publish an event. Consumers should
//!   poll-with-timeout or refresh-on-next-list-response.
//!
//! ## What this module does NOT touch
//!
//! `ordo-control`, `ordo-mcp-host`. The existing HTTP
//! capability path (`cloud.credentials.list` / `upsert` / `delete`
//! tools) keeps working unmodified — it calls the same
//! `CloudCredentialTask`, which (once configured with `with_bus`)
//! automatically emits events on every mutation regardless of who
//! drove it.

use std::sync::Arc;

use futures::stream::{select_all, StreamExt};
use ordo_bus::Bus;
use ordo_protocol::{cloud_topics, Envelope, NodeId, OrdoMessage};

use crate::{test_credential, CloudCredentialTask, CloudCredentialUpdate, CloudHttp};

/// Run the cloud-credential bus bridge. Designed for the
/// runtime's `spawn_component("cloud-bridge", run_bus_bridge(...))`
/// pattern — returns `()`, logs and exits on error rather than
/// bubbling.
///
/// `task` should already have `.with_bus(bus.clone(), node_id.clone())`
/// applied so mutations driven by the existing HTTP path also
/// emit events — otherwise the bridge will still work for
/// bus-driven mutations but HTTP-driven changes won't be visible
/// to bus subscribers.
pub async fn run_bus_bridge(
    bus: Arc<dyn Bus>,
    node_id: NodeId,
    task: CloudCredentialTask,
    http: CloudHttp,
) {
    if let Err(err) = run(bus, node_id, task, http).await {
        tracing::error!(target: "ordo_cloud_bridge", error = %err, "bridge exited");
    }
}

async fn run(
    bus: Arc<dyn Bus>,
    node_id: NodeId,
    task: CloudCredentialTask,
    http: CloudHttp,
) -> Result<(), String> {
    // Subscribe to the five request topics. Failure on any one
    // is fatal — without all five the bridge can't honor the
    // contract advertised by `cloud_topics`.
    let topics = [
        cloud_topics::CREDENTIALS_LIST_REQUEST,
        cloud_topics::CREDENTIAL_UPSERT_REQUEST,
        cloud_topics::CREDENTIAL_REMOVE_REQUEST,
        cloud_topics::CREDENTIAL_TEST_REQUEST,
        cloud_topics::DEFAULT_SET_REQUEST,
    ];
    let mut streams = Vec::with_capacity(topics.len());
    for topic in topics {
        let stream = bus
            .subscribe(topic)
            .await
            .map_err(|err| format!("subscribe to {topic}: {err}"))?;
        streams.push(stream);
    }
    tracing::info!(
        target: "ordo_cloud_bridge",
        topics = topics.len(),
        "ordo-cloud-bridge: subscribed"
    );

    let mut merged = select_all(streams);
    while let Some(envelope) = merged.next().await {
        // Single-threaded loop — see module-level docs.
        let correlation = envelope.correlation_id.clone();
        match envelope.payload {
            OrdoMessage::CloudCredentialsListRequest => {
                handle_list(&bus, &node_id, &task, correlation).await;
            }
            OrdoMessage::CloudCredentialUpsertRequest { credential } => {
                handle_upsert(&task, credential).await;
            }
            OrdoMessage::CloudCredentialRemoveRequest { service } => {
                handle_remove(&task, service).await;
            }
            OrdoMessage::CloudCredentialTestRequest { service } => {
                handle_test(&bus, &node_id, &task, &http, service, correlation).await;
            }
            OrdoMessage::CloudCredentialSetDefaultRequest { service } => {
                handle_set_default(&task, service).await;
            }
            _ => {
                // Other variants leak through if a publisher sends
                // an unexpected message on one of the topics we
                // subscribe to. Ignore — not our concern.
            }
        }
    }
    tracing::info!(target: "ordo_cloud_bridge", "bridge stream ended");
    Ok(())
}

async fn handle_list(
    bus: &Arc<dyn Bus>,
    node_id: &NodeId,
    task: &CloudCredentialTask,
    correlation: Option<ordo_protocol::CorrelationId>,
) {
    let credentials = match task.list().await {
        Ok(creds) => creds.iter().map(|c| c.view()).collect::<Vec<_>>(),
        Err(err) => {
            tracing::warn!(
                target: "ordo_cloud_bridge",
                error = %err,
                "list failed; publishing empty response"
            );
            Vec::new()
        }
    };
    let default_service = match task.get_default().await {
        Ok(d) => d,
        Err(err) => {
            tracing::warn!(target: "ordo_cloud_bridge", error = %err, "get_default failed");
            None
        }
    };
    let mut envelope = Envelope::new(
        node_id.clone(),
        OrdoMessage::CloudCredentialsListResponse {
            credentials,
            default_service,
        },
    );
    if let Some(cid) = correlation {
        envelope = envelope.with_correlation(cid);
    }
    publish(bus, cloud_topics::CREDENTIALS_LIST_RESPONSE, envelope).await;
}

async fn handle_upsert(task: &CloudCredentialTask, credential: ordo_protocol::CloudCredentialFull) {
    // Convert protocol type → internal `CloudCredentialUpdate`.
    // Task publishes `CloudCredentialUpserted` on success via
    // its own publish-after-commit hook; nothing for the bridge
    // to do besides log on failure.
    let update = full_into_update(credential);
    if let Err(err) = task.upsert(update).await {
        tracing::warn!(
            target: "ordo_cloud_bridge",
            error = %err,
            "upsert failed"
        );
    }
}

/// Convert a wire `CloudCredentialFull` into the internal
/// upsert payload. The protocol struct carries every field as
/// a definite value (no `Option`s — clients always send a
/// complete credential); the internal `Update` uses
/// `Option`s for partial-update support that the bridge
/// doesn't expose by default.
///
/// **Empty-secret semantics (Cycle 3 amendment, landed during
/// Cycle 4):** an empty `secret` string is treated as "preserve
/// the existing secret" rather than "overwrite with empty." This
/// is what the UXI's Edit modal needs — the `CloudCredentialView`
/// it has in hand carries no secret, so editing other fields
/// without re-typing the API key must not zero the stored
/// credential. The store handles `secret: None` as preserve, so
/// the conversion just routes empty strings through that path.
fn full_into_update(full: ordo_protocol::CloudCredentialFull) -> CloudCredentialUpdate {
    CloudCredentialUpdate {
        service: full.service,
        label: Some(full.label),
        auth_style: Some(full.auth_style),
        secret: if full.secret.is_empty() {
            None
        } else {
            Some(full.secret)
        },
        base_url: full.base_url,
        extras: Some(full.extras),
    }
}

async fn handle_remove(task: &CloudCredentialTask, service: String) {
    // Task publishes `CloudCredentialRemoved` (and possibly
    // `CloudCredentialDefaultChanged` if the removed service was
    // the default) on success via its own publish-after-commit
    // hook.
    if let Err(err) = task.delete(service).await {
        tracing::warn!(
            target: "ordo_cloud_bridge",
            error = %err,
            "delete failed"
        );
    }
}

async fn handle_set_default(task: &CloudCredentialTask, service: String) {
    // Task publishes `CloudCredentialDefaultChanged` on success.
    if let Err(err) = task.set_default(Some(service)).await {
        tracing::warn!(
            target: "ordo_cloud_bridge",
            error = %err,
            "set_default failed"
        );
    }
}

async fn handle_test(
    bus: &Arc<dyn Bus>,
    node_id: &NodeId,
    task: &CloudCredentialTask,
    http: &CloudHttp,
    service: String,
    correlation: Option<ordo_protocol::CorrelationId>,
) {
    let result = perform_test(task, http, &service).await;
    let mut envelope = Envelope::new(
        node_id.clone(),
        OrdoMessage::CloudCredentialTestResult {
            service,
            ok: result.is_ok(),
            error: result.err(),
        },
    );
    if let Some(cid) = correlation {
        envelope = envelope.with_correlation(cid);
    }
    publish(bus, cloud_topics::CREDENTIAL_TEST_RESULT, envelope).await;
}

/// Bridge-side wrapper: look up the credential by service name,
/// then defer to the public `crate::test_credential` so this
/// path + the new MCP-tool path share one source of truth.
async fn perform_test(
    task: &CloudCredentialTask,
    http: &CloudHttp,
    service: &str,
) -> Result<(), String> {
    let cred = task
        .get(service.to_string())
        .await
        .map_err(|err| format!("store: {err}"))?
        .ok_or_else(|| format!("no credential configured for service '{service}'"))?;
    test_credential(http, &cred).await
}

async fn publish(bus: &Arc<dyn Bus>, topic: &str, envelope: Envelope<OrdoMessage>) {
    if let Err(err) = bus.publish(topic, envelope).await {
        tracing::warn!(
            target: "ordo_cloud_bridge",
            topic,
            error = %err,
            "publish failed"
        );
    }
}
