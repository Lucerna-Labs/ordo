//! Boot-time knowledge seeder (push 4).
//!
//! Populates the assistant's self-knowledge RAG with two classes of
//! entries so L2 (`assistant.knowledge_lookup`) is useful out of the
//! box instead of empty until an operator adds content:
//!
//! 1. **Skill cards** â€” one `Skill` entry per capability advertised on
//!    the bus. Title is the capability name, body is its description,
//!    domain is the first segment of the capability (so `planning.*`
//!    lives in the `planning` domain). Keyed by
//!    `source = "auto:capability:<name>"`, so re-seeding is idempotent
//!    and plugins that come online later get picked up on the next
//!    pass.
//!
//! 2. **Domain notes** â€” one `Note` per named domain slot
//!    (planning / orchestration / research) with a short blurb explaining
//!    what that domain covers. Gives the router something concrete to
//!    retrieve when it scopes `knowledge_lookup` to a domain.
//!
//! The seeder runs once at boot. A periodic refresh is cheap (the
//! upsert is keyed by source) and could be wired in later if plugin
//! hot-reload becomes common, but a one-shot pass is enough for now.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use ordo_bus::Bus;
use ordo_protocol::{
    topics, CapabilityDescriptor, CorrelationId, Envelope, NodeId, OrdoMessage,
    RAG_COLLECTION_ORCHESTRATION, RAG_COLLECTION_PLANNING, RAG_COLLECTION_RESEARCH,
};
use tokio::time::timeout;
use tracing::{info, warn};

use crate::knowledge::KnowledgeStore;
use crate::types::{AssistantError, AssistantResult, KnowledgeKind, NewKnowledge};

const DEFAULT_INVENTORY_TIMEOUT: Duration = Duration::from_secs(2);

/// Runs a one-shot seed of the self-knowledge RAG from the capability
/// inventory + static domain blurbs.
#[derive(Clone)]
pub struct KnowledgeSeeder {
    knowledge: KnowledgeStore,
    bus: Arc<dyn Bus>,
    inventory_timeout: Duration,
}

impl KnowledgeSeeder {
    pub fn new(knowledge: KnowledgeStore, bus: Arc<dyn Bus>) -> Self {
        Self {
            knowledge,
            bus,
            inventory_timeout: DEFAULT_INVENTORY_TIMEOUT,
        }
    }

    pub fn with_inventory_timeout(mut self, timeout: Duration) -> Self {
        self.inventory_timeout = timeout;
        self
    }

    /// Run one full seed pass. Returns the number of entries upserted.
    /// Errors are logged but not propagated per-entry â€” a partial seed
    /// is better than none.
    pub async fn seed_once(&self) -> AssistantResult<SeedReport> {
        let mut report = SeedReport::default();

        // Skill cards from bus-advertised capabilities.
        let descriptors = self.collect_inventory().await;
        for descriptor in descriptors {
            let entry = capability_skill_card(&descriptor);
            match self.knowledge.upsert_by_source(entry).await {
                Ok(_) => report.skills_upserted += 1,
                Err(err) => {
                    warn!(
                        target: "ordo_assistant::seeder",
                        capability = %descriptor.capability,
                        error = %err,
                        "failed to upsert skill card"
                    );
                    report.errors += 1;
                }
            }
        }

        // Static domain blurbs. Six reserved slots are *not* seeded â€”
        // they stay empty on purpose so the operator can claim them
        // with real content when a orchestration attaches.
        for (domain, title, body) in DOMAIN_BLURBS {
            let new_entry = NewKnowledge {
                kind: KnowledgeKind::Note,
                domain: Some((*domain).to_string()),
                title: (*title).to_string(),
                body: (*body).to_string(),
                source: format!("auto:domain:{domain}"),
                confidence: 0.7,
            };
            match self.knowledge.upsert_by_source(new_entry).await {
                Ok(_) => report.domains_upserted += 1,
                Err(err) => {
                    warn!(
                        target: "ordo_assistant::seeder",
                        domain = %domain,
                        error = %err,
                        "failed to upsert domain note"
                    );
                    report.errors += 1;
                }
            }
        }

        info!(
            target: "ordo_assistant::seeder",
            skills = report.skills_upserted,
            domains = report.domains_upserted,
            errors = report.errors,
            "self-knowledge seeding complete"
        );
        Ok(report)
    }

