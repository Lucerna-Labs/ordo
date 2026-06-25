use crate::*;
use crate::helpers::*;

const MAINTENANCE_CAPABILITIES: &[&str] = &[
    SKILLS_LIST,
    SKILLS_GET,
    SKILLS_INSTALL,
    SKILLS_DELETE,
    SKILLS_AUDIT_ROUTING,
    SKILLS_REPAIR_ROUTING,
    PLUGINS_LIST,
    PLUGINS_INSTALL,
    PLUGINS_DELETE,
    PLUGINS_SET_ENABLED,
    AUTOMATION_LIST,
    AUTOMATION_INSPECT,
    AGENT_TEAMS_LIST,
    AGENT_TEAMS_GET,
    AGENT_TEAMS_UPSERT,
    AGENT_TEAMS_DELETE,
    AGENT_TEAMS_SET_ACTIVE,
    LOGS_SYSTEM_TAIL,
];

pub struct MaintenanceProvider {
    user_files_root: PathBuf,
    plugins_root: PathBuf,
    /// Live mode registry — required for `skills.audit_routing` (it routes every
    /// skill against every mode). `None` in tests / pre-mode shape, in which
    /// case the audit returns a clear error instead of panicking.
    modes: Option<ordo_modes::ModeRegistry>,
}

impl MaintenanceProvider {
    pub fn new(user_files_root: impl Into<PathBuf>, plugins_root: impl Into<PathBuf>) -> Self {
        Self {
            user_files_root: user_files_root.into(),
            plugins_root: plugins_root.into(),
            modes: None,
        }
    }

    /// Attach the mode registry so `skills.audit_routing` can run.
    pub fn with_modes(mut self, modes: ordo_modes::ModeRegistry) -> Self {
        self.modes = Some(modes);
        self
    }

    fn skills_root(&self) -> PathBuf {
        self.user_files_root.join("skills")
    }

    fn automations_path(&self) -> PathBuf {
        self.user_files_root.join("automations.json")
    }

    fn agent_teams_path(&self) -> PathBuf {
        self.user_files_root.join("agent-teams.json")
    }
}

#[async_trait]
impl CapabilityProvider for MaintenanceProvider {
    fn name(&self) -> &str {
        "ordo-maintenance"
    }

    fn capabilities(&self) -> Vec<String> {
        MAINTENANCE_CAPABILITIES
            .iter()
            .map(|capability| (*capability).to_string())
            .collect()
    }

