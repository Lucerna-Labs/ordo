//! Shared helper functions used across multiple capability providers.
//!
//! These utilities handle JSON argument parsing, knowledge synthesis,
//! path normalization, and cloud lifecycle logging. They live here
//! (rather than in any single provider module) because multiple
//! providers call them.

use crate::*;
use std::path::{Component, Path, PathBuf};
use serde_json::{json, Value};

pub(crate) fn require_string(arguments: &Value, key: &str) -> Result<String, String> {
    arguments
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("missing required string field '{key}'"))
}

pub(crate) fn optional_string(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

pub(crate) fn optional_string_array(arguments: &Value, key: &str) -> Vec<String> {
    arguments
        .get(key)
        .and_then(|value| value.as_array())
        .map(|array| {
            array
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn log_cloud_lifecycle_report(label: &str, report: &ordo_cloud::LocalModelLifecycleReport) {
    if report.has_work() {
        tracing::info!(
            target: "ordo_mcp_host::cloud_lifecycle",
            label,
            unloaded = report.unloaded.len(),
            errors = report.errors.len(),
            "local model lifecycle applied"
        );
    }
}

pub(crate) fn lifecycle_error(report: &ordo_cloud::LocalModelLifecycleReport) -> Option<String> {
    if report.errors.is_empty() {
        None
    } else {
        Some(format!(
            "local model lifecycle failed before LLM call: {}",
            report.errors.join("; ")
        ))
    }
}

pub(crate) fn parse_runtime_budget_argument(arguments: &Value, key: &str) -> Result<Option<usize>, String> {
    let Some(value) = arguments.get(key) else {
        return Ok(None);
    };
    let numeric = value
        .as_u64()
        .ok_or_else(|| format!("{key} must be a positive integer"))?;
    if numeric == 0 {
        return Err(format!("{key} must be greater than zero"));
    }
    usize::try_from(numeric)
        .map(Some)
        .map_err(|_| format!("{key} is too large for this platform"))
}

pub(crate) fn parse_runtime_optional_string_argument(
    arguments: &Value,
    key: &str,
) -> Result<Option<String>, String> {
    let Some(value) = arguments.get(key) else {
        return Ok(None);
    };
    match value {
        Value::Null => Ok(Some(String::new())),
        Value::String(value) => Ok(Some(value.trim().to_string())),
        _ => Err(format!("{key} must be a string or null")),
    }
}

pub(crate) fn parse_runtime_f32_string_argument(
    arguments: &Value,
    key: &str,
) -> Result<Option<String>, String> {
    let Some(value) = arguments.get(key) else {
        return Ok(None);
    };
    let parsed = match value {
        Value::Number(number) => number
            .as_f64()
            .ok_or_else(|| format!("{key} must be a finite number"))?,
        Value::String(text) => text
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("{key} must be a finite number"))?,
        _ => return Err(format!("{key} must be a number")),
    };
    if !parsed.is_finite() || parsed < 0.0 {
        return Err(format!("{key} must be a non-negative number"));
    }
    Ok(Some(parsed.to_string()))
}

pub(crate) fn runtime_settings_json(settings: &RuntimeSettings) -> Value {
    json!({
        "profile": settings.profile,
        "rag_budget_bytes": settings.rag_budget_bytes,
        "memory_working_budget_bytes": settings.memory_working_budget_bytes,
        "memory_pinned_budget_bytes": settings.memory_pinned_budget_bytes,
        "self_heal_history_budget_bytes": settings.self_heal_history_budget_bytes,
        "self_heal_llama_cpp_binary": settings.self_heal_llama_cpp_binary,
        "self_heal_model_path": settings.self_heal_model_path,
        "self_heal_model_context_size": settings.self_heal_model_context_size,
        "self_heal_model_max_tokens": settings.self_heal_model_max_tokens,
        "self_heal_model_temperature": settings
            .self_heal_model_temperature
            .as_deref()
            .and_then(|value| value.parse::<f64>().ok()),
        "embedding_llama_cpp_binary": settings.embedding_llama_cpp_binary,
        "embedding_model_path": settings.embedding_model_path,
        "embedding_dimensions": settings.embedding_dimensions,
        "embedding_context_size": settings.embedding_context_size,
        "embedding_ollama_url": settings.embedding_ollama_url,
        "embedding_ollama_model": settings.embedding_ollama_model,
    })
}

pub(crate) fn rounded_runtime_float(value: f32) -> f64 {
    ((value as f64) * 1000.0).round() / 1000.0
}

pub(crate) fn parse_limit_argument(arguments: &Value, key: &str, default: usize) -> Result<usize, String> {
    let Some(value) = arguments.get(key) else {
        return Ok(default);
    };
    let numeric = value
        .as_u64()
        .ok_or_else(|| format!("{key} must be a positive integer"))?;
    if numeric == 0 {
        return Err(format!("{key} must be greater than zero"));
    }
    usize::try_from(numeric).map_err(|_| format!("{key} is too large for this platform"))
}

pub(crate) fn require_string_argument<'a>(
    arguments: &'a Value,
    field: &str,
) -> Result<&'a str, ToolCallResult> {
    match arguments.get(field).and_then(Value::as_str) {
        Some(value) => Ok(value),
        None => Err(ToolCallResult::Failed {
            error: format!("missing required string field '{field}'"),
        }),
    }
}

pub(crate) fn extract_read_path(goal: &str) -> Option<String> {
    let normalized_goal = goal.to_ascii_lowercase();
    let marker = "read file";
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

pub(crate) fn preview_text(contents: &str) -> String {
    let preview: String = contents.chars().take(120).collect();
    preview.replace('\r', " ").replace('\n', "\\n")
}

pub(crate) fn attach_context_to_output(result: &mut Value, arguments: &Value) {
    if let Some(object) = result.as_object_mut() {
        if let Some(hits) = arguments.get("context_hits") {
            object.insert("context_hits".to_string(), hits.clone());
        }
        if let Some(sources) = arguments.get("context_sources") {
            object.insert("context_sources".to_string(), sources.clone());
        }
    }
}

pub(crate) fn knowledge_task_from_capability(capability: &str) -> Option<KnowledgeTask> {
    KnowledgeTask::ALL
        .iter()
        .copied()
        .find(|task| task.capability() == capability)
}

pub(crate) fn knowledge_snippets_from_hits(context: &[RagHit]) -> Vec<String> {
    context
        .iter()
        .take(4)
        .map(|hit| compact_snippet(&hit.snippet, 180))
        .collect()
}

pub(crate) fn knowledge_sources_from_hits(context: &[RagHit]) -> Vec<String> {
    context
        .iter()
        .take(4)
        .map(|hit| format!("{}#{}", hit.title, hit.chunk_index))
        .collect()
}

pub(crate) fn knowledge_snippets_from_arguments(arguments: &Value) -> Vec<String> {
    arguments
        .get("snippets")
        .and_then(|value| value.as_array())
        .map(|snippets| {
            snippets
                .iter()
                .filter_map(|value| value.as_str())
                .take(4)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub(crate) fn knowledge_sources_from_arguments(arguments: &Value) -> Vec<String> {
    arguments
        .get("sources")
        .and_then(|value| value.as_array())
        .map(|sources| {
            sources
                .iter()
                .filter_map(|value| value.as_str())
                .take(4)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub(crate) fn synthesize_knowledge_result(
    task: KnowledgeTask,
    goal: &str,
    snippets: &[String],
    sources: &[String],
) -> Value {
    let snippets = knowledge_fallback_snippets(snippets);

    match task {
        KnowledgeTask::Summarize => json!({
            "goal": goal,
            "summary": knowledge_summary_text(goal, &snippets, sources),
            "snippet_count": snippets.len(),
            "source_count": sources.len(),
        }),
        KnowledgeTask::AnswerQuestion => json!({
            "goal": goal,
            "answer": knowledge_answer_text(goal, &snippets, sources),
            "snippet_count": snippets.len(),
            "source_count": sources.len(),
        }),
        KnowledgeTask::CompareSources => {
            let observations = knowledge_observations(&snippets, sources);
            let comparison = if observations.len() >= 2 {
                format!(
                    "comparison for '{}': {} versus {}",
                    goal, observations[0], observations[1]
                )
            } else {
                format!(
                    "comparison for '{}': only one context slice was available: {}",
                    goal,
                    observations
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "no retrieved context was available".to_string())
                )
            };
            json!({
                "goal": goal,
                "comparison": comparison,
                "observations": observations,
                "source_count": sources.len(),
            })
        }
        KnowledgeTask::IdentifyFollowUps => {
            let followups = knowledge_followups(&snippets, sources);
            json!({
                "goal": goal,
                "followups": followups,
                "followup_count": followups.len(),
                "source_count": sources.len(),
            })
        }
    }
}

pub(crate) fn knowledge_result_preview(task: KnowledgeTask, result: &Value) -> String {
    match task {
        KnowledgeTask::Summarize => result
            .get("summary")
            .and_then(|value| value.as_str())
            .unwrap_or("summary unavailable")
            .to_string(),
        KnowledgeTask::AnswerQuestion => result
            .get("answer")
            .and_then(|value| value.as_str())
            .unwrap_or("answer unavailable")
            .to_string(),
        KnowledgeTask::CompareSources => result
            .get("comparison")
            .and_then(|value| value.as_str())
            .unwrap_or("comparison unavailable")
            .to_string(),
        KnowledgeTask::IdentifyFollowUps => result
            .get("followups")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|value| value.as_str())
                    .collect::<Vec<_>>()
                    .join(" | ")
            })
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "follow-ups unavailable".to_string()),
    }
}

pub(crate) fn knowledge_fallback_snippets(snippets: &[String]) -> Vec<String> {
    if snippets.is_empty() {
        vec!["no retrieved context was available".to_string()]
    } else {
        snippets.to_vec()
    }
}

pub(crate) fn knowledge_summary_text(goal: &str, snippets: &[String], sources: &[String]) -> String {
    if sources.is_empty() {
        format!("summary for '{}': {}", goal, snippets.join(" | "))
    } else {
        format!(
            "summary for '{}': {} sources=[{}]",
            goal,
            snippets.join(" | "),
            sources.join(", ")
        )
    }
}

pub(crate) fn knowledge_answer_text(goal: &str, snippets: &[String], sources: &[String]) -> String {
    let answer = snippets
        .iter()
        .take(2)
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    if sources.is_empty() {
        format!("answer for '{}': {}", goal, answer)
    } else {
        format!(
            "answer for '{}': {} evidence=[{}]",
            goal,
            answer,
            sources.join(", ")
        )
    }
}

pub(crate) fn knowledge_observations(snippets: &[String], sources: &[String]) -> Vec<String> {
    snippets
        .iter()
        .enumerate()
        .map(|(index, snippet)| {
            let source = sources
                .get(index)
                .cloned()
                .unwrap_or_else(|| format!("context#{}", index + 1));
            format!("{source}: {snippet}")
        })
        .collect()
}

pub(crate) fn knowledge_followups(snippets: &[String], sources: &[String]) -> Vec<String> {
    let mut followups = Vec::new();

    for (index, snippet) in snippets.iter().enumerate() {
        let candidate = followup_candidate(snippet).unwrap_or_else(|| lead_segment(snippet));
        if candidate.is_empty() {
            continue;
        }
        let source = sources
            .get(index)
            .cloned()
            .unwrap_or_else(|| format!("context#{}", index + 1));
        followups.push(format!("{source}: {candidate}"));
        if followups.len() == 4 {
            break;
        }
    }

    if followups.is_empty() {
        followups.push(
            "Review the retrieved context and identify the next operational step.".to_string(),
        );
    }

    followups
}

pub(crate) fn followup_candidate(snippet: &str) -> Option<String> {
    snippet
        .split(['.', ';'])
        .map(str::trim)
        .find(|segment| {
            let lowered = segment.to_ascii_lowercase();
            [
                "should", "must", "need", "add", "replace", "turn", "feed", "use", "surface",
                "revisit", "expand", "improve",
            ]
            .iter()
            .any(|keyword| lowered.contains(keyword))
        })
        .map(str::to_string)
}

pub(crate) fn lead_segment(snippet: &str) -> String {
    snippet
        .split(['.', ';'])
        .next()
        .unwrap_or(snippet)
        .trim()
        .trim_start_matches("- ")
        .to_string()
}

pub(crate) fn compact_snippet(snippet: &str, max_chars: usize) -> String {
    let mut compact = snippet.trim().replace(['\r', '\n'], " ");
    if compact.chars().count() > max_chars {
        compact = compact.chars().take(max_chars).collect::<String>();
        compact.push_str("...");
    }
    compact
}

pub(crate) fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

pub(crate) fn parse_runtime_profile_argument(value: &Value) -> Result<String, String> {
    let profile = value
        .as_str()
        .ok_or_else(|| "profile must be a string".to_string())?
        .to_ascii_lowercase();
    if matches!(profile.as_str(), "minimal" | "standard" | "full") {
        Ok(profile)
    } else {
        Err(format!(
            "unsupported profile '{}'; expected minimal, standard, or full",
            profile
        ))
    }
}
