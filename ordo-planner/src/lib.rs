use ordo_protocol::{infer_knowledge_task, is_knowledge_goal, ExecutionPlan, PlanStep, RagHit};
use serde_json::json;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PlanError {
    #[error("unsupported goal '{goal}'")]
    UnsupportedGoal { goal: String },
    #[error("goal '{goal}' requires missing capability '{capability}'")]
    MissingCapability { goal: String, capability: String },
    #[error("read goal '{goal}' did not include a file path")]
    MissingReadPath { goal: String },
    #[error("write goal '{goal}' did not include a file path")]
    MissingWritePath { goal: String },
    #[error("write goal '{goal}' did not include content")]
    MissingWriteContent { goal: String },
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RuleBasedPlanner;

impl RuleBasedPlanner {
    pub fn plan(&self, goal: &str, context: &[RagHit]) -> Result<ExecutionPlan, PlanError> {
        self.plan_with_capabilities(goal, context, &[])
    }

    pub fn plan_with_capabilities(
        &self,
        goal: &str,
        context: &[RagHit],
        available_capabilities: &[String],
    ) -> Result<ExecutionPlan, PlanError> {
        let plan = self.plan_unchecked(goal, context)?;
        if available_capabilities.is_empty() {
            return Ok(plan);
        }

        for step in &plan.steps {
            if !available_capabilities
                .iter()
                .any(|capability| capability == &step.capability)
            {
                return Err(PlanError::MissingCapability {
                    goal: goal.to_string(),
                    capability: step.capability.clone(),
                });
            }
        }

        Ok(plan)
    }

    fn plan_unchecked(&self, goal: &str, context: &[RagHit]) -> Result<ExecutionPlan, PlanError> {
        if is_read_goal(goal) {
            return self.plan_read(goal, context);
        }

        if is_write_goal(goal) {
            return self.plan_write(goal, context);
        }

        if is_knowledge_goal(goal) {
            return Ok(self.plan_knowledge(goal, context));
        }

        Err(PlanError::UnsupportedGoal {
            goal: goal.to_string(),
        })
    }

    fn plan_read(&self, goal: &str, context: &[RagHit]) -> Result<ExecutionPlan, PlanError> {
        let path = extract_read_path(goal).ok_or_else(|| PlanError::MissingReadPath {
            goal: goal.to_string(),
        })?;

        let mut arguments = json!({ "path": path });
        attach_context(&mut arguments, context);
        Ok(ExecutionPlan {
            plan_id: Uuid::new_v4(),
            goal: goal.to_string(),
            steps: vec![PlanStep {
                capability: "filesystem.read_file".to_string(),
                name: "filesystem.read_file".to_string(),
                arguments,
            }],
        })
    }

    fn plan_write(&self, goal: &str, context: &[RagHit]) -> Result<ExecutionPlan, PlanError> {
        let (path, content) = extract_write_request(goal)?;
        let mut arguments = json!({
            "path": path,
            "content": content,
        });
        attach_context(&mut arguments, context);
        Ok(ExecutionPlan {
            plan_id: Uuid::new_v4(),
            goal: goal.to_string(),
            steps: vec![PlanStep {
                capability: "filesystem.write_file".to_string(),
                name: "filesystem.write_file".to_string(),
                arguments,
            }],
        })
    }

    fn plan_knowledge(&self, goal: &str, context: &[RagHit]) -> ExecutionPlan {
        let task = infer_knowledge_task(goal).expect("knowledge goal should map to a task");
        let snippets = context
            .iter()
            .take(4)
            .map(|hit| compact_snippet(&hit.snippet, 240))
            .collect::<Vec<_>>();
        let snippets = if snippets.is_empty() {
            vec!["no retrieved context was available; summarize the goal directly".to_string()]
        } else {
            snippets
        };
        let sources = context
            .iter()
            .take(4)
            .map(|hit| format!("{}#{}", hit.title, hit.chunk_index))
            .collect::<Vec<_>>();

        ExecutionPlan {
            plan_id: Uuid::new_v4(),
            goal: goal.to_string(),
            steps: vec![PlanStep {
                capability: task.capability().to_string(),
                name: task.capability().to_string(),
                arguments: json!({
                    "goal": goal,
                    "snippets": snippets,
                    "sources": sources,
                    "context_hits": context.len(),
                }),
            }],
        }
    }
}

fn is_read_goal(goal: &str) -> bool {
    let lowered = goal.to_ascii_lowercase();
    lowered.contains("read file") || lowered.starts_with("read ")
}

fn is_write_goal(goal: &str) -> bool {
    let lowered = goal.to_ascii_lowercase();
    lowered.contains("write file") || lowered.starts_with("write ")
}

fn extract_read_path(goal: &str) -> Option<String> {
    extract_path_after_marker(goal, "read file").or_else(|| extract_path_after_marker(goal, "read"))
}

fn extract_write_request(goal: &str) -> Result<(String, String), PlanError> {
    let path = extract_path_after_marker(goal, "write file")
        .or_else(|| extract_path_after_marker(goal, "write"))
        .ok_or_else(|| PlanError::MissingWritePath {
            goal: goal.to_string(),
        })?;

    let normalized = goal.to_ascii_lowercase();
    let path_marker = format!("\"{}\"", path);
    let after_path = if let Some(index) = goal.find(&path_marker) {
        &goal[index + path_marker.len()..]
    } else {
        let write_index = normalized.find("write").unwrap_or(0);
        let remainder = &goal[write_index..];
        remainder
            .split_once(&path)
            .map(|(_, tail)| tail)
            .unwrap_or("")
    };
    let after_path = after_path.trim_start();

    if let Some(content) = extract_quoted_after_prefix(after_path, "with") {
        return Ok((path, content));
    }
    if let Some(content) = extract_quoted_after_prefix(after_path, "content") {
        return Ok((path, content));
    }
    if let Some(content) = extract_quoted_segment(after_path) {
        return Ok((path, content));
    }

    Err(PlanError::MissingWriteContent {
        goal: goal.to_string(),
    })
}

fn extract_path_after_marker(goal: &str, marker: &str) -> Option<String> {
    let normalized_goal = goal.to_ascii_lowercase();
    let start = normalized_goal.find(marker)?;
    let remainder = goal[start + marker.len()..].trim_start();
    if remainder.is_empty() {
        return None;
    }

    if let Some(stripped) = remainder.strip_prefix('"') {
        let end = stripped.find('"')?;
        return Some(stripped[..end].to_string());
    }

    remainder
        .split_whitespace()
        .next()
        .map(std::string::ToString::to_string)
}

fn extract_quoted_after_prefix(input: &str, prefix: &str) -> Option<String> {
    let trimmed = input.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with(prefix) {
        return None;
    }

    extract_quoted_segment(trimmed[prefix.len()..].trim_start())
}

fn extract_quoted_segment(input: &str) -> Option<String> {
    let stripped = input.strip_prefix('"')?;
    let end = stripped.find('"')?;
    Some(stripped[..end].to_string())
}

/// Attach RAG context snippets, sources, and a count to an arbitrary step
/// argument object. No-op when context is empty. Providers that care can
/// read the fields; providers that don't just ignore them.
fn attach_context(arguments: &mut serde_json::Value, context: &[RagHit]) {
    if context.is_empty() {
        return;
    }
    let snippets = context
        .iter()
        .take(4)
        .map(|hit| compact_snippet(&hit.snippet, 240))
        .collect::<Vec<_>>();
    let sources = context
        .iter()
        .take(4)
        .map(|hit| format!("{}#{}", hit.title, hit.chunk_index))
        .collect::<Vec<_>>();
    if let Some(object) = arguments.as_object_mut() {
        object.insert("context_snippets".to_string(), json!(snippets));
        object.insert("context_sources".to_string(), json!(sources));
        object.insert("context_hits".to_string(), json!(context.len()));
    }
}

fn compact_snippet(snippet: &str, max_chars: usize) -> String {
    let mut compact = snippet.trim().replace(['\r', '\n'], " ");
    if compact.chars().count() > max_chars {
        compact = compact.chars().take(max_chars).collect::<String>();
        compact.push_str("...");
    }
    compact
}

#[cfg(test)]
mod tests {
    use ordo_protocol::RagHit;

    use super::{PlanError, RuleBasedPlanner};

    #[test]
    fn plans_read_goal_into_filesystem_step() {
        let planner = RuleBasedPlanner;
        let plan = planner
            .plan(r#"read file "Cargo.toml""#, &[])
            .expect("read plan");

        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].capability, "filesystem.read_file");
        assert_eq!(plan.steps[0].name, "filesystem.read_file");
        assert_eq!(
            plan.steps[0]
                .arguments
                .get("path")
                .and_then(|value| value.as_str()),
            Some("Cargo.toml")
        );
    }

    #[test]
    fn plans_write_goal_into_filesystem_step() {
        let planner = RuleBasedPlanner;
        let plan = planner
            .plan(r#"write file "notes.txt" with "hello world""#, &[])
            .expect("write plan");

        assert_eq!(plan.steps[0].capability, "filesystem.write_file");
        assert_eq!(
            plan.steps[0]
                .arguments
                .get("content")
                .and_then(|value| value.as_str()),
            Some("hello world")
        );
    }

    #[test]
    fn plans_knowledge_goal_using_context_snippets() {
        let planner = RuleBasedPlanner;
        let plan = planner
            .plan(
                "summarize transport design",
                &[RagHit {
                    document_id: "architecture".to_string(),
                    uri: "docs/architecture.md".to_string(),
                    title: "Architecture".to_string(),
                    chunk_index: 1,
                    score: 2.5,
                    snippet:
                        "Transport adapters allow relay fallback without rebuilding a gateway."
                            .to_string(),
                    tags: vec!["docs".to_string()],
                    collection: "main".to_string(),
                }],
            )
            .expect("knowledge plan");

        assert_eq!(plan.steps[0].capability, "knowledge.summarize");
        assert_eq!(
            plan.steps[0]
                .arguments
                .get("context_hits")
                .and_then(|value| value.as_u64()),
            Some(1)
        );
    }

    #[test]
    fn plans_question_goal_into_answer_capability() {
        let planner = RuleBasedPlanner;
        let plan = planner
            .plan("why is retrieval lazy in the standard profile?", &[])
            .expect("question plan");

        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].capability, "knowledge.answer_question");
    }

    #[test]
    fn plans_compare_goal_into_compare_capability() {
        let planner = RuleBasedPlanner;
        let plan = planner
            .plan("compare transport and retrieval design", &[])
            .expect("compare plan");

        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].capability, "knowledge.compare_sources");
    }

    #[test]
    fn plans_follow_up_goal_into_follow_up_capability() {
        let planner = RuleBasedPlanner;
        let plan = planner
            .plan("what are the next steps for transport?", &[])
            .expect("follow-up plan");

        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].capability, "knowledge.identify_followups");
    }

    #[test]
    fn plans_read_goal_with_context_attaches_snippets() {
        let planner = RuleBasedPlanner;
        let context = vec![RagHit {
            document_id: "runbook".to_string(),
            uri: "operations/runtime-runbook.md".to_string(),
            title: "Runtime Runbook".to_string(),
            chunk_index: 0,
            score: 1.2,
            snippet:
                "The runbook covers runtime diagnostics and model routing for local operations."
                    .to_string(),
            tags: vec!["operations".to_string()],
            collection: "main".to_string(),
        }];
        let plan = planner
            .plan(r#"read file "runtime-runbook.md""#, &context)
            .expect("read plan with context");
        assert_eq!(plan.steps[0].capability, "filesystem.read_file");
        assert_eq!(
            plan.steps[0]
                .arguments
                .get("context_hits")
                .and_then(|value| value.as_u64()),
            Some(1)
        );
        assert!(plan.steps[0]
            .arguments
            .get("context_snippets")
            .and_then(|value| value.as_array())
            .map(|arr| !arr.is_empty())
            .unwrap_or(false));
        assert!(plan.steps[0]
            .arguments
            .get("context_sources")
            .and_then(|value| value.as_array())
            .map(|arr| !arr.is_empty())
            .unwrap_or(false));
    }

    #[test]
    fn plans_write_goal_without_context_omits_context_fields() {
        let planner = RuleBasedPlanner;
        let plan = planner
            .plan(r#"write file "notes.txt" with "hello""#, &[])
            .expect("write plan without context");
        assert!(plan.steps[0].arguments.get("context_hits").is_none());
        assert!(plan.steps[0].arguments.get("context_snippets").is_none());
        assert!(plan.steps[0].arguments.get("context_sources").is_none());
    }

    #[test]
    fn rejects_unstructured_goal() {
        let planner = RuleBasedPlanner;
        let error = planner
            .plan("launch the beast", &[])
            .expect_err("unsupported");
        assert!(matches!(error, PlanError::UnsupportedGoal { .. }));
    }

    #[test]
    fn rejects_goal_when_required_capability_is_missing() {
        let planner = RuleBasedPlanner;
        let error = planner
            .plan_with_capabilities(
                r#"read file "Cargo.toml""#,
                &[],
                &["knowledge.summarize".to_string()],
            )
            .expect_err("missing capability");

        assert!(matches!(error, PlanError::MissingCapability { .. }));
    }
}
