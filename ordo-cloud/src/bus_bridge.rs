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
                handle_upsert(&task, &http, credential).await;
            }
            OrdoMessage::CloudCredentialRemoveRequest { service } => {
                handle_remove(&task, &http, service).await;
            }
            OrdoMessage::CloudCredentialTestRequest { service } => {
                handle_test(&bus, &node_id, &task, &http, service, correlation).await;
            }
            OrdoMessage::CloudCredentialSetDefaultRequest { service } => {
                handle_set_default(&task, &http, service).await;
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

async fn handle_upsert(
    task: &CloudCredentialTask,
    http: &CloudHttp,
    credential: ordo_protocol::CloudCredentialFull,
) {
    // If this edits the local model identity for an existing row, ask
    // the old local provider to unload before the new model can be used.
    let service = credential.service.clone();
    let previous = task.get(service).await.ok().flatten();
    let update = full_into_update(credential);
    match task.upsert(update).await {
        Ok(saved) => {
            if previous.as_ref().and_then(crate::local_model_identity)
                != crate::local_model_identity(&saved)
            {
                if let Some(previous) = previous.as_ref() {
                    match task
                        .release_local_model(http, previous, "credential_upsert_model_changed")
                        .await
                    {
                        Ok(report) => log_lifecycle_report("upsert model change", &report),
                        Err(err) => tracing::warn!(
                            target: "ordo_cloud_bridge",
                            error = %err,
                            "local model release after upsert failed"
                        ),
                    }
                }
            }
        }
        Err(err) => {
            tracing::warn!(
                target: "ordo_cloud_bridge",
                error = %err,
                "upsert failed"
            );
        }
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

async fn handle_remove(task: &CloudCredentialTask, http: &CloudHttp, service: String) {
    // If a removed local credential had a resident model, release it.
    let previous = task.get(service.clone()).await.ok().flatten();
    match task.delete(service).await {
        Ok(true) => {
            if let Some(previous) = previous.as_ref() {
                match task
                    .release_local_model(http, previous, "credential_deleted")
                    .await
                {
                    Ok(report) => log_lifecycle_report("credential delete", &report),
                    Err(err) => tracing::warn!(
                        target: "ordo_cloud_bridge",
                        error = %err,
                        "local model release after delete failed"
                    ),
                }
            }
        }
        Ok(false) => {}
        Err(err) => {
            tracing::warn!(
                target: "ordo_cloud_bridge",
                error = %err,
                "delete failed"
            );
        }
    }
}

async fn handle_set_default(task: &CloudCredentialTask, http: &CloudHttp, service: String) {
    // Release the old local model before the default pointer moves so local
    // runtimes do not stack LM Studio + Ollama during a provider switch.
    let previous_default = task.get_default().await.ok().flatten();
    let previous = match previous_default {
        Some(name) => task.get(name).await.ok().flatten(),
        None => None,
    };
    let next = task.get(service.clone()).await.ok().flatten();
    if previous.as_ref().and_then(crate::local_model_identity)
        != next.as_ref().and_then(crate::local_model_identity)
    {
        if let Some(previous) = previous.as_ref() {
            match task
                .release_local_model(http, previous, "default_provider_switch")
                .await
            {
                Ok(report) => log_lifecycle_report("default switch release", &report),
                Err(err) => tracing::warn!(
                    target: "ordo_cloud_bridge",
                    error = %err,
                    "local model release before default switch failed"
                ),
            }
        }
    }
    if let Err(err) = task.set_default(Some(service.clone())).await {
        tracing::warn!(
            target: "ordo_cloud_bridge",
            error = %err,
            "set_default failed"
        );
        return;
    }
    match task
        .enforce_single_local_model(http, Some(&service), "default_provider_switch")
        .await
    {
        Ok(report) => log_lifecycle_report("default switch enforce", &report),
        Err(err) => tracing::warn!(
            target: "ordo_cloud_bridge",
            error = %err,
            "local model enforcement after default switch failed"
        ),
    }
}

fn log_lifecycle_report(label: &str, report: &crate::LocalModelLifecycleReport) {
    if report.has_work() {
        tracing::info!(
            target: "ordo_cloud_bridge",
            label,
            unloaded = report.unloaded.len(),
            errors = report.errors.len(),
            "local model lifecycle applied"
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
