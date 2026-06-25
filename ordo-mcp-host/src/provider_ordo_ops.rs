use crate::*;
use crate::helpers::*;

#[derive(Debug, Default, Clone)]
pub struct OrdoOpsProvider {
    /// When set, artifact-producing capabilities (plan_initiative,
    /// package_resources, summarize_deliverables, schedule_release,
    /// request_revision) persist a markdown/JSON response
    /// of their output under `<user_files_path>/<lane>/<slug>.{md,json}`
    /// and include the path in the returned value as `artifact_path`.
    user_files_path: Option<PathBuf>,
}

impl OrdoOpsProvider {
    pub fn new() -> Self {
        Self {
            user_files_path: None,
        }
    }

    /// Enable artifact persistence. Produced plans, manifests, and
    /// orchestration notes will land inside subdirectories of `path`.
    pub fn with_user_files_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.user_files_path = Some(path.into());
        self
    }
}

const PLANNING_PLAN_INITIATIVE: &str = "planning.plan_initiative";
const PLANNING_PACKAGE_RESOURCES: &str = "planning.package_resources";
const PLANNING_SUMMARIZE_DELIVERABLES: &str = "planning.summarize_deliverables";
const ORCHESTRATION_ROUTE_REVIEW: &str = "orchestration.route_review";
const ORCHESTRATION_REQUEST_REVISION: &str = "orchestration.request_revision";
const ORCHESTRATION_ADVANCE_STAGE: &str = "orchestration.advance_stage";
const ORCHESTRATION_SCHEDULE_RELEASE: &str = "orchestration.schedule_release";

const ORDO_OPS_CAPABILITIES: &[&str] = &[
    PLANNING_PLAN_INITIATIVE,
    PLANNING_PACKAGE_RESOURCES,
    PLANNING_SUMMARIZE_DELIVERABLES,
    ORCHESTRATION_ROUTE_REVIEW,
    ORCHESTRATION_REQUEST_REVISION,
    ORCHESTRATION_ADVANCE_STAGE,
    ORCHESTRATION_SCHEDULE_RELEASE,
];

fn planning_ops_description(capability: &str) -> &'static str {
    match capability {
        PLANNING_PLAN_INITIATIVE => {
            "Produces an ordered set of initiative phases from a deliverables list."
        }
        PLANNING_PACKAGE_RESOURCES => {
            "Packages a set of planning resources into a manifest with counts by kind."
        }
        PLANNING_SUMMARIZE_DELIVERABLES => "Summarizes a deliverables list with per-type counts.",
        ORCHESTRATION_ROUTE_REVIEW => {
            "Routes a review for the given stage to the next reviewer and stage."
        }
        ORCHESTRATION_REQUEST_REVISION => {
            "Creates a revision request record with stage, reason, and optional due date."
        }
        ORCHESTRATION_ADVANCE_STAGE => {
            "Advances the orchestration from the current stage to the next valid stage."
        }
        ORCHESTRATION_SCHEDULE_RELEASE => {
            "Produces a release schedule backing out from the target date."
        }
        _ => "Ordo operations capability.",
    }
}

#[async_trait]
impl CapabilityProvider for OrdoOpsProvider {
    fn name(&self) -> &str {
        "ordo-ops"
    }

    fn capabilities(&self) -> Vec<String> {
        ORDO_OPS_CAPABILITIES
            .iter()
            .map(|capability| (*capability).to_string())
            .collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        ORDO_OPS_CAPABILITIES
            .iter()
            .map(|capability| {
                CapabilityDescriptor::new(
                    *capability,
                    self.name(),
                    planning_ops_description(capability),
                    CapabilityTier::Optional,
                    CapabilityActivation::Lazy,
                )
            })
            .collect()
    }

    async fn handle_requirement(&self, _requirement: &str) -> Option<CapabilityMatch> {
        None
    }

    async fn handle_run(&self, _goal: &str, _context: &[RagHit]) -> Option<ProviderRun> {
        None
    }

