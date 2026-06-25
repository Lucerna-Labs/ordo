use crate::*;
use crate::helpers::*;

pub struct KnowledgeProvider;

#[async_trait]
impl CapabilityProvider for KnowledgeProvider {
    fn name(&self) -> &str {
        "knowledge"
    }

    fn capabilities(&self) -> Vec<String> {
        KnowledgeTask::ALL
            .iter()
            .map(|task| task.capability().to_string())
            .collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        KnowledgeTask::ALL
            .iter()
            .map(|task| {
                CapabilityDescriptor::new(
                    task.capability(),
                    self.name(),
                    task.description(),
                    CapabilityTier::Optional,
                    CapabilityActivation::Lazy,
                )
            })
            .collect()
    }

    async fn handle_requirement(&self, requirement: &str) -> Option<CapabilityMatch> {
        infer_knowledge_task(requirement).map(|task| CapabilityMatch {
            capability: task.capability().to_string(),
            description: task.description().to_string(),
        })
    }

    async fn handle_run(&self, goal: &str, context: &[RagHit]) -> Option<ProviderRun> {
        let task = infer_knowledge_task(goal)?;
        let snippets = knowledge_snippets_from_hits(context);
        let sources = knowledge_sources_from_hits(context);
        let result = synthesize_knowledge_result(task, goal, &snippets, &sources);

        Some(ProviderRun {
            steps: vec![
                ProviderStep {
                    capability: task.capability().to_string(),
                    name: "knowledge.prepare_context".to_string(),
                    status: ProviderRunStatus::Completed {
                        output: format!("prepared {} retrieved context hit(s)", context.len()),
                    },
                },
                ProviderStep {
                    capability: task.capability().to_string(),
                    name: task.capability().to_string(),
                    status: ProviderRunStatus::Completed {
                        output: knowledge_result_preview(task, &result),
                    },
                },
            ],
        })
    }

    async fn handle_tool_call(
        &self,
        capability: &str,
        arguments: &Value,
    ) -> Option<ToolCallResult> {
        let task = knowledge_task_from_capability(capability)?;

        let goal = match require_string_argument(arguments, "goal") {
            Ok(value) => value,
            Err(failed) => return Some(failed),
        };
        let snippets = knowledge_snippets_from_arguments(arguments);
        let sources = knowledge_sources_from_arguments(arguments);

        Some(ToolCallResult::Completed {
            result: synthesize_knowledge_result(task, goal, &snippets, &sources),
        })
    }
}