    async fn collect_inventory(&self) -> Vec<CapabilityDescriptor> {
        let correlation_id = CorrelationId::new();
        let envelope = Envelope::new(NodeId::new(), OrdoMessage::CapabilityInventoryRequested)
            .with_correlation(correlation_id.clone());
        let mut sub = match self
            .bus
            .subscribe(topics::CAPABILITY_INVENTORY_RESPONSE)
            .await
        {
            Ok(sub) => sub,
            Err(err) => {
                warn!(
                    target: "ordo_assistant::seeder",
                    error = %err,
                    "could not subscribe to capability inventory response"
                );
                return Vec::new();
            }
        };
        if let Err(err) = self
            .bus
            .publish(topics::CAPABILITY_INVENTORY_REQUEST, envelope)
            .await
        {
            warn!(
                target: "ordo_assistant::seeder",
                error = %err,
                "failed to publish capability inventory request"
            );
            return Vec::new();
        }

        let start = tokio::time::Instant::now();
        let mut descriptors: Vec<CapabilityDescriptor> = Vec::new();
        while start.elapsed() < self.inventory_timeout {
            match timeout(
                self.inventory_timeout.saturating_sub(start.elapsed()),
                sub.next(),
            )
            .await
            {
                Ok(Some(event)) => {
                    if event.correlation_id.as_ref() != Some(&correlation_id) {
                        continue;
                    }
                    if let OrdoMessage::CapabilityInventorySnapshot {
                        descriptors: snapshot,
                        ..
                    } = event.payload
                    {
                        descriptors.extend(snapshot);
                    }
                }
                _ => break,
            }
        }
        descriptors.sort_by(|a, b| a.capability.cmp(&b.capability));
        descriptors.dedup_by(|a, b| a.capability == b.capability);
        descriptors
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SeedReport {
    pub skills_upserted: usize,
    pub domains_upserted: usize,
    pub errors: usize,
}

/// Blurbs for each named domain slot. Reserved slots 4-10 stay
/// empty on purpose.
const DOMAIN_BLURBS: &[(&str, &str, &str)] = &[
    (
        RAG_COLLECTION_PLANNING,
        "Planning domain",
        "Task breakdowns, initiative plans, resource grouping, and operator-facing
         coordination notes. Use assistant.knowledge_lookup with domain='planning'
         for planning observations and past execution notes. planning.*
         capabilities support neutral project planning and resource organization.",
    ),
    (
        RAG_COLLECTION_ORCHESTRATION,
        "Orchestration domain",
        "Reviews, approvals, handoffs, and revision pipelines. Use
         assistant.knowledge_lookup with domain='orchestration' for playbooks on how
         work moves through stages. orchestration.* capabilities drive the
         actual pipeline transitions.",
    ),
    (
        RAG_COLLECTION_RESEARCH,
        "Research domain",
        "Source gathering, evidence notes, citation trails, and review context.
         Use assistant.knowledge_lookup with domain='research' before summarizing
         external material or comparing source quality. research.* capabilities
         should stay focused on evidence, not publishing metadata.",
    ),
];

/// Turn one capability descriptor into a `Skill` knowledge entry.
/// The `source` field is the idempotency key.
fn capability_skill_card(descriptor: &CapabilityDescriptor) -> NewKnowledge {
    let domain = descriptor
        .capability
        .split('.')
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let title = descriptor.capability.clone();
    let body = format!(
        "Capability `{cap}` provided by `{provider}`.\n\nDescription: {desc}\n\n\
         Tier: {tier:?} / Activation: {activation:?}\n\n\
         Call it through the bus once routing has pointed you at the \
         `{lane}.*` lane.",
        cap = descriptor.capability,
        provider = descriptor.provider,
        desc = descriptor.description,
        tier = descriptor.tier,
        activation = descriptor.activation,
        lane = descriptor.lane.name,
    );
    NewKnowledge {
        kind: KnowledgeKind::Skill,
        domain,
        title,
        body,
        source: format!("auto:capability:{}", descriptor.capability),
        confidence: 0.6,
    }
}

// The unused import warning without this: we only reference the
// AssistantError type via `AssistantResult<T>`.
#[allow(dead_code)]
type _AssistantErrorRef = AssistantError;