    async fn handle_tool_call(
        &self,
        capability: &str,
        arguments: &Value,
    ) -> Option<ToolCallResult> {
        let result = match capability {
            PLANNING_PLAN_INITIATIVE => plan_initiative(arguments),
            PLANNING_PACKAGE_RESOURCES => {
                package_resources(arguments, self.user_files_path.as_deref())
            }
            PLANNING_SUMMARIZE_DELIVERABLES => summarize_deliverables(arguments),
            ORCHESTRATION_ROUTE_REVIEW => route_review(arguments),
            ORCHESTRATION_REQUEST_REVISION => request_revision(arguments),
            ORCHESTRATION_ADVANCE_STAGE => advance_stage(arguments),
            ORCHESTRATION_SCHEDULE_RELEASE => schedule_release(arguments),
            _ => return None,
        };
        Some(match result {
            Ok(mut value) => {
                if let Some(root) = &self.user_files_path {
                    if let Err(err) = persist_artifact(root, capability, arguments, &mut value) {
                        tracing::warn!(
                            target: "ordo_mcp_host::planning_ops",
                            capability,
                            error = %err,
                            "failed to persist artifact"
                        );
                    }
                }
                attach_context_to_output(&mut value, arguments);
                ToolCallResult::Completed { result: value }
            }
            Err(error) => ToolCallResult::Failed { error },
        })
    }
}

fn plan_initiative(arguments: &Value) -> Result<Value, String> {
    let deliverables = optional_string_array(arguments, "deliverables");
    let phases = if deliverables.is_empty() {
        vec![
            json!({ "phase": "discovery", "deliverables": Vec::<String>::new() }),
            json!({ "phase": "production", "deliverables": Vec::<String>::new() }),
            json!({ "phase": "launch", "deliverables": Vec::<String>::new() }),
        ]
    } else {
        let third = deliverables.len().div_ceil(3);
        let (discovery, rest) = deliverables.split_at(third.min(deliverables.len()));
        let split = third.min(rest.len());
        let (production, launch) = rest.split_at(split);
        vec![
            json!({ "phase": "discovery", "deliverables": discovery }),
            json!({ "phase": "production", "deliverables": production }),
            json!({ "phase": "launch", "deliverables": launch }),
        ]
    };
    Ok(json!({ "phases": phases }))
}

fn package_resources(arguments: &Value, user_files_root: Option<&Path>) -> Result<Value, String> {
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut manifest: Vec<Value> = Vec::new();
    let mut total_bytes: u64 = 0;

    // Preferred mode: walk a real directory. We sandbox every walk to the
    // configured `user_files_path` so a malformed argument can't enumerate
    // the whole filesystem.
    if let Some(rel) = arguments.get("input_directory").and_then(|v| v.as_str()) {
        let root = user_files_root.ok_or_else(|| {
            "package_resources with input_directory requires the provider to be configured with \
             a user-files path"
                .to_string()
        })?;
        let target = sandbox_path(root, rel).map_err(|err| err.to_string())?;
        if !target.exists() {
            return Err(format!(
                "input_directory '{rel}' does not exist under user-files"
            ));
        }
        walk_resources(
            &target,
            root,
            &target,
            &mut manifest,
            &mut counts,
            &mut total_bytes,
        )?;
    }

    // Back-compat: an explicit `resources` array keeps working and can
    // coexist with `input_directory`.
    let inline = arguments
        .get("resources")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    for resource in &inline {
        let path = resource
            .get("path")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        let kind = resource
            .get("kind")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| infer_resource_kind(&path));
        *counts.entry(kind.clone()).or_insert(0) += 1;
        manifest.push(json!({ "path": path, "kind": kind }));
    }

    Ok(json!({
        "manifest": manifest,
        "count": manifest.len(),
        "by_kind": counts,
        "total_bytes": total_bytes,
    }))
}