    fn capability_descriptors(&self) -> Vec<CapabilityDescriptor> {
        MAINTENANCE_CAPABILITIES
            .iter()
            .map(|capability| {
                CapabilityDescriptor::new(
                    *capability,
                    self.name(),
                    maintenance_description(capability),
                    CapabilityTier::Core,
                    CapabilityActivation::Eager,
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
            SKILLS_LIST => maintenance_list_skills(&self.skills_root()),
            SKILLS_GET => maintenance_get_skill(&self.skills_root(), arguments),
            SKILLS_INSTALL => maintenance_install_skill(&self.skills_root(), arguments),
            SKILLS_DELETE => maintenance_delete_named_dir(&self.skills_root(), arguments, "skill"),
            SKILLS_AUDIT_ROUTING => {
                maintenance_audit_skill_routing(&self.skills_root(), self.modes.as_ref())
            }
            SKILLS_REPAIR_ROUTING => {
                let apply = arguments
                    .get("apply")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                maintenance_repair_skill_routing(&self.skills_root(), self.modes.as_ref(), apply)
            }
            PLUGINS_LIST => maintenance_list_plugins(&self.plugins_root),
            PLUGINS_INSTALL => maintenance_install_plugin(&self.plugins_root, arguments),
            PLUGINS_DELETE => maintenance_delete_named_dir(&self.plugins_root, arguments, "plugin"),
            PLUGINS_SET_ENABLED => maintenance_set_plugin_enabled(&self.plugins_root, arguments),
            AUTOMATION_LIST => maintenance_list_automations(&self.automations_path()),
            AUTOMATION_INSPECT => {
                maintenance_inspect_automation(&self.automations_path(), arguments)
            }
            AGENT_TEAMS_LIST => maintenance_agent_teams_list(&self.agent_teams_path()),
            AGENT_TEAMS_GET => maintenance_agent_teams_get(&self.agent_teams_path(), arguments),
            AGENT_TEAMS_UPSERT => {
                maintenance_agent_teams_upsert(&self.agent_teams_path(), arguments)
            }
            AGENT_TEAMS_DELETE => {
                maintenance_agent_teams_delete(&self.agent_teams_path(), arguments)
            }
            AGENT_TEAMS_SET_ACTIVE => {
                maintenance_agent_teams_set_active(&self.agent_teams_path(), arguments)
            }
            LOGS_SYSTEM_TAIL => maintenance_tail_system_logs(&self.user_files_root, arguments),
            _ => return None,
        };
        Some(match result {
            Ok(value) => ToolCallResult::Completed { result: value },
            Err(error) => ToolCallResult::Failed { error },
        })
    }
}

fn maintenance_description(capability: &str) -> &'static str {
    match capability {
        SKILLS_LIST => "List locally installed Ordo skills under user-files/skills.",
        SKILLS_GET => "Read one installed skill's full markdown by id (the progressive-disclosure path: the prompt lists a mode's skills, the model fetches the body on demand).",
        SKILLS_INSTALL => "Install or update a local Ordo skill by writing user-files/skills/<id>/skill.md.",
        SKILLS_DELETE => "Delete a local Ordo skill directory by id.",
        SKILLS_AUDIT_ROUTING => "Audit how every installed skill routes across every mode: flags orphaned skills, declared-but-vetoed contradictions, and skills declaring modes that don't exist (phantom modes). Read-only.",
        SKILLS_REPAIR_ROUTING => "Apply ONLY safe skill-frontmatter repairs: drop phantom-mode declarations (modes that don't exist) from a skill that still keeps at least one real mode. Dry-run by default; pass apply=true to write. Skills declaring ONLY nonexistent modes, and all mode-side policy issues, are deferred to the operator.",
        PLUGINS_LIST => "List local plugin manifests under user-files/plugins.",
        PLUGINS_INSTALL => "Install or update a plugin manifest under user-files/plugins/<name>/plugin.json. Restart required to load.",
        PLUGINS_DELETE => "Delete a local plugin directory by name. Restart required to unload if active.",
        PLUGINS_SET_ENABLED => "Enable or disable a local plugin manifest. Restart required to apply.",
        AUTOMATION_LIST => "List registered Ordo automations and their recent automation events.",
        AUTOMATION_INSPECT => "Inspect one registered Ordo automation by id without mutating it.",
        AGENT_TEAMS_LIST => "List configured Agent Teams and the active team id from user-files/agent-teams.json.",
        AGENT_TEAMS_GET => "Read one Agent Team by id.",
        AGENT_TEAMS_UPSERT => "Create or update one Agent Team definition. Used by Tech Specialist after operator approval.",
        AGENT_TEAMS_DELETE => "Delete one Agent Team by id. Used by Tech Specialist after operator approval.",
        AGENT_TEAMS_SET_ACTIVE => "Set or clear the active Agent Team id.",
        LOGS_SYSTEM_TAIL => "Read a bounded tail of local Ordo runtime system logs for diagnostics.",
        _ => "Ordo maintenance capability.",
    }
}

/// Read one installed skill's full markdown + parsed metadata by id. The
/// progressive-disclosure fetch behind the per-mode skills list in the prompt.
fn maintenance_get_skill(skills_root: &Path, arguments: &Value) -> Result<Value, String> {
    let id = required_name(arguments, &["id", "name"])?;
    let dir = safe_named_dir(skills_root, &id)?;
    let skill_path = if dir.join("skill.md").exists() {
        dir.join("skill.md")
    } else {
        dir.join("SKILL.md")
    };
    if !skill_path.exists() {
        return Err(format!("skill '{id}' not found"));
    }
    let content = std::fs::read_to_string(&skill_path).map_err(|err| err.to_string())?;
    let manifest = ordo_skills::SkillManifest::from_markdown(&id, &content);
    Ok(json!({
        "id": id,
        "path": skill_path.display().to_string(),
        "name": manifest.name,
        "description": manifest.description,
        "tags": manifest.tags,
        "modes": manifest.modes,
        "risk_level": manifest.risk_level,
        "content": content,
    }))
}

/// Audit skill→mode routing health. Read-only: discovers skills from disk,
/// lists all modes, and runs the routing audit (`ordo_modes::audit_skill_routing`).
/// This is what the diagnostic mode's daily scan calls (see
/// `docs/skill-routing.md`).
fn maintenance_audit_skill_routing(
    skills_root: &Path,
    modes: Option<&ordo_modes::ModeRegistry>,
) -> Result<Value, String> {
    let registry = modes.ok_or_else(|| {
        "skills.audit_routing requires the mode registry, which is not attached".to_string()
    })?;
    let modes = registry.list();
    let skills = ordo_skills::discover_skills(skills_root).map_err(|err| err.to_string())?;
    let audit = ordo_modes::audit_skill_routing(&modes, &skills);

    let orphaned = audit.orphaned();
    let unhealthy_count = audit.unhealthy().len();
    let anomaly_count = audit.anomaly_count();
    let audit_value = serde_json::to_value(&audit).map_err(|err| err.to_string())?;
    Ok(json!({
        "skills_root": skills_root.display().to_string(),
        "mode_count": modes.len(),
        "skill_count": skills.len(),
        "anomaly_count": anomaly_count,
        "orphaned": orphaned,
        "unhealthy_count": unhealthy_count,
        "audit": audit_value,
    }))
}

/// Bounded SAFE repair of skill routing (see `docs/skill-routing.md`). For each
/// skill that declares a phantom mode (one that doesn't exist) while STILL
/// declaring at least one real mode, drop the phantom entries from its
/// `available_to_modes` frontmatter — an inert, no-behaviour-change cleanup. A
/// skill that declares ONLY nonexistent modes is DEFERRED (removing all of them
/// would change its routing — the operator must pick real modes). All mode-side
/// policy issues are out of scope here (the runtime can't self-edit modes).
///
/// Dry-run by default (`apply=false`) returns the plan; `apply=true` rewrites
/// the `skill.md` files and reports what changed.
fn maintenance_repair_skill_routing(
    skills_root: &Path,
    modes: Option<&ordo_modes::ModeRegistry>,
    apply: bool,
) -> Result<Value, String> {
    let registry = modes.ok_or_else(|| {
        "skills.repair_routing requires the mode registry, which is not attached".to_string()
    })?;
    let mode_ids: std::collections::BTreeSet<String> =
        registry.list().into_iter().map(|m| m.id).collect();
    let skills = ordo_skills::discover_skills(skills_root).map_err(|err| err.to_string())?;
    // Canonical root for the path-traversal guard: a write target must resolve
    // INSIDE this (so a symlinked skill dir can't escape user-files/skills).
    let canonical_root = std::fs::canonicalize(skills_root).ok();

    let mut safe_repairs = Vec::new();
    let mut deferred = Vec::new();
    let mut errors = Vec::new();
    for skill in &skills {
        if skill.modes.is_empty() {
            continue;
        }
        let phantoms: Vec<String> = skill
            .modes
            .iter()
            .filter(|m| !mode_ids.contains(*m))
            .cloned()
            .collect();
        if phantoms.is_empty() {
            continue;
        }
        let valid: Vec<String> = skill
            .modes
            .iter()
            .filter(|m| mode_ids.contains(*m))
            .cloned()
            .collect();
        if valid.is_empty() {
            deferred.push(json!({
                "skill_id": skill.id,
                "declared_modes": skill.modes,
                "reason": "skill declares only nonexistent modes; an operator must choose real modes (removing all declarations would change routing)",
            }));
            continue;
        }
        // Safe: removing the phantom entries leaves real declarations intact.
        if apply {
            // One skill's failure must not abort the rest (continue-on-error).
            let outcome = (|| -> Result<usize, String> {
                let path = skill
                    .path
                    .as_ref()
                    .ok_or_else(|| format!("skill '{}' has no resolved path", skill.id))?;
                let canonical = std::fs::canonicalize(path).map_err(|err| err.to_string())?;
                if let Some(root) = canonical_root.as_deref() {
                    if !canonical.starts_with(root) {
                        return Err(format!(
                            "refusing to write outside the skills root: {}",
                            canonical.display()
                        ));
                    }
                }
                let content = std::fs::read_to_string(&canonical).map_err(|err| err.to_string())?;
                let (rewritten, removed) =
                    ordo_skills::remove_modes_from_frontmatter(&content, &phantoms);
                if removed > 0 {
                    std::fs::write(&canonical, rewritten).map_err(|err| err.to_string())?;
                }
                Ok(removed)
            })();
            match outcome {
                Ok(removed) => safe_repairs.push(json!({
                    "skill_id": skill.id,
                    "removed_modes": phantoms,
                    "resulting_modes": valid,
                    "edits": removed,
                    "path": skill.path,
                })),
                Err(error) => errors.push(json!({ "skill_id": skill.id, "error": error })),
            }
        } else {
            safe_repairs.push(json!({
                "skill_id": skill.id,
                "remove_modes": phantoms,
                "resulting_modes": valid,
            }));
        }
    }

    Ok(json!({
        "applied": apply,
        "skills_root": skills_root.display().to_string(),
        "safe_repairs": safe_repairs,
        "deferred": deferred,
        "errors": errors,
        "note": if apply { "safe skill-frontmatter repairs written" } else { "dry-run; pass apply=true to write" },
    }))
}

fn maintenance_load_automations(path: &Path) -> Result<AutomationOrchestrator, String> {
    AutomationOrchestrator::load_or_seed(
        path,
        vec![
            default_diagnostic_automation(),
            default_dreaming_automation(),
        ],
    )
    .map_err(|err| err.to_string())
}

fn maintenance_list_automations(path: &Path) -> Result<Value, String> {
    let automation = maintenance_load_automations(path)?;
    let automations = automation.list().into_iter().cloned().collect::<Vec<_>>();
    Ok(json!({
        "path": path.display().to_string(),
        "count": automations.len(),
        "automations": automations,
        "events": automation.event_log(),
    }))
}

fn maintenance_inspect_automation(path: &Path, arguments: &Value) -> Result<Value, String> {
    let id = first_string(arguments, &["id", "automation_id"])
        .ok_or_else(|| "automation.inspect requires id or automation_id".to_string())?;
    let automation_id = AutomationId::parse_str(id.trim()).map_err(|err| err.to_string())?;
    let automation = maintenance_load_automations(path)?;
    let spec = automation
        .get(automation_id)
        .cloned()
        .ok_or_else(|| "automation not found".to_string())?;
    Ok(json!({ "automation": spec }))
}

fn maintenance_tail_system_logs(root: &Path, arguments: &Value) -> Result<Value, String> {
    let limit = arguments
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(200)
        .clamp(1, 1_000) as usize;
    let source = optional_string(arguments, "source")
        .unwrap_or_else(|| "both".to_string())
        .to_ascii_lowercase();
    let mut logs = Vec::new();
    for (name, path) in system_log_candidates(root) {
        let include = match source.as_str() {
            "stdout" | "out" => name.contains("out"),
            "stderr" | "err" => name.contains("err"),
            "both" | "all" => true,
            other => {
                return Err(format!(
                    "unsupported log source '{other}'; use stdout, stderr, or both"
                ))
            }
        };
        if !include || !path.is_file() {
            continue;
        }
        let lines = tail_file_lines(&path, limit)?;
        logs.push(json!({
            "name": name,
            "path": path.display().to_string(),
            "line_count": lines.len(),
            "lines": lines,
        }));
    }
    Ok(json!({
        "source": source,
        "limit": limit,
        "count": logs.len(),
        "logs": logs,
    }))
}

fn system_log_candidates(root: &Path) -> Vec<(String, PathBuf)> {
    let mut dirs = Vec::new();
    dirs.push(root.to_path_buf());
    if let Some(parent) = root.parent() {
        dirs.push(parent.to_path_buf());
        if let Some(grandparent) = parent.parent() {
            dirs.push(grandparent.to_path_buf());
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut candidates = Vec::new();
    for dir in dirs {
        for name in ["runtime-dev.out.log", "runtime-dev.err.log"] {
            let path = dir.join(name);
            if seen.insert(path.clone()) {
                candidates.push((name.to_string(), path));
            }
        }
        for name in ["ordo.log", "runtime.log", "system.log"] {
            let path = dir.join("logs").join(name);
            if seen.insert(path.clone()) {
                candidates.push((name.to_string(), path));
            }
        }
    }
    candidates
}

fn tail_file_lines(path: &Path, limit: usize) -> Result<Vec<String>, String> {
    let content = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
    let mut lines = content
        .lines()
        .rev()
        .take(limit)
        .map(str::to_string)
        .collect::<Vec<_>>();
    lines.reverse();
    Ok(lines)
}

fn maintenance_list_skills(root: &Path) -> Result<Value, String> {
    std::fs::create_dir_all(root).map_err(|err| err.to_string())?;
    let mut skills = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        if !entry.file_type().map_err(|err| err.to_string())?.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().to_string();
        let skill_path = entry.path().join("skill.md");
        skills.push(json!({
            "id": id,
            "path": skill_path.display().to_string(),
            "installed": skill_path.exists(),
        }));
    }
    Ok(json!({ "root": root.display().to_string(), "count": skills.len(), "skills": skills }))
}

fn maintenance_install_skill(root: &Path, arguments: &Value) -> Result<Value, String> {
    let id = required_name(arguments, &["id", "name"])?;
    let body = first_string(arguments, &["body", "content", "skill_md"])
        .ok_or_else(|| "skills.install requires body, content, or skill_md".to_string())?;
    if body.trim().is_empty() {
        return Err("skills.install body cannot be empty".into());
    }
    let overwrite = arguments
        .get("overwrite")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let dir = safe_named_dir(root, &id)?;
    let skill_path = dir.join("skill.md");
    if skill_path.exists() && !overwrite {
        return Err(format!(
            "skill '{id}' already exists; pass overwrite=true to replace it"
        ));
    }
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    std::fs::write(&skill_path, body).map_err(|err| err.to_string())?;
    if let Some(metadata) = arguments.get("metadata") {
        let metadata_path = dir.join("metadata.json");
        let encoded = serde_json::to_string_pretty(metadata).map_err(|err| err.to_string())?;
        std::fs::write(metadata_path, encoded).map_err(|err| err.to_string())?;
    }
    Ok(json!({
        "id": id,
        "path": skill_path.display().to_string(),
        "note": "skill installed locally; refresh the UXI or restart runtime if the skill index is cached",
    }))
}

fn maintenance_list_plugins(root: &Path) -> Result<Value, String> {
    std::fs::create_dir_all(root).map_err(|err| err.to_string())?;
    let mut plugins = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        if !entry.file_type().map_err(|err| err.to_string())?.is_dir() {
            continue;
        }
        let manifest_path = entry.path().join("plugin.json");
        if !manifest_path.exists() {
            continue;
        }
        let raw = std::fs::read_to_string(&manifest_path).map_err(|err| err.to_string())?;
        let manifest: Value = serde_json::from_str(&raw).map_err(|err| err.to_string())?;
        plugins.push(json!({
            "name": manifest.get("name").and_then(Value::as_str).unwrap_or(""),
            "enabled": manifest.get("enabled").and_then(Value::as_bool).unwrap_or(false),
            "manifest_path": manifest_path.display().to_string(),
        }));
    }
    Ok(json!({ "root": root.display().to_string(), "count": plugins.len(), "plugins": plugins }))
}

fn maintenance_install_plugin(root: &Path, arguments: &Value) -> Result<Value, String> {
    let manifest_value = arguments.get("manifest").unwrap_or(arguments).clone();
    let mut manifest: Value = manifest_value;
    let name = manifest
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "plugins.install requires manifest.name".to_string())?;
    validate_name(&name)?;
    let command = manifest
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if command.is_empty() {
        return Err("plugins.install requires manifest.command".into());
    }
    let core_override = manifest
        .get("core_override")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !core_override {
        if let Some(lanes) = manifest.get("expected_lanes").and_then(Value::as_array) {
            for lane in lanes.iter().filter_map(Value::as_str) {
                if [
                    "cloud.",
                    "runtime.",
                    "filesystem.",
                    "self_heal.",
                    "memory.",
                    "knowledge.",
                ]
                .iter()
                .any(|reserved| lane.starts_with(reserved))
                {
                    return Err(format!(
                        "plugin lane '{lane}' is reserved for core runtime providers"
                    ));
                }
            }
        }
    }
    if manifest.get("version").is_none() {
        manifest["version"] = Value::String("0.0.0".into());
    }
    if manifest.get("enabled").is_none() {
        manifest["enabled"] = Value::Bool(true);
    }
    let overwrite = arguments
        .get("overwrite")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let dir = safe_named_dir(root, &name)?;
    let manifest_path = dir.join("plugin.json");
    if manifest_path.exists() && !overwrite {
        return Err(format!(
            "plugin '{name}' already exists; pass overwrite=true to replace it"
        ));
    }
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let encoded = serde_json::to_string_pretty(&manifest).map_err(|err| err.to_string())?;
    std::fs::write(&manifest_path, encoded).map_err(|err| err.to_string())?;
    Ok(json!({
        "name": name,
        "manifest_path": manifest_path.display().to_string(),
        "note": "plugin manifest installed locally; restart runtime to spawn it",
    }))
}

