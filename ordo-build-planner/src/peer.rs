use futures::StreamExt;
use ordo_bus::Bus;
use ordo_protocol::{topics, BuildGateResult, BuildPlannerEvent, Envelope, NodeId, OrdoMessage};
use std::{collections::HashMap, sync::Arc};
use uuid::Uuid;

use crate::{
    BuildLedger, BuildLedgerTask, BuildPlanner, BuildPlannerDecision, BuildPlannerError,
    BuildRunStatus,
};

type DynError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum BuildPlannerPeerError {
    #[error("build planner did not find build {0}")]
    BuildNotFound(Uuid),
    #[error(transparent)]
    Planner(#[from] BuildPlannerError),
}

pub struct BuildPlannerPeer {
    node_id: NodeId,
    bus: Arc<dyn Bus>,
    planners: HashMap<Uuid, BuildPlanner>,
    ledger_task: Option<BuildLedgerTask>,
}

impl BuildPlannerPeer {
    pub fn new(bus: Arc<dyn Bus>) -> Self {
        Self::with_node_id(bus, NodeId::new())
    }

    pub fn with_node_id(bus: Arc<dyn Bus>, node_id: NodeId) -> Self {
        Self {
            node_id,
            bus,
            planners: HashMap::new(),
            ledger_task: None,
        }
    }

    pub fn with_ledgers(bus: Arc<dyn Bus>, ledgers: Vec<BuildLedger>) -> Self {
        let mut peer = Self::new(bus);
        peer.hydrate(ledgers);
        peer
    }

    pub fn with_store(
        bus: Arc<dyn Bus>,
        ledger_task: BuildLedgerTask,
        ledgers: Vec<BuildLedger>,
    ) -> Self {
        let mut peer = Self::with_ledgers(bus, ledgers);
        peer.ledger_task = Some(ledger_task);
        peer
    }

    pub fn hydrate(&mut self, ledgers: Vec<BuildLedger>) {
        for ledger in ledgers {
            self.planners
                .insert(ledger.build_id, BuildPlanner::from_ledger(ledger));
        }
    }

    pub fn get(&self, build_id: &Uuid) -> Option<&BuildPlanner> {
        self.planners.get(build_id)
    }

    pub fn ledgers_snapshot(&self) -> Vec<BuildLedger> {
        let mut ledgers: Vec<BuildLedger> = self
            .planners
            .values()
            .map(|planner| planner.ledger().clone())
            .collect();
        ledgers.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        ledgers
    }

    pub fn active_builds(&self) -> Vec<Uuid> {
        let mut ids: Vec<Uuid> = self
            .planners
            .iter()
            .filter_map(|(id, planner)| {
                (planner.ledger().status == BuildRunStatus::Active).then_some(*id)
            })
            .collect();
        ids.sort();
        ids
    }

    pub async fn start_build(&mut self, project_id: impl Into<String>) -> Result<Uuid, DynError> {
        let planner = BuildPlanner::new(project_id);
        let build_id = planner.ledger().build_id;
        let event = planner.start_event();
        self.save_ledger(planner.ledger()).await?;
        self.planners.insert(build_id, planner);
        self.publish_event(event).await?;
        Ok(build_id)
    }

    pub async fn handle_gate_result(
        &mut self,
        result: BuildGateResult,
    ) -> Result<BuildPlannerDecision, DynError> {
        let build_id = result.build_id;
        let (decision, ledger) = {
            let planner = self
                .planners
                .get_mut(&build_id)
                .ok_or(BuildPlannerPeerError::BuildNotFound(build_id))?;
            let decision = planner.handle_gate_result(result)?;
            let ledger = planner.ledger().clone();
            (decision, ledger)
        };
        self.save_ledger(&ledger).await?;
        if let Some(event) = planner_event_from_decision(&ledger, &decision) {
            self.publish_event(event).await?;
        }
        Ok(decision)
    }

    pub async fn run(&mut self) -> Result<(), DynError> {
        let mut subscription = self.bus.subscribe(topics::BUILD_GATE_RESULT).await?;
        while let Some(envelope) = subscription.next().await {
            if let OrdoMessage::BuildGateResult(result) = envelope.payload {
                self.handle_gate_result(result).await?;
            }
        }
        Ok(())
    }

    async fn save_ledger(&self, ledger: &BuildLedger) -> Result<(), DynError> {
        if let Some(task) = &self.ledger_task {
            task.save(ledger.clone())
                .await
                .map_err(|err| -> DynError { Box::new(std::io::Error::other(err.to_string())) })?;
        }
        Ok(())
    }

    async fn publish_event(&self, event: BuildPlannerEvent) -> Result<(), DynError> {
        let envelope = Envelope::new(self.node_id.clone(), OrdoMessage::BuildPlannerEvent(event));
        self.bus
            .publish(topics::BUILD_PLANNER_EVENT, envelope)
            .await
            .map_err(|err| err.into())
    }
}

fn planner_event_from_decision(
    ledger: &BuildLedger,
    decision: &BuildPlannerDecision,
) -> Option<BuildPlannerEvent> {
    match decision {
        BuildPlannerDecision::Advance(event)
        | BuildPlannerDecision::HardHalt(event)
        | BuildPlannerDecision::Deferred(event) => Some(event.clone()),
        BuildPlannerDecision::AutonomousRetryEligible {
            step,
            error_class,
            summary,
        } => Some(BuildPlannerEvent::AutonomousRetryRequested {
            build_id: ledger.build_id,
            project_id: ledger.project_id.clone(),
            step: *step,
            error_class: *error_class,
            summary: summary.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::BuildPlannerPeer;
    use futures::StreamExt;
    use ordo_bus::{Bus, InProcessBus};
    use ordo_protocol::{
        topics, BuildErrorClass, BuildGateEvidence, BuildGateResult, BuildPlannerEvent, BuildStep,
        GateOutcome, OrdoMessage,
    };
    use std::{sync::Arc, time::Duration};

    fn pass_for(peer: &BuildPlannerPeer, build_id: uuid::Uuid, step: BuildStep) -> BuildGateResult {
        let planner = peer.get(&build_id).expect("planner");
        BuildGateResult {
            build_id,
            project_id: planner.ledger().project_id.clone(),
            step,
            outcome: GateOutcome::Pass {
                evidence: BuildGateEvidence::new(format!("{step:?} passed")),
            },
        }
    }

    async fn next_planner_event(
        stream: &mut (dyn futures::Stream<Item = ordo_protocol::BusEnvelope> + Unpin + Send),
    ) -> BuildPlannerEvent {
        let envelope = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("planner event timeout")
            .expect("planner event envelope");
        match envelope.payload {
            OrdoMessage::BuildPlannerEvent(event) => event,
            other => panic!("unexpected message {other:?}"),
        }
    }

    #[tokio::test]
    async fn start_build_publishes_released_intake_skill() {
        let bus = Arc::new(InProcessBus::new());
        let mut subscription = bus
            .subscribe(topics::BUILD_PLANNER_EVENT)
            .await
            .expect("subscribe");
        let mut peer = BuildPlannerPeer::new(bus);

        let build_id = peer.start_build("demo").await.expect("start build");
        let event = next_planner_event(&mut *subscription).await;

        assert_eq!(peer.active_builds(), vec![build_id]);
        match event {
            BuildPlannerEvent::BuildStarted { released_skill, .. } => {
                assert_eq!(released_skill, "ordo-build-intake");
            }
            other => panic!("unexpected event {other:?}"),
        }
    }

    #[tokio::test]
    async fn gate_pass_advances_and_publishes_next_skill() {
        let bus = Arc::new(InProcessBus::new());
        let mut subscription = bus
            .subscribe(topics::BUILD_PLANNER_EVENT)
            .await
            .expect("subscribe");
        let mut peer = BuildPlannerPeer::new(bus);
        let build_id = peer.start_build("demo").await.expect("start build");
        let _started = next_planner_event(&mut *subscription).await;

        peer.handle_gate_result(pass_for(&peer, build_id, BuildStep::Intake))
            .await
            .expect("gate pass");
        let event = next_planner_event(&mut *subscription).await;

        match event {
            BuildPlannerEvent::StepAdvanced {
                completed_step,
                next_step,
                released_skill,
                ..
            } => {
                assert_eq!(completed_step, BuildStep::Intake);
                assert_eq!(next_step, Some(BuildStep::Blueprint));
                assert_eq!(released_skill.as_deref(), Some("ordo-build-blueprint"));
            }
            other => panic!("unexpected event {other:?}"),
        }
    }

    #[tokio::test]
    async fn autonomous_retry_decision_is_published_without_halting() {
        let bus = Arc::new(InProcessBus::new());
        let mut subscription = bus
            .subscribe(topics::BUILD_PLANNER_EVENT)
            .await
            .expect("subscribe");
        let mut peer = BuildPlannerPeer::new(bus);
        let build_id = peer.start_build("demo").await.expect("start build");
        let _started = next_planner_event(&mut *subscription).await;
        peer.planners
            .get_mut(&build_id)
            .expect("planner")
            .ledger_mut()
            .autonomous_correction = true;
        let project_id = peer
            .get(&build_id)
            .expect("planner")
            .ledger()
            .project_id
            .clone();

        peer.handle_gate_result(BuildGateResult {
            build_id,
            project_id,
            step: BuildStep::Intake,
            outcome: GateOutcome::Fail {
                error_class: BuildErrorClass::CompileWarnings,
                evidence: BuildGateEvidence::new("warning found"),
            },
        })
        .await
        .expect("retry event");
        let event = next_planner_event(&mut *subscription).await;

        match event {
            BuildPlannerEvent::AutonomousRetryRequested {
                step, error_class, ..
            } => {
                assert_eq!(step, BuildStep::Intake);
                assert_eq!(error_class, BuildErrorClass::CompileWarnings);
            }
            other => panic!("unexpected event {other:?}"),
        }
    }

    #[tokio::test]
    async fn ledgers_can_hydrate_peer_on_restart() {
        let bus = Arc::new(InProcessBus::new());
        let mut peer = BuildPlannerPeer::new(bus.clone());
        let build_id = peer.start_build("demo").await.expect("start build");
        let ledgers = peer.ledgers_snapshot();
        let restarted = BuildPlannerPeer::with_ledgers(bus, ledgers);

        assert_eq!(restarted.active_builds(), vec![build_id]);
        assert_eq!(
            restarted
                .get(&build_id)
                .expect("planner")
                .ledger()
                .project_id,
            "demo"
        );
    }
}