fn walk_resources(
    dir: &Path,
    root: &Path,
    base: &Path,
    manifest: &mut Vec<Value>,
    counts: &mut std::collections::BTreeMap<String, usize>,
    total_bytes: &mut u64,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|err| err.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|err| err.to_string())?;
        let file_type = entry.file_type().map_err(|err| err.to_string())?;
        let path = entry.path();
        if file_type.is_dir() {
            walk_resources(&path, root, base, manifest, counts, total_bytes)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let meta = entry.metadata().map_err(|err| err.to_string())?;
        let size = meta.len();
        *total_bytes = total_bytes.saturating_add(size);

        let relative_to_root = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let relative_to_base = path
            .strip_prefix(base)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let kind = infer_resource_kind(&relative_to_root);
        *counts.entry(kind.clone()).or_insert(0) += 1;

        let modified_rfc3339 = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| {
                chrono::DateTime::<chrono::Utc>::from(
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(d.as_secs()),
                )
                .to_rfc3339()
            });

        manifest.push(json!({
            "path": relative_to_root,
            "relative_path": relative_to_base,
            "kind": kind,
            "size_bytes": size,
            "modified": modified_rfc3339,
        }));
    }
    Ok(())
}

/// Resolve `rel` against `root` while preventing escapes via `..`.
fn sandbox_path(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let mut resolved = root.to_path_buf();
    for component in Path::new(rel).components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                return Err(format!(
                    "path '{rel}' must be relative to the user-files root"
                ));
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !resolved.pop() || resolved.as_path() < root {
                    return Err(format!("path '{rel}' escapes the user-files root"));
                }
            }
            Component::Normal(segment) => resolved.push(segment),
        }
    }
    if !resolved.starts_with(root) {
        return Err(format!("path '{rel}' escapes the user-files root"));
    }
    Ok(resolved)
}

fn summarize_deliverables(arguments: &Value) -> Result<Value, String> {
    let deliverables = optional_string_array(arguments, "deliverables");
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for deliverable in &deliverables {
        let kind = classify_deliverable(deliverable);
        *counts.entry(kind).or_insert(0) += 1;
    }
    Ok(json!({
        "total": deliverables.len(),
        "by_kind": counts,
    }))
}

fn route_review(arguments: &Value) -> Result<Value, String> {
    let stage = require_string(arguments, "stage")?;
    let (next_reviewer, next_stage) = match stage.as_str() {
        "draft" => ("planning-lead", "planning-review"),
        "planning-review" => ("editor", "editorial-review"),
        "editorial-review" => ("research-lead", "research-review"),
        "research-review" => ("release-manager", "release-ready"),
        "release-ready" => ("release-manager", "scheduled"),
        other => {
            return Err(format!("unknown review stage '{other}'"));
        }
    };
    Ok(json!({
        "stage": stage,
        "next_reviewer": next_reviewer,
        "next_stage": next_stage,
    }))
}

fn request_revision(arguments: &Value) -> Result<Value, String> {
    let stage = require_string(arguments, "stage")?;
    let reason = require_string(arguments, "reason")?;
    let due = optional_string(arguments, "due");
    Ok(json!({
        "revision_request": {
            "stage": stage,
            "reason": reason,
            "due": due,
        },
    }))
}

fn advance_stage(arguments: &Value) -> Result<Value, String> {
    let stage = require_string(arguments, "stage")?;
    let next_stage = match stage.as_str() {
        "draft" => "planning-review",
        "planning-review" => "editorial-review",
        "editorial-review" => "research-review",
        "research-review" => "release-ready",
        "release-ready" => "scheduled",
        "scheduled" => "released",
        other => {
            return Err(format!("cannot advance from unknown stage '{other}'"));
        }
    };
    Ok(json!({ "stage": stage, "next_stage": next_stage }))
}