fn maintenance_set_plugin_enabled(root: &Path, arguments: &Value) -> Result<Value, String> {
    let name = required_name(arguments, &["name", "id"])?;
    let enabled = arguments
        .get("enabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| "plugins.set_enabled requires enabled=true or false".to_string())?;
    let dir = safe_named_dir(root, &name)?;
    let manifest_path = dir.join("plugin.json");
    let raw = std::fs::read_to_string(&manifest_path).map_err(|err| err.to_string())?;
    let mut manifest: Value = serde_json::from_str(&raw).map_err(|err| err.to_string())?;
    manifest["enabled"] = Value::Bool(enabled);
    let encoded = serde_json::to_string_pretty(&manifest).map_err(|err| err.to_string())?;
    std::fs::write(&manifest_path, encoded).map_err(|err| err.to_string())?;
    Ok(json!({
        "name": name,
        "enabled": enabled,
        "manifest_path": manifest_path.display().to_string(),
        "note": "restart runtime to apply plugin enabled state",
    }))
}

fn empty_agent_teams_state() -> Value {
    json!({
        "teams": [],
        "active_team_id": ""
    })
}

fn load_agent_teams_state(path: &Path) -> Result<Value, String> {
    if !path.exists() {
        return Ok(empty_agent_teams_state());
    }
    let raw = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
    let state: Value = serde_json::from_str(&raw).map_err(|err| err.to_string())?;
    normalize_agent_teams_state(state)
}

fn write_agent_teams_state(path: &Path, state: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let encoded = serde_json::to_vec_pretty(state).map_err(|err| err.to_string())?;
    ordo_store::atomic_write(path, &encoded).map_err(|err| err.to_string())
}

fn normalize_agent_teams_state(state: Value) -> Result<Value, String> {
    let mut teams = state
        .get("teams")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    for team in &teams {
        let id = team
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| "each Agent Team requires a non-empty id".to_string())?;
        if !seen.insert(id.to_string()) {
            return Err(format!("duplicate Agent Team id '{id}'"));
        }
        if team
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
        {
            return Err(format!("Agent Team '{id}' requires a non-empty name"));
        }
        if !team
            .get("members")
            .and_then(Value::as_array)
            .map(|members| !members.is_empty())
            .unwrap_or(false)
        {
            return Err(format!("Agent Team '{id}' requires at least one member"));
        }
    }
    teams.sort_by(|a, b| {
        a.get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .cmp(b.get("name").and_then(Value::as_str).unwrap_or(""))
    });
    let requested_active = state
        .get("active_team_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let active_team_id = if requested_active.is_empty()
        || teams.iter().any(|team| {
            team.get("id")
                .and_then(Value::as_str)
                .map(|id| id == requested_active)
                .unwrap_or(false)
        }) {
        requested_active
    } else {
        ""
    };
    Ok(json!({
        "teams": teams,
        "active_team_id": active_team_id
    }))
}

fn maintenance_agent_teams_list(path: &Path) -> Result<Value, String> {
    let state = load_agent_teams_state(path)?;
    Ok(json!({
        "path": path.display().to_string(),
        "teams": state.get("teams").cloned().unwrap_or_else(|| json!([])),
        "active_team_id": state.get("active_team_id").and_then(Value::as_str).unwrap_or(""),
    }))
}

fn maintenance_agent_teams_get(path: &Path, arguments: &Value) -> Result<Value, String> {
    let id = required_name(arguments, &["id", "team_id"])?;
    let state = load_agent_teams_state(path)?;
    let team = state
        .get("teams")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|team| team.get("id").and_then(Value::as_str) == Some(id.as_str()))
        .cloned()
        .ok_or_else(|| format!("Agent Team '{id}' not found"))?;
    Ok(json!({ "team": team }))
}

