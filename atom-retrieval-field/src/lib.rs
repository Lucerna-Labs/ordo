//! Bounded multi-cue activation without storage or model dependencies.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

pub type NodeId = usize;
pub type EdgeId = usize;
const PER_MILLE: u128 = 1_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Arc {
    pub from: NodeId,
    pub to: NodeId,
    pub edge: EdgeId,
    pub conductance_per_mille: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cue {
    pub node: NodeId,
    pub activation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FieldConfig {
    pub max_depth: usize,
    pub frontier_capacity: usize,
    pub node_budget: usize,
    pub propagation_per_mille: u16,
    pub minimum_activation: u64,
    pub highway_threshold_per_mille: u16,
}

impl Default for FieldConfig {
    fn default() -> Self {
        Self {
            max_depth: 4,
            frontier_capacity: 128,
            node_budget: 4_096,
            propagation_per_mille: 700,
            minimum_activation: 1,
            highway_threshold_per_mille: 1_100,
        }
    }
}

impl FieldConfig {
    pub fn validate(self) -> Result<Self, String> {
        if self.max_depth == 0 || self.max_depth > 32 {
            return Err("retrieval depth must be between 1 and 32".into());
        }
        if self.frontier_capacity == 0 || self.frontier_capacity > 65_536 {
            return Err("frontier capacity must be between 1 and 65536".into());
        }
        if self.node_budget < self.frontier_capacity || self.node_budget > 10_000_000 {
            return Err("node budget must cover the frontier and not exceed 10000000".into());
        }
        if self.propagation_per_mille == 0 || self.propagation_per_mille > 1_000 {
            return Err("propagation must be between 1 and 1000 per mille".into());
        }
        if self.highway_threshold_per_mille > 4_000 {
            return Err("highway threshold cannot exceed 4000 per mille".into());
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Thread {
    pub cue_index: usize,
    pub nodes: Vec<NodeId>,
    pub edges: Vec<EdgeId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hit {
    pub node: NodeId,
    pub activation: u64,
    pub depth: usize,
    pub incoming_signals: usize,
    pub supporting_cues: Vec<usize>,
    pub threads: Vec<Thread>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FieldReport {
    pub hits: Vec<Hit>,
    pub rounds: usize,
    pub explored_edges: usize,
    pub working_set_peak: usize,
    pub promoted_edges: usize,
    pub stabilized: bool,
    pub budget_exhausted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetrievalCue<I> {
    pub term: String,
    pub identity: I,
    pub known: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceThread<I> {
    pub cue: String,
    pub atoms: Vec<I>,
    pub bonds: Vec<I>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Evidence<I> {
    pub identity: I,
    pub text: String,
    pub sources: Vec<String>,
    pub activation: u64,
    pub depth: usize,
    pub incoming_signals: usize,
    pub supporting_cues: Vec<String>,
    pub threads: Vec<EvidenceThread<I>>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextPacket<I> {
    pub query: String,
    pub snapshot_sequence: u64,
    pub answerable: bool,
    pub insufficient_evidence: bool,
    pub cues: Vec<RetrievalCue<I>>,
    pub evidence: Vec<Evidence<I>>,
    pub rounds: usize,
    pub explored_edges: usize,
    pub working_set_peak: usize,
    pub promoted_edges: usize,
    pub stabilized: bool,
    pub budget_exhausted: bool,
    pub context_bytes: usize,
}

impl<I: ToString> ContextPacket<I> {
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        let _ = write!(
            out,
            "{{\"schema\":1,\"query\":{},\"snapshot_sequence\":{},\"answerable\":{},\"insufficient_evidence\":{},\"cues\":[",
            json(&self.query),
            self.snapshot_sequence,
            self.answerable,
            self.insufficient_evidence
        );
        for (index, cue) in self.cues.iter().enumerate() {
            comma(&mut out, index);
            let _ = write!(
                out,
                "{{\"term\":{},\"identity\":{},\"known\":{}}}",
                json(&cue.term),
                json(&cue.identity.to_string()),
                cue.known
            );
        }
        out.push_str("],\"evidence\":[");
        for (index, evidence) in self.evidence.iter().enumerate() {
            comma(&mut out, index);
            let _ = write!(
                out,
                "{{\"identity\":{},\"text\":{},\"sources\":{},\"activation\":{},\"depth\":{},\"incoming_signals\":{},\"supporting_cues\":{},\"truncated\":{},\"threads\":[",
                json(&evidence.identity.to_string()),
                json(&evidence.text),
                strings(&evidence.sources),
                evidence.activation,
                evidence.depth,
                evidence.incoming_signals,
                strings(&evidence.supporting_cues),
                evidence.truncated
            );
            for (thread_index, thread) in evidence.threads.iter().enumerate() {
                comma(&mut out, thread_index);
                let atoms = thread
                    .atoms
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                let bonds = thread
                    .bonds
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                let _ = write!(
                    out,
                    "{{\"cue\":{},\"atoms\":{},\"bonds\":{}}}",
                    json(&thread.cue),
                    strings(&atoms),
                    strings(&bonds)
                );
            }
            out.push_str("]}");
        }
        let _ = write!(
            out,
            "],\"field\":{{\"rounds\":{},\"explored_bonds\":{},\"working_set_peak\":{},\"promoted_bonds\":{},\"stabilized\":{},\"budget_exhausted\":{}}},\"context_bytes\":{}}}",
            self.rounds,
            self.explored_edges,
            self.working_set_peak,
            self.promoted_edges,
            self.stabilized,
            self.budget_exhausted,
            self.context_bytes
        );
        out
    }
}

fn comma(out: &mut String, index: usize) {
    if index > 0 {
        out.push(',');
    }
}

fn strings(values: &[String]) -> String {
    let mut out = String::from("[");
    for (index, value) in values.iter().enumerate() {
        comma(&mut out, index);
        out.push_str(&json(value));
    }
    out.push(']');
    out
}

fn json(value: &str) -> String {
    let mut out = String::from("\"");
    for character in value.chars() {
        match character {
            '\"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if control <= '\u{1f}' => {
                let _ = write!(out, "\\u{:04x}", control as u32);
            }
            other => out.push(other),
        }
    }
    out.push('\"');
    out
}

#[derive(Clone, Copy, Debug)]
struct Signal {
    activation: u64,
    strongest_signal: u64,
    predecessor: Option<(NodeId, EdgeId)>,
}

#[derive(Clone, Debug)]
struct State {
    depth: usize,
    incoming_signals: usize,
    signals: BTreeMap<usize, Signal>,
}

#[derive(Clone, Debug, Default)]
struct Candidate {
    incoming_signals: usize,
    signals: BTreeMap<usize, Signal>,
}

pub fn activate(arcs: &[Arc], cues: &[Cue], config: FieldConfig) -> Result<FieldReport, String> {
    let config = config.validate()?;
    if cues.is_empty() {
        return Ok(FieldReport {
            stabilized: true,
            ..FieldReport::default()
        });
    }
    if cues.len() > 256 || cues.len() > config.node_budget {
        return Err("retrieval cues exceed the bounded field capacity".into());
    }

    let mut adjacency: BTreeMap<NodeId, Vec<Arc>> = BTreeMap::new();
    for arc in arcs {
        if arc.conductance_per_mille == 0 || arc.conductance_per_mille > 4_000 {
            return Err("arc conductance must be between 1 and 4000 per mille".into());
        }
        adjacency.entry(arc.from).or_default().push(*arc);
    }
    for outgoing in adjacency.values_mut() {
        outgoing.sort_by_key(|arc| (arc.edge, arc.to));
        outgoing.dedup_by_key(|arc| (arc.edge, arc.to));
    }

    let mut states: BTreeMap<NodeId, State> = BTreeMap::new();
    for (cue_index, cue) in cues.iter().enumerate() {
        if cue.activation == 0 {
            continue;
        }
        states
            .entry(cue.node)
            .or_insert_with(|| State {
                depth: 0,
                incoming_signals: 1,
                signals: BTreeMap::new(),
            })
            .signals
            .insert(
                cue_index,
                Signal {
                    activation: cue.activation,
                    strongest_signal: cue.activation,
                    predecessor: None,
                },
            );
    }
    let mut active = ranked_states(states.iter().map(|(node, state)| (*node, state)));
    active.truncate(config.frontier_capacity);
    let mut expanded_signals = BTreeSet::new();
    let mut explored_edges = BTreeSet::new();
    let mut promoted_edges = BTreeSet::new();
    let mut report = FieldReport {
        working_set_peak: active.len(),
        ..FieldReport::default()
    };

    for depth in 1..=config.max_depth {
        if active.is_empty() {
            report.stabilized = true;
            break;
        }
        report.rounds = depth;
        let mut candidates: BTreeMap<NodeId, Candidate> = BTreeMap::new();
        for from in active.drain(..) {
            let Some(state) = states.get(&from) else {
                continue;
            };
            let pending = state
                .signals
                .iter()
                .filter(|(cue, _)| !expanded_signals.contains(&(from, **cue)))
                .map(|(cue, signal)| (*cue, *signal))
                .collect::<Vec<_>>();
            if pending.is_empty() {
                continue;
            }
            for (cue, _) in &pending {
                expanded_signals.insert((from, *cue));
            }
            let Some(outgoing) = adjacency.get(&from) else {
                continue;
            };
            let degree = outgoing.len().max(1) as u128;
            for arc in outgoing {
                explored_edges.insert(arc.edge);
                if arc.conductance_per_mille >= config.highway_threshold_per_mille {
                    promoted_edges.insert(arc.edge);
                }
                let mut arrivals = Vec::new();
                for (cue_index, signal) in &pending {
                    if expanded_signals.contains(&(arc.to, *cue_index)) {
                        continue;
                    }
                    let transmitted = (signal.activation as u128)
                        .saturating_mul(config.propagation_per_mille as u128)
                        .saturating_mul(arc.conductance_per_mille as u128)
                        / PER_MILLE
                        / PER_MILLE
                        / degree;
                    let transmitted = transmitted.min(u64::MAX as u128) as u64;
                    if transmitted >= config.minimum_activation {
                        arrivals.push((*cue_index, transmitted));
                    }
                }
                if arrivals.is_empty() {
                    continue;
                }
                let candidate = candidates.entry(arc.to).or_default();
                candidate.incoming_signals = candidate.incoming_signals.saturating_add(1);
                for (cue_index, transmitted) in arrivals {
                    let next = candidate.signals.entry(cue_index).or_insert(Signal {
                        activation: 0,
                        strongest_signal: 0,
                        predecessor: None,
                    });
                    next.activation = next.activation.saturating_add(transmitted);
                    if transmitted > next.strongest_signal {
                        next.strongest_signal = transmitted;
                        next.predecessor = Some((from, arc.edge));
                    }
                }
            }
        }

        candidates.retain(|_, candidate| !candidate.signals.is_empty());
        let ranked = ranked_candidates(&candidates);
        let mut next = Vec::new();
        let mut known_nodes = states.len();
        for node in ranked {
            if next.len() == config.frontier_capacity {
                report.budget_exhausted = true;
                break;
            }
            if !states.contains_key(&node) {
                if known_nodes == config.node_budget {
                    report.budget_exhausted = true;
                    continue;
                }
                known_nodes += 1;
            }
            next.push(node);
        }
        for node in &next {
            let candidate = candidates.remove(node).expect("ranked candidate exists");
            if let Some(state) = states.get_mut(node) {
                state.depth = state.depth.min(depth);
                state.incoming_signals = state
                    .incoming_signals
                    .saturating_add(candidate.incoming_signals);
                for (cue, incoming) in candidate.signals {
                    let signal = state.signals.entry(cue).or_insert(Signal {
                        activation: 0,
                        strongest_signal: 0,
                        predecessor: None,
                    });
                    signal.activation = signal.activation.saturating_add(incoming.activation);
                    if incoming.strongest_signal > signal.strongest_signal {
                        signal.strongest_signal = incoming.strongest_signal;
                        signal.predecessor = incoming.predecessor;
                    }
                }
            } else {
                states.insert(
                    *node,
                    State {
                        depth,
                        incoming_signals: candidate.incoming_signals,
                        signals: candidate.signals,
                    },
                );
            }
        }
        active = next;
        report.working_set_peak = report.working_set_peak.max(active.len());
        if active.is_empty() {
            report.stabilized = true;
            break;
        }
    }

    if !active.is_empty() && report.rounds == config.max_depth {
        report.budget_exhausted = true;
    }
    report.explored_edges = explored_edges.len();
    report.promoted_edges = promoted_edges.len();
    report.hits = states
        .iter()
        .map(|(node, state)| Hit {
            node: *node,
            activation: total_activation(state),
            depth: state.depth,
            incoming_signals: state.incoming_signals,
            supporting_cues: state.signals.keys().copied().collect(),
            threads: state
                .signals
                .keys()
                .filter_map(|cue| reconstruct_thread(*node, *cue, cues, &states))
                .collect(),
        })
        .collect();
    report.hits.sort_by(|left, right| {
        resonance(right.activation, right.supporting_cues.len())
            .cmp(&resonance(left.activation, left.supporting_cues.len()))
            .then_with(|| right.supporting_cues.len().cmp(&left.supporting_cues.len()))
            .then_with(|| right.activation.cmp(&left.activation))
            .then_with(|| left.depth.cmp(&right.depth))
            .then_with(|| left.node.cmp(&right.node))
    });
    Ok(report)
}

fn ranked_states<'a>(states: impl Iterator<Item = (NodeId, &'a State)>) -> Vec<NodeId> {
    let mut ranked = states.collect::<Vec<_>>();
    ranked.sort_by(|(left_id, left), (right_id, right)| {
        resonance(total_activation(right), right.signals.len())
            .cmp(&resonance(total_activation(left), left.signals.len()))
            .then_with(|| right.signals.len().cmp(&left.signals.len()))
            .then_with(|| total_activation(right).cmp(&total_activation(left)))
            .then_with(|| left_id.cmp(right_id))
    });
    ranked.into_iter().map(|(node, _)| node).collect()
}

fn ranked_candidates(candidates: &BTreeMap<NodeId, Candidate>) -> Vec<NodeId> {
    let mut ranked = candidates.iter().collect::<Vec<_>>();
    ranked.sort_by(|(left_id, left), (right_id, right)| {
        resonance(candidate_activation(right), right.signals.len())
            .cmp(&resonance(candidate_activation(left), left.signals.len()))
            .then_with(|| right.signals.len().cmp(&left.signals.len()))
            .then_with(|| candidate_activation(right).cmp(&candidate_activation(left)))
            .then_with(|| left_id.cmp(right_id))
    });
    ranked.into_iter().map(|(node, _)| *node).collect()
}

fn total_activation(state: &State) -> u64 {
    state.signals.values().fold(0_u64, |total, signal| {
        total.saturating_add(signal.activation)
    })
}

fn candidate_activation(candidate: &Candidate) -> u64 {
    candidate.signals.values().fold(0_u64, |total, signal| {
        total.saturating_add(signal.activation)
    })
}

/// Independent cue agreement amplifies accumulated signal strength. This
/// keeps neither a single rare cue nor a pile of weak generic cues dominant.
fn resonance(activation: u64, supporting_cues: usize) -> u128 {
    activation as u128 * supporting_cues.max(1) as u128
}

fn reconstruct_thread(
    target: NodeId,
    cue_index: usize,
    cues: &[Cue],
    states: &BTreeMap<NodeId, State>,
) -> Option<Thread> {
    let cue = cues.get(cue_index)?.node;
    let mut nodes = vec![target];
    let mut edges = Vec::new();
    let mut current = target;
    let mut seen = BTreeSet::new();
    while current != cue {
        if !seen.insert(current) {
            return None;
        }
        let (from, edge) = states.get(&current)?.signals.get(&cue_index)?.predecessor?;
        edges.push(edge);
        nodes.push(from);
        current = from;
    }
    nodes.reverse();
    edges.reverse();
    Some(Thread {
        cue_index,
        nodes,
        edges,
    })
}

pub fn terms(text: &str, capacity: usize) -> Vec<String> {
    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    for raw in text.split(|character: char| !character.is_alphanumeric()) {
        let raw = raw.to_lowercase();
        if is_stop_word(&raw) {
            continue;
        }
        let term = canonical_term(&raw);
        if term.is_empty() || !seen.insert(term.clone()) {
            continue;
        }
        found.push(term);
        if found.len() == capacity {
            break;
        }
    }
    found
}

fn canonical_term(raw: &str) -> String {
    let mut term = raw.to_lowercase();
    if term.chars().count() < 2 {
        return String::new();
    }
    if term.len() > 5 && term.ends_with("ies") {
        term.truncate(term.len() - 3);
        term.push('y');
    } else if term.len() > 5
        && ["ches", "shes", "sses", "xes", "zes"]
            .iter()
            .any(|ending| term.ends_with(ending))
    {
        term.truncate(term.len() - 2);
    } else if term.len() > 5 && term.ends_with("ing") {
        term.truncate(term.len() - 3);
    } else if term.len() > 4 && term.ends_with("ed") {
        term.truncate(term.len() - 2);
    } else if term.len() > 3
        && term.ends_with('s')
        && !term.ends_with("ss")
        && !term.ends_with("us")
        && !term.ends_with("is")
    {
        term.pop();
    }
    term
}

fn is_stop_word(term: &str) -> bool {
    matches!(
        term,
        "a" | "after"
            | "an"
            | "and"
            | "are"
            | "as"
            | "at"
            | "be"
            | "before"
            | "by"
            | "can"
            | "could"
            | "did"
            | "do"
            | "does"
            | "for"
            | "from"
            | "how"
            | "in"
            | "is"
            | "it"
            | "of"
            | "on"
            | "or"
            | "should"
            | "that"
            | "the"
            | "this"
            | "to"
            | "was"
            | "what"
            | "when"
            | "where"
            | "which"
            | "who"
            | "why"
            | "with"
            | "would"
    )
}

pub fn passages(text: &str, max_bytes: usize) -> Result<Vec<String>, String> {
    passages_with_overlap(text, max_bytes, 0)
}

/// Segment text into bounded passage windows that share `overlap_bytes` of
/// trailing context, so an answer spanning a hard boundary survives whole
/// in the next window. Overlap is sentence-aligned when possible and the
/// tail of the previous chunk; windows stay deterministic and bounded.
pub fn passages_with_overlap(
    text: &str,
    max_bytes: usize,
    overlap_bytes: usize,
) -> Result<Vec<String>, String> {
    if max_bytes < 64 {
        return Err("passage size must be at least 64 bytes".into());
    }
    if overlap_bytes >= max_bytes / 2 {
        return Err("overlap must stay below half the passage size".into());
    }
    let mut sentences = Vec::new();
    for piece in text.split_inclusive(['.', '!', '?', '\n']) {
        let piece = piece.trim();
        if !piece.is_empty() {
            sentences.push(piece.to_string());
        }
    }
    let mut result = Vec::new();
    let mut start = 0;
    while start < sentences.len() {
        // Seed the window with the overlap tail of the previous window,
        // always leaving room for the next fresh sentence.
        let mut used = 0;
        let mut window: Vec<&str> = Vec::new();
        if start > 0 && overlap_bytes > 0 {
            let reserved = max_bytes
                .saturating_sub(sentences[start].len())
                .saturating_sub(1);
            let tail_budget = overlap_bytes.min(reserved);
            let mut tail = Vec::new();
            let mut tail_bytes = 0;
            for sentence in sentences[..start].iter().rev() {
                let extra = sentence.len() + usize::from(!tail.is_empty());
                if tail_bytes + extra > tail_budget {
                    break;
                }
                tail_bytes += extra;
                tail.push(sentence.as_str());
            }
            for sentence in tail.into_iter().rev() {
                used += sentence.len() + usize::from(used > 0);
                window.push(sentence);
            }
        }
        // Extend forward until the budget is full.
        let mut end = start;
        while end < sentences.len() {
            let piece = &sentences[end];
            let extra = piece.len() + usize::from(!window.is_empty());
            if !window.is_empty() && used + extra > max_bytes && end > start {
                break;
            }
            if piece.len() > max_bytes {
                // Oversized sentence: emit what we have and split it hard.
                if !window.is_empty() {
                    result.push(window.join(" "));
                    window = Vec::new();
                }
                let mut split = Vec::new();
                split_long_piece(piece, max_bytes, &mut split);
                for chunk in split {
                    result.push(chunk);
                }
                end += 1;
                start = end;
                break;
            }
            used += extra;
            window.push(piece);
            end += 1;
        }
        if start == sentences.len() || (end == sentences.len() && !window.is_empty()) {
            if !window.is_empty() {
                result.push(window.join(" "));
            }
            break;
        }
        if !window.is_empty() {
            result.push(window.join(" "));
        }
        start = end.max(start + 1);
    }
    Ok(result)
}

fn split_long_piece(piece: &str, max_bytes: usize, result: &mut Vec<String>) {
    let mut start = 0;
    while start < piece.len() {
        let mut end = (start + max_bytes).min(piece.len());
        while !piece.is_char_boundary(end) {
            end -= 1;
        }
        if end < piece.len()
            && let Some(space) = piece[start..end].rfind(char::is_whitespace)
            && space > 0
        {
            end = start + space;
        }
        let chunk = piece[start..end].trim();
        if !chunk.is_empty() {
            result.push(chunk.to_string());
        }
        start = end;
        while start < piece.len()
            && piece[start..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
        {
            start += piece[start..]
                .chars()
                .next()
                .expect("character exists")
                .len_utf8();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_cues_resonate_at_an_intersection() {
        let arcs = [
            Arc {
                from: 0,
                to: 2,
                edge: 0,
                conductance_per_mille: 1_400,
            },
            Arc {
                from: 1,
                to: 2,
                edge: 1,
                conductance_per_mille: 1_400,
            },
            Arc {
                from: 0,
                to: 3,
                edge: 2,
                conductance_per_mille: 1_400,
            },
        ];
        let report = activate(
            &arcs,
            &[
                Cue {
                    node: 0,
                    activation: 1_000_000,
                },
                Cue {
                    node: 1,
                    activation: 1_000_000,
                },
            ],
            FieldConfig::default(),
        )
        .unwrap();
        assert_eq!(report.hits[0].node, 2);
        assert_eq!(report.hits[0].supporting_cues, vec![0, 1]);
        assert_eq!(report.hits[0].threads.len(), 2);
    }

    #[test]
    fn later_cue_joins_an_already_discovered_intersection() {
        let arcs = [
            Arc {
                from: 0,
                to: 2,
                edge: 0,
                conductance_per_mille: 1_400,
            },
            Arc {
                from: 1,
                to: 3,
                edge: 1,
                conductance_per_mille: 1_000,
            },
            Arc {
                from: 3,
                to: 2,
                edge: 2,
                conductance_per_mille: 1_000,
            },
        ];
        let report = activate(
            &arcs,
            &[
                Cue {
                    node: 0,
                    activation: 1_000_000,
                },
                Cue {
                    node: 1,
                    activation: 1_000_000,
                },
            ],
            FieldConfig::default(),
        )
        .unwrap();
        let hit = report.hits.iter().find(|hit| hit.node == 2).unwrap();
        assert_eq!(hit.supporting_cues, vec![0, 1]);
        assert_eq!(hit.threads[1].nodes, vec![1, 3, 2]);
    }

    #[test]
    fn field_quiesces_on_cycles_without_reexpansion() {
        let arcs = [
            Arc {
                from: 0,
                to: 1,
                edge: 0,
                conductance_per_mille: 1_000,
            },
            Arc {
                from: 1,
                to: 0,
                edge: 0,
                conductance_per_mille: 1_000,
            },
        ];
        let report = activate(
            &arcs,
            &[Cue {
                node: 0,
                activation: 1_000,
            }],
            FieldConfig::default(),
        )
        .unwrap();
        assert!(report.stabilized);
        assert!(!report.budget_exhausted);
    }

    #[test]
    fn text_primitives_are_deterministic_and_bounded() {
        assert_eq!(
            terms("Writers writing, and crashes!", 8),
            vec!["writer", "writ", "crash"]
        );
        let chunks = passages(&"x".repeat(150), 64).unwrap();
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|chunk| chunk.len() <= 64));
    }

    #[test]
    fn overlapping_windows_keep_boundary_answers_whole() {
        // The answer sentence ends one valid window and must reappear with
        // the following context in the next valid window.
        let document = format!(
            "{}. The lease recovers after the crash. {}.",
            "setup context ".repeat(5).trim(),
            "trailing detail ".repeat(5).trim()
        );
        assert_eq!(
            passages_with_overlap(&document, 120, 60).unwrap_err(),
            "overlap must stay below half the passage size"
        );
        let windows = passages_with_overlap(&document, 120, 59).unwrap();
        assert!(windows.len() >= 2);
        assert!(windows.iter().all(|window| window.len() <= 120));
        // The boundary sentence appears in two consecutive windows.
        let appearances = windows
            .iter()
            .filter(|window| window.contains("The lease recovers after the crash."))
            .count();
        assert!(
            appearances >= 2,
            "the boundary sentence must survive in overlapping windows: {windows:?}"
        );
        // No information is duplicated beyond the overlap allowance.
        assert_eq!(
            passages_with_overlap(&document, 120, 0).unwrap(),
            passages(&document, 120).unwrap()
        );
    }
}