fn schedule_release(arguments: &Value) -> Result<Value, String> {
    let release_date = require_string(arguments, "release_date")?;
    let default_stages = [
        "draft",
        "planning-review",
        "editorial-review",
        "research-review",
        "release-ready",
        "scheduled",
    ];
    let stages = arguments
        .get("stages")
        .and_then(|value| value.as_array())
        .map(|array| {
            array
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| default_stages.iter().map(|s| (*s).to_string()).collect());
    Ok(json!({
        "release_date": release_date,
        "stages": stages,
        "stage_count": stages.len(),
    }))
}

fn infer_resource_kind(path: &str) -> String {
    let lowered = path.to_ascii_lowercase();
    if lowered.ends_with(".jpg")
        || lowered.ends_with(".jpeg")
        || lowered.ends_with(".png")
        || lowered.ends_with(".gif")
        || lowered.ends_with(".webp")
    {
        "image".to_string()
    } else if lowered.ends_with(".mp4") || lowered.ends_with(".mov") || lowered.ends_with(".webm") {
        "video".to_string()
    } else if lowered.ends_with(".mp3") || lowered.ends_with(".wav") || lowered.ends_with(".flac") {
        "audio".to_string()
    } else if lowered.ends_with(".md") || lowered.ends_with(".txt") || lowered.ends_with(".html") {
        "response".to_string()
    } else {
        "other".to_string()
    }
}

fn classify_deliverable(deliverable: &str) -> String {
    let lowered = deliverable.to_ascii_lowercase();
    if lowered.contains("video") {
        "video".to_string()
    } else if lowered.contains("image") || lowered.contains("photo") || lowered.contains("banner") {
        "image".to_string()
    } else if lowered.contains("response")
        || lowered.contains("article")
        || lowered.contains("post")
    {
        "response".to_string()
    } else if lowered.contains("email") || lowered.contains("newsletter") {
        "email".to_string()
    } else {
        "other".to_string()
    }
}

/// Kebab-case slug for a free-form title. Strips punctuation, collapses
/// whitespace, and trims hyphens so the slug is safe to use as a
/// filename.
fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_hyphen = false;
    for ch in input.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_hyphen = false;
        } else if !prev_hyphen && !out.is_empty() {
            out.push('-');
            prev_hyphen = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("untitled");
    }
    out
}

/// Render one of the capability outputs into a markdown string.
fn render_artifact_markdown(capability: &str, arguments: &Value, result: &Value) -> String {
    match capability {
        PLANNING_PLAN_INITIATIVE => {
            let mut md = String::from("# Initiative Plan\n\n");
            if let Some(phases) = result["phases"].as_array() {
                for phase in phases {
                    let name = phase["phase"].as_str().unwrap_or("phase");
                    md.push_str(&format!("## {name}\n\n"));
                    if let Some(items) = phase["deliverables"].as_array() {
                        if items.is_empty() {
                            md.push_str("_(no deliverables assigned)_\n\n");
                        } else {
                            for item in items {
                                if let Some(s) = item.as_str() {
                                    md.push_str(&format!("- {s}\n"));
                                }
                            }
                            md.push('\n');
                        }
                    }
                }
            }
            md.push_str("---\n_Generated by `planning.plan_initiative`._\n");
            md
        }
        PLANNING_SUMMARIZE_DELIVERABLES => {
            let mut md = String::from("# Deliverables Summary\n\n");
            md.push_str(&format!(
                "Total: **{}**\n\n",
                result["total"].as_u64().unwrap_or(0)
            ));
            md.push_str("## By kind\n\n");
            if let Some(counts) = result["by_kind"].as_object() {
                for (kind, n) in counts {
                    md.push_str(&format!("- `{kind}` × {}\n", n));
                }
            }
            md.push_str("\n---\n_Generated by `planning.summarize_deliverables`._\n");
            md
        }
        ORCHESTRATION_SCHEDULE_RELEASE => {
            let mut md = String::from("# Release Schedule\n\n");
            md.push_str(&format!(
                "Target release: **{}**\n\n",
                result["release_date"].as_str().unwrap_or("(unset)")
            ));
            if let Some(schedule) = result["schedule"].as_array() {
                md.push_str("| Stage | Date |\n|---|---|\n");
                for entry in schedule {
                    md.push_str(&format!(
                        "| {} | {} |\n",
                        entry["stage"].as_str().unwrap_or(""),
                        entry["date"].as_str().unwrap_or("")
                    ));
                }
            }
            md.push_str("\n---\n_Generated by `orchestration.schedule_release`._\n");
            md
        }
        ORCHESTRATION_REQUEST_REVISION => {
            let rev = &result["revision_request"];
            format!(
                "# Revision Request\n\n\
                 **Stage:** {stage}\n\n\
                 **Reason:** {reason}\n\n\
                 **Due:** {due}\n\n\
                 ---\n_Generated by `orchestration.request_revision`._\n",
                stage = rev["stage"].as_str().unwrap_or(""),
                reason = rev["reason"].as_str().unwrap_or(""),
                due = rev["due"].as_str().unwrap_or("—"),
            )
        }
        _ => {
            // Fallback for anything we don't have a dedicated template
            // for — dump the input + output as JSON.
            format!(
                "# {capability}\n\n## Arguments\n\n```json\n{}\n```\n\n## Result\n\n```json\n{}\n```\n",
                serde_json::to_string_pretty(arguments).unwrap_or_default(),
                serde_json::to_string_pretty(result).unwrap_or_default(),
            )
        }
    }
}