fn maintenance_agent_teams_upsert(path: &Path, arguments: &Value) -> Result<Value, String> {
    let team = arguments.get("team").unwrap_or(arguments).clone();
    let team_id = team
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "agent_teams.upsert requires team.id".to_string())?
        .to_string();
    let state = load_agent_teams_state(path)?;
    let mut teams = state
        .get("teams")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut updated = false;
    for existing in &mut teams {
        if existing.get("id").and_then(Value::as_str) == Some(team_id.as_str()) {
            *existing = team.clone();
            updated = true;
            break;
        }
    }
    if !updated {
        teams.push(team);
    }
    let next = normalize_agent_teams_state(json!({
        "teams": teams,
        "active_team_id": state.get("active_team_id").and_then(Value::as_str).unwrap_or("")
    }))?;
    write_agent_teams_state(path, &next)?;
    Ok(json!({
        "team_id": team_id,
        "created": !updated,
        "path": path.display().to_string(),
        "state": next
    }))
}

fn maintenance_agent_teams_delete(path: &Path, arguments: &Value) -> Result<Value, String> {
    let id = required_name(arguments, &["id", "team_id"])?;
    let state = load_agent_teams_state(path)?;
    let before = state
        .get("teams")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let teams = before
        .iter()
        .filter(|team| team.get("id").and_then(Value::as_str) != Some(id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let deleted = teams.len() != before.len();
    let active = state
        .get("active_team_id")
        .and_then(Value::as_str)
        .filter(|active| *active != id)
        .unwrap_or("");
    let next = normalize_agent_teams_state(json!({
        "teams": teams,
        "active_team_id": active
    }))?;
    write_agent_teams_state(path, &next)?;
    Ok(json!({ "deleted": deleted, "team_id": id, "state": next }))
}

fn maintenance_agent_teams_set_active(path: &Path, arguments: &Value) -> Result<Value, String> {
    let id = first_string(arguments, &["id", "team_id", "active_team_id"])
        .unwrap_or_default()
        .trim()
        .to_string();
    let state = load_agent_teams_state(path)?;
    let teams = state
        .get("teams")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !id.is_empty()
        && !teams
            .iter()
            .any(|team| team.get("id").and_then(Value::as_str) == Some(id.as_str()))
    {
        return Err(format!("Agent Team '{id}' not found"));
    }
    let next = normalize_agent_teams_state(json!({
        "teams": teams,
        "active_team_id": id
    }))?;
    write_agent_teams_state(path, &next)?;
    Ok(json!({ "active_team_id": id, "state": next }))
}

fn maintenance_delete_named_dir(
    root: &Path,
    arguments: &Value,
    label: &str,
) -> Result<Value, String> {
    let name = required_name(arguments, &["id", "name"])?;
    let dir = safe_named_dir(root, &name)?;
    if !dir.exists() {
        return Ok(
            json!({ "name": name, "deleted": false, "note": format!("{label} did not exist") }),
        );
    }
    if dir == root {
        return Err(format!("refusing to delete {label} root"));
    }
    std::fs::remove_dir_all(&dir).map_err(|err| err.to_string())?;
    Ok(json!({ "name": name, "deleted": true, "path": dir.display().to_string() }))
}

fn required_name(arguments: &Value, keys: &[&str]) -> Result<String, String> {
    for key in keys {
        if let Some(value) = arguments.get(*key).and_then(Value::as_str) {
            let value = value.trim().to_string();
            validate_name(&value)?;
            return Ok(value);
        }
    }
    Err(format!(
        "missing required name field: {}",
        keys.join(" or ")
    ))
}

fn first_string(arguments: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| arguments.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

fn validate_name(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("name cannot be empty".into());
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(format!(
            "invalid name '{value}'; use ASCII letters, numbers, hyphens, or underscores"
        ));
    }
    Ok(())
}

fn safe_named_dir(root: &Path, name: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    std::fs::create_dir_all(root).map_err(|err| err.to_string())?;
    Ok(root.join(name))
}

