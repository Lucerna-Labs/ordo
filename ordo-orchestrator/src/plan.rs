//! Goal decomposition (Stage 3) — the "main agent splits the task".
//!
//! Turns a single goal into a DAG of subtasks for the parallel dispatcher
//! (Stage 2). The LLM decomposition CALL lives in the production
//! [`GoalPlanner`] impl (Stage 5 glue, over the assistant's model/credential
//! infrastructure); the parsing + DAG construction + deterministic fallback
//! here are pure and unit-tested without a model.
//!
//! Design notes:
//! - The plan is orchestrator-owned (it wraps the dispatcher's [`Subtask`],
//!   which carries `mode` + lane narrowing) rather than reusing
//!   `ordo-tasks::Task`, which has no typed mode/lane fields.
//! - Routing is by MODE: the model assigns each subtask a mode (the
//!   runtime's specialist workspaces). Agent-profile routing
//!   (`ordo-agents`) is intentionally not used here — modes, not agent
//!   profiles, are what a subagent actually runs in.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use uuid::Uuid;

use crate::dispatch::Subtask;

/// One node in a decomposed goal: the work plus its DAG dependencies.
#[derive(Debug, Clone)]
pub struct PlannedTask {
    /// The dispatchable unit (id, goal, mode, optional lane narrowing).
    pub subtask: Subtask,
    /// Ids of sibling subtasks that must complete before this one is ready.
    pub deps: Vec<Uuid>,
}

/// A decomposed goal: a DAG of subtasks. The driver loop (Stage 5) walks
/// it round by round via [`PlannedGoal::ready`].
#[derive(Debug, Clone)]
pub struct PlannedGoal {
    pub goal: String,
    pub tasks: Vec<PlannedTask>,
}

impl PlannedGoal {
    /// A single-task plan — the deterministic fallback used when
    /// decomposition fails or the goal is atomic. Degrades to exactly
    /// today's behaviour (one subagent runs the whole goal).
    pub fn single(goal: impl Into<String>, mode: Option<String>) -> Self {
        let goal = goal.into();
        let subtask = Subtask::new(goal.clone(), mode);
        Self {
            goal,
            tasks: vec![PlannedTask {
                subtask,
                deps: Vec::new(),
            }],
        }
    }

    /// Subtasks that are ready to dispatch given the set of already
    /// `completed` subtask ids: not yet completed, and every dependency
    /// completed. (The loop tracks in-flight tasks separately so it does
    /// not re-dispatch one already running.)
    pub fn ready(&self, completed: &HashSet<Uuid>) -> Vec<&Subtask> {
        self.tasks
            .iter()
            .filter(|t| !completed.contains(&t.subtask.id))
            .filter(|t| t.deps.iter().all(|dep| completed.contains(dep)))
            .map(|t| &t.subtask)
            .collect()
    }

    /// True once every subtask id is in `completed`.
    pub fn is_complete(&self, completed: &HashSet<Uuid>) -> bool {
        self.tasks.iter().all(|t| completed.contains(&t.subtask.id))
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }
}

/// Decomposes a goal into a [`PlannedGoal`]. The production impl runs an
/// LLM decomposition turn ([`planning_prompt`] + [`parse_plan`]); the
/// orchestrator depends only on this trait so the driver loop is testable
/// with a stub planner.
#[async_trait::async_trait]
pub trait GoalPlanner: Send + Sync {
    async fn plan(&self, goal: &str) -> PlannedGoal;
}

/// Build the decomposition prompt handed to the model. Asks for STRICT
/// JSON; `modes` advertises the runtime's available modes for routing.
pub fn planning_prompt(goal: &str, modes: &[String]) -> String {
    let modes_line = if modes.is_empty() {
        "Omit the \"mode\" field (the default mode is used).".to_string()
    } else {
        format!(
            "For each subtask set \"mode\" to one of: {}.",
            modes.join(", ")
        )
    };
    format!(
        "Decompose the goal below into the FEWEST subtasks that genuinely parallelize the \
         work, for execution by specialist subagents. {modes_line}\n\n\
         Return STRICT JSON only — no prose, no code fences — shaped exactly:\n\
         {{\"subtasks\":[{{\"id\":\"s1\",\"description\":\"...\",\"mode\":\"<mode>\",\"deps\":[]}}]}}\n\
         Rules: ids are short unique strings; \"deps\" lists the ids of subtasks that must \
         finish first; if the goal is atomic, return a single subtask.\n\n\
         GOAL:\n{goal}"
    )
}

/// Parse a model decomposition response into a [`PlannedGoal`]. Permissive
/// (strips ``` fences / leading prose). Falls back to a single-task plan on
/// ANY parse failure or an empty decomposition — so a malformed model
/// response degrades safely to today's single-subagent behaviour rather
/// than failing the goal.
pub fn parse_plan(goal: &str, raw: &str) -> PlannedGoal {
    match try_parse_plan(goal, raw) {
        Some(plan) if !plan.tasks.is_empty() => plan,
        _ => PlannedGoal::single(goal, None),
    }
}

#[derive(Deserialize)]
struct PlanResponse {
    subtasks: Vec<PlanSubtask>,
}

#[derive(Deserialize)]
struct PlanSubtask {
    id: String,
    description: String,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    deps: Vec<String>,
}