/// For artifact-producing capabilities: render a markdown file to
/// `<root>/<subdir>/<slug>.md` (or a sibling `.json` for pure-data JSON
/// manifests), and inject `artifact_path` into the result so callers can
/// link to it.
fn persist_artifact(
    root: &Path,
    capability: &str,
    arguments: &Value,
    result: &mut Value,
) -> std::io::Result<()> {
    let (subdir, slug_basis, extension): (&str, String, &str) = match capability {
        PLANNING_PLAN_INITIATIVE => {
            let seed = arguments
                .get("title")
                .and_then(|v| v.as_str())
                .or_else(|| arguments.get("initiative").and_then(|v| v.as_str()))
                .unwrap_or("initiative-plan")
                .to_string();
            ("initiatives", seed, "md")
        }
        PLANNING_PACKAGE_RESOURCES => {
            let seed = arguments
                .get("name")
                .and_then(|v| v.as_str())
                .or_else(|| arguments.get("input_directory").and_then(|v| v.as_str()))
                .unwrap_or("resources")
                .to_string();
            ("resources", seed, "json")
        }
        PLANNING_SUMMARIZE_DELIVERABLES => (
            "deliverables",
            arguments
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("deliverables-summary")
                .to_string(),
            "md",
        ),
        ORCHESTRATION_SCHEDULE_RELEASE => (
            "releases",
            arguments
                .get("title")
                .and_then(|v| v.as_str())
                .or_else(|| arguments.get("release_date").and_then(|v| v.as_str()))
                .unwrap_or("release")
                .to_string(),
            "md",
        ),
        ORCHESTRATION_REQUEST_REVISION => {
            let seed = arguments
                .get("stage")
                .and_then(|v| v.as_str())
                .unwrap_or("revision")
                .to_string();
            (
                "revisions",
                format!("{seed}-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S")),
                "md",
            )
        }
        _ => return Ok(()),
    };

    let slug = slugify(&slug_basis);
    let dir = root.join(subdir);
    std::fs::create_dir_all(&dir)?;
    let file_path = dir.join(format!("{slug}.{extension}"));

    let body = if extension == "json" {
        serde_json::to_string_pretty(result).unwrap_or_else(|_| "{}".into())
    } else {
        render_artifact_markdown(capability, arguments, result)
    };
    std::fs::write(&file_path, body)?;

    if let Some(object) = result.as_object_mut() {
        let relative = file_path
            .strip_prefix(root)
            .unwrap_or(&file_path)
            .to_string_lossy()
            .replace('\\', "/");
        object.insert("artifact_path".into(), Value::String(relative));
        object.insert(
            "artifact_absolute_path".into(),
            Value::String(file_path.to_string_lossy().to_string()),
        );
    }
    Ok(())
}

