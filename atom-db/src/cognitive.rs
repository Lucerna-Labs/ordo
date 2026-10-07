use crate::{AtomDb, Digest};
use std::collections::{BTreeMap, BTreeSet};

const PER_MILLE: u128 = 1_000;
const BASE_CONDUCTANCE: u32 = 1_000;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LearningScope {
    Global,
    #[default]
    Contextual,
    GuardedRelay,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CognitiveConfig {
    pub working_memory_capacity: usize,
    pub max_depth: usize,
    pub cue_activation: u64,
    pub context_activation: u64,
    pub propagation_per_mille: u16,
    pub highway_threshold: u32,
    pub observation_half_life: u64,
    pub learning_scope: LearningScope,
    pub context_field_depth: usize,
    pub context_field_capacity: usize,
    pub context_field_propagation_per_mille: u16,
    pub relay_guard_per_mille: u16,
    pub context_merge_per_mille: u16,
    pub context_trace_capacity: usize,
}

impl Default for CognitiveConfig {
    fn default() -> Self {
        Self {
            working_memory_capacity: 12,
            max_depth: 5,
            cue_activation: 1_000_000,
            context_activation: 350_000,
            propagation_per_mille: 700,
            highway_threshold: 1_100,
            observation_half_life: 32,
            learning_scope: LearningScope::Contextual,
            context_field_depth: 2,
            context_field_capacity: 64,
            context_field_propagation_per_mille: 600,
            relay_guard_per_mille: 50,
            context_merge_per_mille: 800,
            context_trace_capacity: 32,
        }
    }
}

impl CognitiveConfig {
    pub fn validate(self) -> Result<Self, String> {
        if self.working_memory_capacity == 0 || self.working_memory_capacity > 4_096 {
            return Err("working-memory capacity must be between 1 and 4096".into());
        }
        if self.max_depth == 0 || self.max_depth > 64 {
            return Err("recall depth must be between 1 and 64".into());
        }
        if self.cue_activation == 0 || self.context_activation == 0 {
            return Err("cue and context activation must be nonzero".into());
        }
        if self.propagation_per_mille == 0 || self.propagation_per_mille > 1_000 {
            return Err("propagation must be between 1 and 1000 per mille".into());
        }
        if self.highway_threshold < BASE_CONDUCTANCE {
            return Err("highway threshold cannot be below untouched conductance".into());
        }
        if self.context_field_depth == 0 || self.context_field_depth > 8 {
            return Err("context-field depth must be between 1 and 8".into());
        }
        if self.context_field_capacity == 0 || self.context_field_capacity > 4_096 {
            return Err("context-field capacity must be between 1 and 4096".into());
        }
        if self.context_field_propagation_per_mille == 0
            || self.context_field_propagation_per_mille > 1_000
        {
            return Err("context-field propagation must be between 1 and 1000 per mille".into());
        }
        if self.relay_guard_per_mille > 1_000 || self.context_merge_per_mille > 1_000 {
            return Err("relay guard and context merge thresholds cannot exceed 1000".into());
        }
        if self.context_merge_per_mille < self.relay_guard_per_mille {
            return Err("context merge threshold cannot be below the relay guard".into());
        }
        if self.context_trace_capacity == 0 || self.context_trace_capacity > 65_536 {
            return Err("context-trace capacity must be between 1 and 65536".into());
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BondMemory {
    pub attempts: u64,
    pub successes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActivatedAtom {
    pub identity: Digest,
    pub activation: u64,
    pub depth: usize,
    pub incoming_signals: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecallThread {
    pub atoms: Vec<Digest>,
    pub bonds: Vec<Digest>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecallReport {
    pub cue: Digest,
    pub contexts: Vec<Digest>,
    pub ranked: Vec<ActivatedAtom>,
    pub target: Option<Digest>,
    pub target_found: bool,
    pub thread: Option<RecallThread>,
    pub intersections: Vec<Digest>,
    pub explored_bonds: usize,
    pub working_set_peak: usize,
    pub promoted_hops: usize,
    pub explanation: String,
}

#[derive(Clone, Copy, Debug)]
struct Arc {
    bond: Digest,
    to: Digest,
}

#[derive(Clone, Copy, Debug)]
struct Trace {
    activation: u64,
    depth: usize,
    incoming_signals: usize,
    predecessor: Option<(Digest, Digest)>,
}

#[derive(Clone, Copy, Debug, Default)]
struct Candidate {
    activation: u64,
    incoming_signals: usize,
    strongest_signal: u64,
    predecessor: Option<(Digest, Digest)>,
}

type ContextField = BTreeMap<Digest, u16>;

#[derive(Clone, Debug, Default)]
struct ContextTrace {
    field: ContextField,
    bond_memory: BTreeMap<Digest, BondMemory>,
    observations: u64,
}

#[derive(Debug)]
pub struct CognitiveEngine {
    config: CognitiveConfig,
    memories: BTreeMap<Vec<Digest>, BTreeMap<Digest, BondMemory>>,
    context_traces: Vec<ContextTrace>,
    evicted_context_traces: u64,
    observations: u64,
}

impl CognitiveEngine {
    pub fn new(config: CognitiveConfig) -> Result<Self, String> {
        Ok(Self {
            config: config.validate()?,
            memories: BTreeMap::new(),
            context_traces: Vec::new(),
            evicted_context_traces: 0,
            observations: 0,
        })
    }

    pub fn config(&self) -> CognitiveConfig {
        self.config
    }

    pub fn observations(&self) -> u64 {
        self.observations
    }

    pub fn context_trace_count(&self) -> usize {
        self.context_traces.len()
    }

    pub fn evicted_context_traces(&self) -> u64 {
        self.evicted_context_traces
    }

    pub fn bond_memory(&self, identity: Digest) -> BondMemory {
        self.bond_memory_in(&[], identity)
    }

    pub fn bond_memory_in(&self, contexts: &[Digest], identity: Digest) -> BondMemory {
        let key = self.learning_key(contexts);
        self.memories
            .get(&key)
            .and_then(|lane| lane.get(&identity))
            .copied()
            .unwrap_or_default()
    }

    pub fn bond_conductance(&self, identity: Digest) -> u32 {
        self.bond_conductance_in(&[], identity)
    }

    pub fn bond_conductance_in(&self, contexts: &[Digest], identity: Digest) -> u32 {
        let key = self.learning_key(contexts);
        let Some(memory) = self.memories.get(&key).and_then(|lane| lane.get(&identity)) else {
            return BASE_CONDUCTANCE;
        };
        memory_conductance(memory)
    }

    pub fn bond_conductance_for(&self, db: &AtomDb, contexts: &[Digest], identity: Digest) -> u32 {
        let contexts = normalized_contexts(contexts);
        let graph = build_graph(db);
        let field = build_context_field(&graph, &contexts, self.config);
        self.bond_conductance_with_field(&contexts, &field, identity)
    }

    pub fn context_compatibility(
        &self,
        db: &AtomDb,
        left_contexts: &[Digest],
        right_contexts: &[Digest],
    ) -> u16 {
        let graph = build_graph(db);
        let left = build_context_field(&graph, &normalized_contexts(left_contexts), self.config);
        let right = build_context_field(&graph, &normalized_contexts(right_contexts), self.config);
        field_compatibility(&left, &right)
    }

    fn bond_conductance_with_field(
        &self,
        contexts: &[Digest],
        field: &ContextField,
        identity: Digest,
    ) -> u32 {
        match self.config.learning_scope {
            LearningScope::Global => self.bond_conductance_in(&[], identity),
            LearningScope::Contextual => self.bond_conductance_in(contexts, identity),
            LearningScope::GuardedRelay if contexts.is_empty() => {
                self.bond_conductance_in(&[], identity)
            }
            LearningScope::GuardedRelay => self.guarded_relay_conductance(field, identity),
        }
    }

    fn guarded_relay_conductance(&self, field: &ContextField, identity: Digest) -> u32 {
        let mut weighted_delta = 0_i128;
        let mut total_compatibility = 0_u128;
        for trace in &self.context_traces {
            let Some(memory) = trace.bond_memory.get(&identity) else {
                continue;
            };
            let compatibility = field_compatibility(field, &trace.field);
            if compatibility < self.config.relay_guard_per_mille {
                continue;
            }
            let delta = memory_conductance(memory) as i128 - BASE_CONDUCTANCE as i128;
            weighted_delta += delta * compatibility as i128;
            total_compatibility += compatibility as u128;
        }
        let denominator = total_compatibility.max(PER_MILLE) as i128;
        (BASE_CONDUCTANCE as i128 + weighted_delta / denominator).clamp(250, 2_000) as u32
    }

    pub fn recall(
        &mut self,
        db: &AtomDb,
        cue: Digest,
        contexts: &[Digest],
        target: Option<Digest>,
    ) -> Result<RecallReport, String> {
        self.recall_impl(db, cue, contexts, target, target.map(|_| true))
    }

    /// Recall with an explicit externally observed outcome.
    ///
    /// The original `recall(..., Some(target))` contract treats reaching the
    /// target as a successful supervised observation. Retrieval systems also
    /// receive explicit negative judgments for a target that was reached but
    /// proved unhelpful. This entry point preserves the same explainable recall
    /// thread while allowing that judgment to inhibit the explored route
    /// instead of accidentally reinforcing it.
    pub fn recall_with_feedback(
        &mut self,
        db: &AtomDb,
        cue: Digest,
        contexts: &[Digest],
        target: Digest,
        useful: bool,
    ) -> Result<RecallReport, String> {
        self.recall_impl(db, cue, contexts, Some(target), Some(useful))
    }

    fn recall_impl(
        &mut self,
        db: &AtomDb,
        cue: Digest,
        contexts: &[Digest],
        target: Option<Digest>,
        feedback: Option<bool>,
    ) -> Result<RecallReport, String> {
        if !db.contains_atom(cue) {
            return Err(format!("cue atom {cue} does not exist"));
        }
        for context in contexts {
            if !db.contains_atom(*context) {
                return Err(format!("context atom {context} does not exist"));
            }
        }
        if let Some(target) = target
            && !db.contains_atom(target)
        {
            return Err(format!("target atom {target} does not exist"));
        }

        let contexts = normalized_contexts(contexts);

        let graph = build_graph(db);
        let context_field = build_context_field(&graph, &contexts, self.config);
        let mut traces = BTreeMap::new();
        traces.insert(
            cue,
            Trace {
                activation: self.config.cue_activation,
                depth: 0,
                incoming_signals: 1,
                predecessor: None,
            },
        );
        for context in &contexts {
            let trace = traces.entry(*context).or_insert(Trace {
                activation: 0,
                depth: 0,
                incoming_signals: 0,
                predecessor: None,
            });
            trace.activation = trace
                .activation
                .saturating_add(self.config.context_activation);
            trace.incoming_signals += 1;
        }

        let mut active: Vec<(Digest, u64)> = traces
            .iter()
            .map(|(identity, trace)| (*identity, trace.activation))
            .collect();
        sort_active(&mut active);
        active.truncate(self.config.working_memory_capacity);
        let mut expanded = BTreeSet::new();
        let mut explored_bonds = BTreeSet::new();
        let mut promoted_hops = 0usize;
        let mut working_set_peak = active.len();

        for depth in 1..=self.config.max_depth {
            if active.is_empty() {
                break;
            }
            let mut candidates: BTreeMap<Digest, Candidate> = BTreeMap::new();
            for (from, activation) in &active {
                expanded.insert(*from);
                let Some(arcs) = graph.get(from) else {
                    continue;
                };
                let degree = arcs.len().max(1) as u128;
                for arc in arcs {
                    explored_bonds.insert(arc.bond);
                    if expanded.contains(&arc.to) {
                        continue;
                    }
                    let conductance =
                        self.bond_conductance_with_field(&contexts, &context_field, arc.bond);
                    if conductance >= self.config.highway_threshold {
                        promoted_hops += 1;
                    }
                    let transmitted = ((*activation as u128)
                        .saturating_mul(self.config.propagation_per_mille as u128)
                        .saturating_mul(conductance as u128)
                        / PER_MILLE
                        / PER_MILLE
                        / degree)
                        .min(u64::MAX as u128) as u64;
                    if transmitted == 0 {
                        continue;
                    }
                    let candidate = candidates.entry(arc.to).or_default();
                    candidate.activation = candidate.activation.saturating_add(transmitted);
                    candidate.incoming_signals += 1;
                    if transmitted > candidate.strongest_signal {
                        candidate.strongest_signal = transmitted;
                        candidate.predecessor = Some((*from, arc.bond));
                    }
                }
            }

            let mut next: Vec<(Digest, Candidate)> = candidates.into_iter().collect();
            next.sort_by(|(left_id, left), (right_id, right)| {
                right
                    .activation
                    .cmp(&left.activation)
                    .then_with(|| left_id.cmp(right_id))
            });
            next.truncate(self.config.working_memory_capacity);
            working_set_peak = working_set_peak.max(next.len());
            active.clear();
            for (identity, candidate) in next {
                let should_replace = traces
                    .get(&identity)
                    .is_none_or(|existing| candidate.activation > existing.activation);
                if should_replace {
                    traces.insert(
                        identity,
                        Trace {
                            activation: candidate.activation,
                            depth,
                            incoming_signals: candidate.incoming_signals,
                            predecessor: candidate.predecessor,
                        },
                    );
                }
                active.push((identity, candidate.activation));
            }
        }

        let target_found = target.is_some_and(|identity| traces.contains_key(&identity));
        let thread = target
            .filter(|_| target_found)
            .and_then(|identity| reconstruct_thread(identity, &traces));
        if let Some(useful) = feedback {
            self.observe_feedback(
                &contexts,
                &context_field,
                &explored_bonds,
                useful.then_some(thread.as_ref()).flatten(),
            );
        }

        let mut ranked: Vec<ActivatedAtom> = traces
            .into_iter()
            .map(|(identity, trace)| ActivatedAtom {
                identity,
                activation: trace.activation,
                depth: trace.depth,
                incoming_signals: trace.incoming_signals,
            })
            .collect();
        ranked.sort_by(|left, right| {
            right
                .activation
                .cmp(&left.activation)
                .then_with(|| left.identity.cmp(&right.identity))
        });
        let intersections = ranked
            .iter()
            .filter(|atom| atom.incoming_signals > 1)
            .map(|atom| atom.identity)
            .collect::<Vec<_>>();
        let explanation = explain(
            cue,
            &contexts,
            target,
            target_found,
            thread.as_ref(),
            (explored_bonds.len(), working_set_peak, promoted_hops),
        );
        Ok(RecallReport {
            cue,
            contexts,
            ranked,
            target,
            target_found,
            thread,
            intersections,
            explored_bonds: explored_bonds.len(),
            working_set_peak,
            promoted_hops,
            explanation,
        })
    }

    fn observe_feedback(
        &mut self,
        contexts: &[Digest],
        context_field: &ContextField,
        explored_bonds: &BTreeSet<Digest>,
        thread: Option<&RecallThread>,
    ) {
        let successful = thread
            .map(|thread| thread.bonds.iter().copied().collect::<BTreeSet<_>>())
            .unwrap_or_default();
        if self.config.learning_scope == LearningScope::GuardedRelay && !contexts.is_empty() {
            self.update_guarded_trace(context_field, explored_bonds, &successful);
        } else {
            let key = self.learning_key(contexts);
            let lane = self.memories.entry(key).or_default();
            update_memory_lane(lane, explored_bonds, &successful);
        }
        self.observations = self.observations.saturating_add(1);
        if self.config.observation_half_life != 0
            && self
                .observations
                .is_multiple_of(self.config.observation_half_life)
        {
            self.apply_half_life();
        }
    }

    fn apply_half_life(&mut self) {
        self.memories.retain(|_, lane| {
            lane.retain(|_, memory| {
                memory.attempts /= 2;
                memory.successes /= 2;
                memory.attempts != 0 || memory.successes != 0
            });
            !lane.is_empty()
        });
        self.context_traces.retain_mut(|trace| {
            trace.bond_memory.retain(|_, memory| {
                memory.attempts /= 2;
                memory.successes /= 2;
                memory.attempts != 0 || memory.successes != 0
            });
            trace.observations /= 2;
            !trace.bond_memory.is_empty()
        });
    }

    fn update_guarded_trace(
        &mut self,
        field: &ContextField,
        explored_bonds: &BTreeSet<Digest>,
        successful: &BTreeSet<Digest>,
    ) {
        let best = self
            .context_traces
            .iter()
            .enumerate()
            .map(|(index, trace)| (index, field_compatibility(field, &trace.field)))
            .max_by_key(|(index, compatibility)| (*compatibility, std::cmp::Reverse(*index)));
        let index = match best {
            Some((index, compatibility))
                if compatibility >= self.config.context_merge_per_mille =>
            {
                index
            }
            _ => {
                if self.context_traces.len() >= self.config.context_trace_capacity {
                    let weakest = self
                        .context_traces
                        .iter()
                        .enumerate()
                        .min_by_key(|(index, trace)| (trace.observations, *index))
                        .map(|(index, _)| index)
                        .expect("positive trace capacity");
                    self.context_traces.remove(weakest);
                    self.evicted_context_traces = self.evicted_context_traces.saturating_add(1);
                }
                self.context_traces.push(ContextTrace {
                    field: field.clone(),
                    ..ContextTrace::default()
                });
                self.context_traces.len() - 1
            }
        };
        let trace = &mut self.context_traces[index];
        merge_context_field(&mut trace.field, field, trace.observations);
        trace.observations = trace.observations.saturating_add(1);
        update_memory_lane(&mut trace.bond_memory, explored_bonds, successful);
    }

    fn learning_key(&self, contexts: &[Digest]) -> Vec<Digest> {
        if self.config.learning_scope == LearningScope::Global {
            return Vec::new();
        }
        let mut key = contexts.to_vec();
        key.sort_unstable();
        key.dedup();
        key
    }
}

fn normalized_contexts(contexts: &[Digest]) -> Vec<Digest> {
    let mut normalized = contexts.to_vec();
    normalized.sort_unstable();
    normalized.dedup();
    normalized
}

fn memory_conductance(memory: &BondMemory) -> u32 {
    let quality = ((memory.successes as u128 + 1) * 1_000 / (memory.attempts as u128 + 2)) as u32;
    let experience = memory.attempts.min(100) as u32 * 4;
    (500 + quality + experience).clamp(250, 2_000)
}

fn update_memory_lane(
    lane: &mut BTreeMap<Digest, BondMemory>,
    explored_bonds: &BTreeSet<Digest>,
    successful: &BTreeSet<Digest>,
) {
    for identity in explored_bonds {
        let memory = lane.entry(*identity).or_default();
        memory.attempts = memory.attempts.saturating_add(1);
        if successful.contains(identity) {
            memory.successes = memory.successes.saturating_add(1);
        }
    }
}

fn build_context_field(
    graph: &BTreeMap<Digest, Vec<Arc>>,
    contexts: &[Digest],
    config: CognitiveConfig,
) -> ContextField {
    let mut field = ContextField::new();
    let mut active = Vec::new();
    for context in contexts {
        field.insert(*context, 1_000);
        active.push((*context, 1_000_u16));
    }
    for _ in 0..config.context_field_depth {
        let mut candidates: ContextField = ContextField::new();
        for (from, activation) in &active {
            let Some(arcs) = graph.get(from) else {
                continue;
            };
            let source_degree = arcs.len().max(1) as u128;
            for arc in arcs {
                let target_degree = graph.get(&arc.to).map_or(1, Vec::len).max(1) as u128;
                let target_penalty = target_degree.div_ceil(2);
                let transmitted = ((*activation as u128)
                    .saturating_mul(config.context_field_propagation_per_mille as u128)
                    / PER_MILLE
                    / source_degree
                    / target_penalty)
                    .min(1_000) as u16;
                if transmitted == 0 {
                    continue;
                }
                let candidate = candidates.entry(arc.to).or_default();
                *candidate = (*candidate).max(transmitted);
            }
        }
        let mut next: Vec<(Digest, u16)> = candidates.into_iter().collect();
        next.sort_by(|(left_id, left), (right_id, right)| {
            right.cmp(left).then_with(|| left_id.cmp(right_id))
        });
        next.truncate(config.context_field_capacity);
        active.clear();
        for (identity, activation) in next {
            let prior = field.get(&identity).copied().unwrap_or_default();
            if activation > prior {
                field.insert(identity, activation);
                active.push((identity, activation));
            }
        }
        if active.is_empty() {
            break;
        }
    }
    if field.len() > config.context_field_capacity {
        let mut bounded: Vec<(Digest, u16)> = field.into_iter().collect();
        bounded.sort_by(|(left_id, left), (right_id, right)| {
            right.cmp(left).then_with(|| left_id.cmp(right_id))
        });
        bounded.truncate(config.context_field_capacity);
        return bounded.into_iter().collect();
    }
    field
}

fn field_compatibility(left: &ContextField, right: &ContextField) -> u16 {
    let identities = left
        .keys()
        .chain(right.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    let mut intersection = 0_u128;
    let mut union = 0_u128;
    for identity in identities {
        let left_weight = left.get(&identity).copied().unwrap_or_default() as u128;
        let right_weight = right.get(&identity).copied().unwrap_or_default() as u128;
        intersection += left_weight.min(right_weight);
        union += left_weight.max(right_weight);
    }
    if union == 0 {
        return 0;
    }
    (intersection * PER_MILLE / union).min(1_000) as u16
}

fn merge_context_field(target: &mut ContextField, incoming: &ContextField, observations: u64) {
    let identities = target
        .keys()
        .chain(incoming.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    let denominator = observations.saturating_add(1) as u128;
    for identity in identities {
        let prior = target.get(&identity).copied().unwrap_or_default() as u128;
        let next = incoming.get(&identity).copied().unwrap_or_default() as u128;
        let merged = (prior.saturating_mul(observations as u128) + next) / denominator;
        if merged == 0 {
            target.remove(&identity);
        } else {
            target.insert(identity, merged.min(1_000) as u16);
        }
    }
}

fn build_graph(db: &AtomDb) -> BTreeMap<Digest, Vec<Arc>> {
    let mut graph: BTreeMap<Digest, Vec<Arc>> = BTreeMap::new();
    for (identity, bond) in db.all_bonds() {
        let members = [bond.source, bond.relation, bond.target];
        for from_index in 0..members.len() {
            for to_index in 0..members.len() {
                if from_index != to_index {
                    graph.entry(members[from_index]).or_default().push(Arc {
                        bond: identity,
                        to: members[to_index],
                    });
                }
            }
        }
    }
    for arcs in graph.values_mut() {
        arcs.sort_by_key(|arc| (arc.bond, arc.to));
        arcs.dedup_by_key(|arc| (arc.bond, arc.to));
    }
    graph
}

fn sort_active(active: &mut [(Digest, u64)]) {
    active.sort_by(|(left_id, left), (right_id, right)| {
        right.cmp(left).then_with(|| left_id.cmp(right_id))
    });
}

fn reconstruct_thread(target: Digest, traces: &BTreeMap<Digest, Trace>) -> Option<RecallThread> {
    let mut atoms = vec![target];
    let mut bonds = Vec::new();
    let mut current = target;
    let mut seen = BTreeSet::new();
    while let Some((from, bond)) = traces.get(&current)?.predecessor {
        if !seen.insert(current) {
            return None;
        }
        bonds.push(bond);
        atoms.push(from);
        current = from;
    }
    atoms.reverse();
    bonds.reverse();
    Some(RecallThread { atoms, bonds })
}

fn explain(
    cue: Digest,
    contexts: &[Digest],
    target: Option<Digest>,
    target_found: bool,
    thread: Option<&RecallThread>,
    metrics: (usize, usize, usize),
) -> String {
    let (explored_bonds, working_set_peak, promoted_hops) = metrics;
    let outcome = match target {
        Some(identity) if target_found => format!("target {identity} entered working memory"),
        Some(identity) => format!("target {identity} did not enter working memory"),
        None => "recall completed without supervised feedback".into(),
    };
    let path = thread.map_or(0, |thread| thread.bonds.len());
    format!(
        "cue={cue}; contexts={}; {outcome}; thread_hops={path}; explored_bonds={explored_bonds}; working_set_peak={working_set_peak}; promoted_hops={promoted_hops}",
        contexts.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Bond;
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_file(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "atom-db-cognition-{name}-{}-{nonce}.atoms",
            std::process::id()
        ))
    }

    fn atom(db: &mut AtomDb, text: &str) -> Digest {
        db.put_atom(text.as_bytes()).unwrap()
    }

    fn bond(db: &mut AtomDb, source: Digest, relation: Digest, target: Digest) -> Digest {
        db.put_bond(Bond {
            source,
            relation,
            target,
        })
        .unwrap()
    }

    #[test]
    fn recall_forms_an_explainable_thread() {
        let path = temp_file("thread");
        let mut db = AtomDb::open(&path).unwrap();
        let earth = atom(&mut db, "Earth");
        let orbits = atom(&mut db, "orbits");
        let sun = atom(&mut db, "Sun");
        let relation = bond(&mut db, earth, orbits, sun);
        let mut mind = CognitiveEngine::new(CognitiveConfig::default()).unwrap();
        let report = mind.recall(&db, earth, &[], Some(sun)).unwrap();
        assert!(report.target_found);
        assert_eq!(report.thread.unwrap().bonds, vec![relation]);
        assert!(report.explanation.contains("entered working memory"));
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn context_changes_attention_without_changing_truth() {
        let path = temp_file("context");
        let mut db = AtomDb::open(&path).unwrap();
        let bank = atom(&mut db, "bank");
        let associated = atom(&mut db, "associated-with");
        let money = atom(&mut db, "money");
        let river = atom(&mut db, "river");
        let finance = atom(&mut db, "finance-context");
        let nature = atom(&mut db, "nature-context");
        bond(&mut db, bank, associated, money);
        bond(&mut db, bank, associated, river);
        bond(&mut db, finance, associated, money);
        bond(&mut db, nature, associated, river);
        let mut mind = CognitiveEngine::new(CognitiveConfig::default()).unwrap();
        let financial = mind.recall(&db, bank, &[finance], None).unwrap();
        let natural = mind.recall(&db, bank, &[nature], None).unwrap();
        assert!(activation(&financial, money) > activation(&financial, river));
        assert!(activation(&natural, river) > activation(&natural, money));
        assert_eq!(db.stats().unwrap().facts, 10);
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn feedback_reinforces_success_and_inhibits_failure() {
        let path = temp_file("feedback");
        let mut db = AtomDb::open(&path).unwrap();
        let cue = atom(&mut db, "cue");
        let relation = atom(&mut db, "relation");
        let found = atom(&mut db, "found");
        let absent = atom(&mut db, "unconnected");
        let edge = bond(&mut db, cue, relation, found);
        let mut success = CognitiveEngine::new(CognitiveConfig::default()).unwrap();
        success.recall(&db, cue, &[], Some(found)).unwrap();
        assert!(success.bond_conductance(edge) > BASE_CONDUCTANCE);
        let mut failure = CognitiveEngine::new(CognitiveConfig::default()).unwrap();
        failure.recall(&db, cue, &[], Some(absent)).unwrap();
        assert!(failure.bond_conductance(edge) < BASE_CONDUCTANCE);
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn explicit_negative_feedback_inhibits_a_reached_thread() {
        let path = temp_file("negative-feedback");
        let mut db = AtomDb::open(&path).unwrap();
        let cue = atom(&mut db, "cue");
        let relation = atom(&mut db, "relation");
        let target = atom(&mut db, "target");
        let edge = bond(&mut db, cue, relation, target);
        let mut mind = CognitiveEngine::new(CognitiveConfig::default()).unwrap();

        let report = mind
            .recall_with_feedback(&db, cue, &[], target, false)
            .unwrap();

        assert!(
            report.target_found,
            "the judged target must be explainably reached"
        );
        assert!(mind.bond_conductance(edge) < BASE_CONDUCTANCE);
        assert_eq!(mind.observations(), 1);
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn context_gating_preserves_opposing_context_after_learning() {
        let path = temp_file("context-gating");
        let mut db = AtomDb::open(&path).unwrap();
        let bank = atom(&mut db, "bank");
        let associated = atom(&mut db, "associated-with");
        let money = atom(&mut db, "money");
        let river = atom(&mut db, "river");
        let finance = atom(&mut db, "finance-context");
        let nature = atom(&mut db, "nature-context");
        let bank_money = bond(&mut db, bank, associated, money);
        let bank_river = bond(&mut db, bank, associated, river);
        bond(&mut db, finance, associated, money);
        bond(&mut db, nature, associated, river);
        let mut mind = CognitiveEngine::new(CognitiveConfig::default()).unwrap();
        for _ in 0..8 {
            mind.recall(&db, bank, &[finance], Some(money)).unwrap();
        }
        let financial = mind.recall(&db, bank, &[finance], None).unwrap();
        let natural = mind.recall(&db, bank, &[nature], None).unwrap();
        assert!(activation(&financial, money) > activation(&financial, river));
        assert!(activation(&natural, river) > activation(&natural, money));
        assert!(mind.bond_conductance_in(&[finance], bank_money) > BASE_CONDUCTANCE);
        assert_eq!(
            mind.bond_conductance_in(&[nature], bank_money),
            BASE_CONDUCTANCE
        );
        assert_eq!(
            mind.bond_conductance_in(&[nature], bank_river),
            BASE_CONDUCTANCE
        );
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn global_learning_reproduces_cross_context_habit() {
        let path = temp_file("global-habit");
        let mut db = AtomDb::open(&path).unwrap();
        let bank = atom(&mut db, "bank");
        let associated = atom(&mut db, "associated-with");
        let money = atom(&mut db, "money");
        let river = atom(&mut db, "river");
        let finance = atom(&mut db, "finance-context");
        let nature = atom(&mut db, "nature-context");
        bond(&mut db, bank, associated, money);
        bond(&mut db, bank, associated, river);
        bond(&mut db, finance, associated, money);
        bond(&mut db, nature, associated, river);
        let config = CognitiveConfig {
            learning_scope: LearningScope::Global,
            ..CognitiveConfig::default()
        };
        let mut mind = CognitiveEngine::new(config).unwrap();
        for _ in 0..8 {
            mind.recall(&db, bank, &[finance], Some(money)).unwrap();
        }
        let natural = mind.recall(&db, bank, &[nature], None).unwrap();
        assert!(activation(&natural, money) > activation(&natural, river));
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn guarded_relay_transfers_to_related_context_not_opposing_context() {
        let path = temp_file("guarded-relay");
        let mut db = AtomDb::open(&path).unwrap();
        let bank = atom(&mut db, "bank");
        let associated = atom(&mut db, "associated-with");
        let money = atom(&mut db, "money");
        let river = atom(&mut db, "river");
        let finance = atom(&mut db, "finance-context");
        let banking = atom(&mut db, "banking-context");
        let nature = atom(&mut db, "nature-context");
        let financial = atom(&mut db, "financially-related");
        let natural = atom(&mut db, "naturally-related");
        let bank_money = bond(&mut db, bank, associated, money);
        bond(&mut db, bank, associated, river);
        bond(&mut db, finance, associated, money);
        bond(&mut db, nature, associated, river);
        bond(&mut db, finance, financial, banking);
        bond(&mut db, nature, natural, river);
        let config = CognitiveConfig {
            learning_scope: LearningScope::GuardedRelay,
            ..CognitiveConfig::default()
        };
        let mut mind = CognitiveEngine::new(config).unwrap();
        for _ in 0..8 {
            mind.recall(&db, bank, &[finance], Some(money)).unwrap();
        }
        let finance_conductance = mind.bond_conductance_for(&db, &[finance], bank_money);
        let banking_conductance = mind.bond_conductance_for(&db, &[banking], bank_money);
        let nature_conductance = mind.bond_conductance_for(&db, &[nature], bank_money);
        let related_compatibility = mind.context_compatibility(&db, &[finance], &[banking]);
        let opposing_compatibility = mind.context_compatibility(&db, &[finance], &[nature]);
        let related_recall = mind.recall(&db, bank, &[banking], None).unwrap();
        let opposing_recall = mind.recall(&db, bank, &[nature], None).unwrap();
        assert!(related_compatibility >= mind.config().relay_guard_per_mille);
        assert!(opposing_compatibility < mind.config().relay_guard_per_mille);
        assert!(finance_conductance > banking_conductance);
        assert!(banking_conductance > BASE_CONDUCTANCE);
        assert_eq!(nature_conductance, BASE_CONDUCTANCE);
        assert!(activation(&related_recall, money) > activation(&related_recall, river));
        assert!(activation(&opposing_recall, river) > activation(&opposing_recall, money));
        assert_eq!(mind.context_trace_count(), 1);
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn guarded_relay_enforces_finite_trace_capacity() {
        let path = temp_file("trace-capacity");
        let mut db = AtomDb::open(&path).unwrap();
        let cue = atom(&mut db, "cue");
        let relation = atom(&mut db, "relation");
        let target = atom(&mut db, "target");
        let context_a = atom(&mut db, "context-a");
        let context_b = atom(&mut db, "context-b");
        let context_c = atom(&mut db, "context-c");
        bond(&mut db, cue, relation, target);
        let config = CognitiveConfig {
            learning_scope: LearningScope::GuardedRelay,
            context_trace_capacity: 2,
            context_merge_per_mille: 1_000,
            ..CognitiveConfig::default()
        };
        let mut mind = CognitiveEngine::new(config).unwrap();
        for context in [context_a, context_b, context_c] {
            mind.recall(&db, cue, &[context], Some(target)).unwrap();
        }
        assert_eq!(mind.context_trace_count(), 2);
        assert_eq!(mind.evicted_context_traces(), 1);
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn context_identity_is_order_independent_and_duplicate_free() {
        let path = temp_file("context-set");
        let mut db = AtomDb::open(&path).unwrap();
        let cue = atom(&mut db, "cue");
        let relation = atom(&mut db, "relation");
        let target = atom(&mut db, "target");
        let context_a = atom(&mut db, "context-a");
        let context_b = atom(&mut db, "context-b");
        let edge = bond(&mut db, cue, relation, target);
        let mut first = CognitiveEngine::new(CognitiveConfig::default()).unwrap();
        let first_report = first
            .recall(&db, cue, &[context_a, context_b, context_a], Some(target))
            .unwrap();
        let mut second = CognitiveEngine::new(CognitiveConfig::default()).unwrap();
        let second_report = second
            .recall(&db, cue, &[context_b, context_a], Some(target))
            .unwrap();
        assert_eq!(first_report.contexts, second_report.contexts);
        assert_eq!(first_report.ranked, second_report.ranked);
        assert_eq!(
            first.bond_memory_in(&[context_b, context_a], edge),
            BondMemory {
                attempts: 1,
                successes: 1
            }
        );
        drop(db);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn working_memory_is_bounded_under_fanout() {
        let path = temp_file("capacity");
        let mut db = AtomDb::open(&path).unwrap();
        let cue = atom(&mut db, "cue");
        let relation = atom(&mut db, "relation");
        for index in 0..32 {
            let target = atom(&mut db, &format!("target-{index}"));
            bond(&mut db, cue, relation, target);
        }
        let config = CognitiveConfig {
            working_memory_capacity: 4,
            ..CognitiveConfig::default()
        };
        let mut mind = CognitiveEngine::new(config).unwrap();
        let report = mind.recall(&db, cue, &[], None).unwrap();
        assert!(report.working_set_peak <= 4);
        drop(db);
        fs::remove_file(path).unwrap();
    }

    fn activation(report: &RecallReport, identity: Digest) -> u64 {
        report
            .ranked
            .iter()
            .find(|atom| atom.identity == identity)
            .unwrap()
            .activation
    }
}