fn try_parse_plan(goal: &str, raw: &str) -> Option<PlannedGoal> {
    let json = extract_json(raw);
    let parsed: PlanResponse = serde_json::from_str(json).ok()?;
    if parsed.subtasks.is_empty() {
        return None;
    }
    // Map the model's free-form string ids to stable Uuids so deps can be
    // wired by id. Unknown dep ids are dropped (a dep on a non-existent
    // subtask simply doesn't gate).
    let mut id_map: HashMap<&str, Uuid> = HashMap::new();
    for st in &parsed.subtasks {
        id_map.entry(st.id.as_str()).or_insert_with(Uuid::new_v4);
    }
    let tasks = parsed
        .subtasks
        .iter()
        .map(|st| {
            let id = id_map[st.id.as_str()];
            let deps = st
                .deps
                .iter()
                .filter_map(|d| id_map.get(d.as_str()).copied())
                .filter(|dep| *dep != id) // a self-dep can't gate
                .collect();
            let mode = st
                .mode
                .as_ref()
                .map(|m| m.trim().to_string())
                .filter(|m| !m.is_empty());
            PlannedTask {
                subtask: Subtask {
                    id,
                    goal: st.description.clone(),
                    mode,
                    allowed_lanes: None,
                },
                deps,
            }
        })
        .collect();
    Some(PlannedGoal {
        goal: goal.to_string(),
        tasks,
    })
}

/// Pull the JSON object out of a model response: strips a leading ```json
/// (or plain ```) fence if present, otherwise returns the first `{ ... }`
/// span. Permissive — reasoning models often wrap output or think aloud.
fn extract_json(raw: &str) -> &str {
    let trimmed = raw.trim();
    if let Some(fence) = trimmed.find("```") {
        let after = &trimmed[fence + 3..];
        let body_start = after.find('\n').map(|i| i + 1).unwrap_or(0);
        let body = &after[body_start..];
        return match body.find("```") {
            Some(end) => body[..end].trim(),
            None => body.trim(),
        };
    }
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if end >= start {
            return &trimmed[start..=end];
        }
    }
    trimmed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(goal: &PlannedGoal) -> HashMap<&str, Uuid> {
        // Map by the subtask goal text for test assertions.
        goal.tasks
            .iter()
            .map(|t| (t.subtask.goal.as_str(), t.subtask.id))
            .collect()
    }

    #[test]
    fn parses_dag_with_dependencies_and_modes() {
        let raw = r#"{"subtasks":[
            {"id":"a","description":"research the topic","mode":"research","deps":[]},
            {"id":"b","description":"analyze findings","mode":"analysis","deps":["a"]}
        ]}"#;
        let plan = parse_plan("write a report", raw);
        assert_eq!(plan.len(), 2);
        let by_goal = ids(&plan);
        let a = by_goal["research the topic"];
        let b = by_goal["analyze findings"];

        // Modes routed from the model.
        let task_a = plan.tasks.iter().find(|t| t.subtask.id == a).unwrap();
        assert_eq!(task_a.subtask.mode.as_deref(), Some("research"));

        // 'b' depends on 'a'.
        let task_b = plan.tasks.iter().find(|t| t.subtask.id == b).unwrap();
        assert_eq!(task_b.deps, vec![a]);

        // Readiness gating walks the DAG.
        let mut done = HashSet::new();
        let ready: Vec<Uuid> = plan.ready(&done).iter().map(|s| s.id).collect();
        assert_eq!(ready, vec![a], "only 'a' is ready first");
        done.insert(a);
        let ready: Vec<Uuid> = plan.ready(&done).iter().map(|s| s.id).collect();
        assert_eq!(ready, vec![b], "'b' unlocks after 'a'");
        done.insert(b);
        assert!(plan.is_complete(&done));
    }

    #[test]
    fn strips_code_fences() {
        let raw = "Sure, here is the plan:\n```json\n{\"subtasks\":[{\"id\":\"x\",\"description\":\"do it\",\"deps\":[]}]}\n```\n";
        let plan = parse_plan("g", raw);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan.tasks[0].subtask.goal, "do it");
    }

    #[test]
    fn malformed_response_falls_back_to_single_task() {
        let plan = parse_plan("just do this", "the model rambled and emitted no json");
        assert_eq!(plan.len(), 1);
        assert_eq!(plan.tasks[0].subtask.goal, "just do this");
        assert!(plan.tasks[0].deps.is_empty());
    }

    #[test]
    fn empty_subtasks_falls_back_to_single_task() {
        let plan = parse_plan("atomic goal", r#"{"subtasks":[]}"#);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan.tasks[0].subtask.goal, "atomic goal");
    }

    #[test]
    fn unknown_and_self_deps_are_dropped() {
        let raw = r#"{"subtasks":[{"id":"a","description":"x","deps":["a","ghost"]}]}"#;
        let plan = parse_plan("g", raw);
        assert_eq!(plan.len(), 1);
        // self-dep and dep on a non-existent id are both dropped, so it's ready immediately.
        assert!(plan.tasks[0].deps.is_empty());
        assert_eq!(plan.ready(&HashSet::new()).len(), 1);
    }

    #[test]
    fn single_task_plan_has_no_deps_and_runs_alone() {
        let plan = PlannedGoal::single("solo", Some("general".into()));
        assert_eq!(plan.len(), 1);
        assert_eq!(plan.tasks[0].subtask.mode.as_deref(), Some("general"));
        assert_eq!(plan.ready(&HashSet::new()).len(), 1);
    }
}
