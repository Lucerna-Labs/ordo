//! Integration: the health task publishes ok / degraded events
//! on the bus, and a subscriber (Rescue Mode stand-in) can pick
//! them up. This is the acceptance criterion for concern 1: a
//! silent write-path failure becomes a first-class bus event
//! within the probe interval.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use ordo_bus::{Bus, InProcessBus};
use ordo_memory_log::{MemoryLogHealthTask, MemoryLogService, MemoryLogStore};
use ordo_protocol::{memory_topics, OrdoMessage};

#[tokio::test]
async fn health_ok_event_fires_on_successful_canary() {
    let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
    let log = MemoryLogService::new(MemoryLogStore::in_memory().expect("store"), "local")
        .with_bus(bus.clone());

    // Subscribe BEFORE the probe so we don't miss the publish.
    let mut ok_sub = bus
        .subscribe(memory_topics::LOG_HEALTH_OK)
        .await
        .expect("subscribe");

    let task = MemoryLogHealthTask::new(log, bus.clone()).with_interval(Duration::from_millis(10));

    // Drive one tick directly to avoid flake from the sleeper.
    task.tick_once().await;

    let env = tokio::time::timeout(Duration::from_secs(2), ok_sub.next())
        .await
        .expect("got ok event")
        .expect("envelope present");
    match env.payload {
        OrdoMessage::MemoryLogHealthOk(health) => {
            assert_eq!(health.appends_attempted, 1);
            assert_eq!(health.appends_succeeded, 1);
            assert!(health.last_successful_append_at_ms.is_some());
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[tokio::test]
async fn health_degraded_event_fires_when_write_path_broken() {
    // "Simulate a filesystem permission flip" translates to "drop
    // the memory_events table under the service's feet" in a pure
    // in-process test â€” the effect on the append path is the same:
    // every INSERT errors.
    let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
    let log = MemoryLogService::new(MemoryLogStore::in_memory().expect("store"), "local")
        .with_bus(bus.clone());

    // Rescue Mode stand-in â€” a subscriber that must see the
    // degraded event. The blueprint acceptance criterion is
    // verified here: a silent write-path failure becomes a
    // first-class subscribable bus event.
    let mut rescue_sub = bus
        .subscribe(memory_topics::LOG_HEALTH_DEGRADED)
        .await
        .expect("subscribe");

    // Break the write path.
    log.drop_events_table_for_tests();

    let task = MemoryLogHealthTask::new(log, bus.clone());
    task.tick_once().await;

    let env = tokio::time::timeout(Duration::from_secs(2), rescue_sub.next())
        .await
        .expect("rescue got degraded event")
        .expect("envelope present");
    match env.payload {
        OrdoMessage::MemoryLogHealthDegraded { reason, health } => {
            assert!(
                reason.contains("memory_events")
                    || reason.contains("no such table")
                    || reason.contains("storage"),
                "reason should name the underlying cause; got: {reason}"
            );
            assert_eq!(health.appends_succeeded, 0);
            assert_eq!(health.appends_failed_last_hour, 1);
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[tokio::test]
async fn integrity_result_fires_after_startup_sweep() {
    let bus: Arc<dyn Bus> = Arc::new(InProcessBus::new());
    let log = MemoryLogService::new(MemoryLogStore::in_memory().expect("store"), "local")
        .with_bus(bus.clone());

    // Seed a couple of valid events.
    use chrono::Utc;
    use ordo_protocol::{MemoryEvent, MemoryEventType, RetentionTier};
    use serde_json::json;
    for i in 0..3 {
        let payload = json!({"i": i});
        let hash = MemoryLogService::compute_payload_hash(&payload);
        let event = MemoryEvent {
            id: MemoryLogService::new_event_id(),
            timestamp_ms: Utc::now().timestamp_millis(),
            event_type: MemoryEventType::UserMessage,
            actor: "op".into(),
            domain: None,
            category: None,
            parent_id: None,
            turn_id: None,
            payload,
            payload_hash: hash,
            tier: RetentionTier::Hot,
            pinned: false,
            soft_deleted: false,
            soft_deleted_at: None,
            soft_deleted_reason: None,
        };
        log.append(event).await.expect("seed");
    }

    let mut sub = bus
        .subscribe(memory_topics::LOG_INTEGRITY_RESULT)
        .await
        .expect("subscribe");

    let report = log.run_integrity_sweep().await;
    assert!(report.passed);
    assert_eq!(report.checked_count, 3);

    let env = tokio::time::timeout(Duration::from_secs(1), sub.next())
        .await
        .expect("got integrity event")
        .expect("envelope");
    match env.payload {
        OrdoMessage::MemoryLogIntegrityResult(r) => {
            assert!(r.passed);
            assert_eq!(r.checked_count, 3);
        }
        other => panic!("unexpected: {other:?}"),
    }
}
