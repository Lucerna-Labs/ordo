use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};
use tauri::{AppHandle, Emitter, State};

use crate::types::{
    LibrarySnapshot, LogEntry, LogLevel, MechanicReply, NicheModule, P2pStatus, RagCollection,
    ShellBootstrap, SwarmNode, SystemState,
};

#[derive(Default)]
pub struct StudioState {
    pub system_state: Mutex<SystemState>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AssistantModeManifest {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub memory_scope: Vec<String>,
    #[serde(default)]
    pub rag_domains: Vec<String>,
    #[serde(default)]
    pub allowed_tool_lanes: Vec<String>,
    #[serde(default)]
    pub blocked_tool_capabilities: Vec<String>,
    #[serde(default)]
    pub policies: Vec<String>,
    #[serde(default)]
    pub planner_bias: Vec<String>,
    #[serde(default)]
    pub persona: Vec<String>,
    #[serde(default)]
    pub default_timeout_secs: Option<u64>,
    #[serde(default)]
    pub default_strictness: Option<String>,
    #[serde(default)]
    pub default_credential: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AssistantModesResponse {
    pub count: usize,
    pub modes: Vec<AssistantModeManifest>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LocalApiKeyInstallResult {
    pub env_var: String,
    pub platform: String,
    pub installed_for: String,
    pub local_env_path: String,
    pub current_process_ready: bool,
    pub restart_recommended: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct PluginManifest {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub expected_lanes: Vec<String>,
    #[serde(default)]
    pub required_env: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub core_override: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct PluginStatus {
    pub name: String,
    pub version: String,
    pub description: String,
    pub state: String,
    pub tool_count: usize,
    pub expected_lanes: Vec<String>,
    pub enabled: bool,
    pub command: String,
    pub args: Vec<String>,
    pub required_env: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub core_override: bool,
    pub manifest_path: String,
    pub failure: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PluginsResponse {
    pub count: usize,
    pub plugins: Vec<PluginStatus>,
}

#[derive(Clone, Debug, Serialize)]
pub struct McpServerStatus {
    pub server_id: String,
    pub trust_state: String,
    pub installed_at: String,
    pub tool_count: usize,
    pub privilege_tier: Option<String>,
    pub drift: Option<String>,
    pub lockfile_hash: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct McpServersResponse {
    pub count: usize,
    pub servers: Vec<McpServerStatus>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CapabilityLane {
    pub group: String,
    pub name: String,
    pub label: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct CapabilityDescriptor {
    pub capability: String,
    pub description: String,
    pub provider: String,
    pub tier: String,
    pub activation: String,
    pub lane: CapabilityLane,
    pub input_schema: Option<Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CapabilitiesResponse {
    pub count: usize,
    pub descriptors: Vec<CapabilityDescriptor>,
}

fn default_true() -> bool {
    true
}

fn default_plugin_version() -> String {
    "0.1.0".to_string()
}

#[tauri::command]
pub fn get_shell_bootstrap(state: State<'_, StudioState>) -> Result<ShellBootstrap, String> {
    let system_state = lock_state(&state.system_state)?.clone();
    let niche_modules = load_niche_modules()?;
    let library = build_library_snapshot(&system_state, &niche_modules);
    let mut active_niches = vec!["Project Research".to_string(), "Runtime Ops".to_string()];
    for module in &niche_modules {
        if !active_niches.iter().any(|label| label == &module.label) {
            active_niches.push(module.label.clone());
        }
    }

    Ok(ShellBootstrap {
        system_state,
        niche_modules,
        library,
        active_niches,
    })
}

#[tauri::command]
pub fn get_library_snapshot(state: State<'_, StudioState>) -> Result<LibrarySnapshot, String> {
    let system_state = lock_state(&state.system_state)?.clone();
    let niche_modules = load_niche_modules()?;
    Ok(build_library_snapshot(&system_state, &niche_modules))
}

#[tauri::command]
pub fn init_new_crate(
    app: AppHandle,
    name: String,
    state: State<'_, StudioState>,
) -> Result<NicheModule, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Niche name cannot be empty.".to_string());
    }

    let module = build_niche_module(trimmed);
    let config_dir = niche_config_dir()?;
    fs::create_dir_all(&config_dir).map_err(|error| error.to_string())?;
    let config_path = config_dir.join(format!("{}.json", module.id));

    if config_path.exists() {
        emit_log(
            &app,
            "ORCHESTRATOR",
            format!(
                "Niche {} already exists. Reusing the existing modular lane.",
                module.label
            ),
            LogLevel::Warn,
        )?;
        return load_niche_module_file(&config_path);
    }

    let serialized = serde_json::to_string_pretty(&module).map_err(|error| error.to_string())?;
    fs::write(&config_path, serialized).map_err(|error| error.to_string())?;

    *lock_state(&state.system_state)? = SystemState::Healthy;
    emit_log(
        &app,
        "ORCHESTRATOR",
        format!(
            "Niche {} registered at {}.",
            module.label,
            relative_config_path(&config_path)
        ),
        LogLevel::Info,
    )?;
    emit_log(
        &app,
        "LIBRARY",
        format!(
            "Carving dedicated retrieval lane {} for {}.",
            module.collection_id, module.label
        ),
        LogLevel::Info,
    )?;

    Ok(module)
}

#[tauri::command]
pub fn simulate_failure(app: AppHandle, state: State<'_, StudioState>) -> Result<(), String> {
    *lock_state(&state.system_state)? = SystemState::Rescue;
    emit_log(
        &app,
        "GATEWAY",
        "PQ Handshake (KYBER-768) failed. Triggering traditional fallback relay.",
        LogLevel::Error,
    )?;
    emit_log(
        &app,
        "ROUTER",
        "Peer mesh downgraded to rescue posture while direct bridges are rebalanced.",
        LogLevel::Warn,
    )?;
    emit_log(
        &app,
        "MECHANIC",
        "Local LLM Mechanic engaged. Inspecting crate stability and replay candidates.",
        LogLevel::Info,
    )?;
    Ok(())
}

#[tauri::command]
pub fn send_mechanic_command(
    app: AppHandle,
    command: String,
    state: State<'_, StudioState>,
) -> Result<MechanicReply, String> {
    let lowered = command.trim().to_ascii_lowercase();
    if lowered.is_empty() {
        return Err("Mechanic command cannot be empty.".to_string());
    }

    let current = lock_state(&state.system_state)?.clone();
    let reply = if lowered.contains("stabilize")
        || lowered.contains("repair")
        || lowered.contains("patch")
        || lowered.contains("clear")
    {
        emit_log(
            &app,
            "MECHANIC",
            "Manual repair directive accepted. Releasing fallback pressure and restoring direct bridge preference.",
            LogLevel::Info,
        )?;
        emit_log(
            &app,
            "GATEWAY",
            "Fallback retired. Mesh returned to healthy post-quantum posture.",
            LogLevel::Info,
        )?;
        MechanicReply {
            state: SystemState::Healthy,
            response: "Manual fix acknowledged. Gateway fallback retired, the peer mesh stabilized, and lanes returned to nominal pressure.".to_string(),
            actions: vec![
                "Run `status` to confirm healthy posture.".to_string(),
                "Inspect Library to verify relay preference is back on direct lanes.".to_string(),
            ],
        }
    } else if lowered.contains("scan") || lowered.contains("diagnose") {
        emit_log(
            &app,
            "MECHANIC",
            "Deep scan initiated across bridge latency, retrieval pressure, and the last repair fingerprint.",
            LogLevel::Info,
        )?;
        MechanicReply {
            state: SystemState::Processing,
            response: "Deep scan initiated. The mechanic is tracing bridge latency, RAG lane pressure, and self-heal replay candidates.".to_string(),
            actions: vec![
                "Watch the Engine Room console for the next telemetry burst.".to_string(),
                "Use `replay last fix` if this is a recurring incident.".to_string(),
            ],
        }
    } else if lowered.contains("replay") {
        emit_log(
            &app,
            "MECHANIC",
            "Replaying the last stable repair pack against the current gateway posture.",
            LogLevel::Info,
        )?;
        MechanicReply {
            state: if current == SystemState::Critical {
                SystemState::Rescue
            } else {
                SystemState::Healthy
            },
            response:
                "Replayed the last known stabilization path against the current gateway posture."
                    .to_string(),
            actions: vec![
                "Inspect Bridges for relay pressure.".to_string(),
                "Run `stabilize gateway` if rescue posture should be cleared.".to_string(),
            ],
        }
    } else if lowered.contains("critical") || lowered.contains("containment") {
        emit_log(
            &app,
            "MECHANIC",
            "Containment posture raised. Restricting direct bridges until the operator clears the incident.",
            LogLevel::Error,
        )?;
        MechanicReply {
            state: SystemState::Critical,
            response: "Containment posture raised. Direct bridges are restricted until an operator stabilizes the runtime.".to_string(),
            actions: vec![
                "Inspect the bridge mesh for isolated peers.".to_string(),
                "Run `stabilize gateway` after review.".to_string(),
            ],
        }
    } else if lowered.contains("status") {
        emit_log(
            &app,
            "MECHANIC",
            format!(
                "Mechanic status requested while shell posture is {:?}.",
                current
            ),
            LogLevel::Info,
        )?;
        MechanicReply {
            state: current.clone(),
            response: format!(
                "Current shell posture is {:?}. Main mesh health is {}.",
                current,
                if current == SystemState::Healthy {
                    "stable"
                } else {
                    "elevated"
                }
            ),
            actions: vec![
                "Run `scan gateway` for a deeper read.".to_string(),
                "Run `stabilize gateway` to clear rescue posture.".to_string(),
            ],
        }
    } else if lowered.contains("fallback") || lowered.contains("rescue") {
        emit_log(
            &app,
            "ROUTER",
            "Relay fallback was pinned manually by the operator.",
            LogLevel::Warn,
        )?;
        MechanicReply {
            state: SystemState::Rescue,
            response:
                "Rescue posture pinned. The shell will prefer relay-safe routing until cleared."
                    .to_string(),
            actions: vec![
                "Run `scan gateway` to inspect the incident.".to_string(),
                "Run `stabilize gateway` when the operator wants to exit rescue mode.".to_string(),
            ],
        }
    } else {
        emit_log(
            &app,
            "MECHANIC",
            "Command parsed, but the mechanic needs a clearer directive.",
            LogLevel::Warn,
        )?;
        MechanicReply {
            state: current.clone(),
            response:
                "Mechanic command parsed, but it needs a clearer directive. Try `status`, `scan gateway`, `stabilize gateway`, or `replay last fix`."
                    .to_string(),
            actions: Vec::new(),
        }
    };

    *lock_state(&state.system_state)? = reply.state.clone();
    Ok(reply)
}

pub fn build_library_snapshot(
    system_state: &SystemState,
    niche_modules: &[NicheModule],
) -> LibrarySnapshot {
    let mut collections = vec![
        RagCollection {
            id: "main".to_string(),
            label: "Main".to_string(),
            group: "SHARED".to_string(),
            chunk_count: 58,
            document_count: 11,
            accent: "#93c5fd".to_string(),
            summary: "Compact shared memory for project notes, runtime state, and operator reference material.".to_string(),
        },
        RagCollection {
            id: "project".to_string(),
            label: "Project".to_string(),
            group: "DOMAIN".to_string(),
            chunk_count: 16,
            document_count: 3,
            accent: "#14b8a6".to_string(),
            summary: "Local project notes, decisions, and implementation references.".to_string(),
        },
    ];

    for module in niche_modules {
        let seed = hash_seed(&module.id);
        collections.push(RagCollection {
            id: module.collection_id.clone(),
            label: module.label.clone(),
            group: "CUSTOM".to_string(),
            chunk_count: 7 + (seed % 14) as usize,
            document_count: 2 + (seed % 5) as usize,
            accent: module.accent.clone(),
            summary: format!(
                "{} is staged as a modular lane with its own config and future retrieval carve-out.",
                module.label
            ),
        });
    }

    let nodes = build_swarm_nodes(system_state, niche_modules);
    LibrarySnapshot {
        collections,
        p2p_status: P2pStatus {
            mode: match system_state {
                SystemState::Healthy => "Post-quantum mesh".to_string(),
                SystemState::Processing => "Adaptive sync mesh".to_string(),
                SystemState::Rescue => "Fallback relay mesh".to_string(),
                SystemState::Critical => "Containment mesh".to_string(),
            },
            health: match system_state {
                SystemState::Healthy => "STABLE".to_string(),
                SystemState::Processing => "SYNCING".to_string(),
                SystemState::Rescue => "DEGRADED".to_string(),
                SystemState::Critical => "CRITICAL".to_string(),
            },
            relay: match system_state {
                SystemState::Healthy | SystemState::Processing => "Direct preferred".to_string(),
                SystemState::Rescue | SystemState::Critical => "Relay preferred".to_string(),
            },
            summary: match system_state {
                SystemState::Healthy => {
                    "Peer bridges are synchronized and ready to carry niche memory slices."
                        .to_string()
                }
                SystemState::Processing => {
                    "The mesh is rebalancing while new tasks and mechanic scans settle."
                        .to_string()
                }
                SystemState::Rescue => {
                    "The swarm shifted to rescue routing while the mechanic inspects gateway pressure."
                        .to_string()
                }
                SystemState::Critical => {
                    "Containment routing is active. Manual stabilization is recommended."
                        .to_string()
                }
            },
            connected_peers: nodes.iter().filter(|node| node.status != "ISOLATED").count(),
            nodes,
        },
        last_sync: Utc::now().to_rfc3339(),
    }
}

fn build_swarm_nodes(system_state: &SystemState, niche_modules: &[NicheModule]) -> Vec<SwarmNode> {
    let custom: Vec<String> = niche_modules
        .iter()
        .map(|module| module.collection_id.clone())
        .collect();

    vec![
        SwarmNode {
            id: "openai-bridge".to_string(),
            label: "OpenAI Bridge".to_string(),
            status: if *system_state == SystemState::Critical {
                "ISOLATED".to_string()
            } else {
                "ONLINE".to_string()
            },
            transport: "PQ-ACTIVE".to_string(),
            latency_ms: if *system_state == SystemState::Rescue {
                58
            } else {
                32
            },
            collections: vec![
                "main".to_string(),
                "workflow".to_string(),
                custom.first().cloned().unwrap_or_else(|| "seo".to_string()),
            ],
            zone: "Inference".to_string(),
        },
        SwarmNode {
            id: "anthropic-bridge".to_string(),
            label: "Anthropic Bridge".to_string(),
            status: if *system_state == SystemState::Processing {
                "SYNCING".to_string()
            } else {
                "ONLINE".to_string()
            },
            transport: "PQ-ACTIVE".to_string(),
            latency_ms: if *system_state == SystemState::Processing {
                64
            } else {
                38
            },
            collections: vec![
                "main".to_string(),
                "workflow".to_string(),
                "seo".to_string(),
            ],
            zone: "Analysis".to_string(),
        },
        SwarmNode {
            id: "hetzner-ssh".to_string(),
            label: "Hetzner SSH".to_string(),
            status: if *system_state == SystemState::Rescue {
                "DEGRADED".to_string()
            } else {
                "ONLINE".to_string()
            },
            transport: "TCP-NOISE".to_string(),
            latency_ms: if *system_state == SystemState::Rescue {
                86
            } else {
                52
            },
            collections: vec!["infra".to_string(), "ops".to_string()],
            zone: "Remote".to_string(),
        },
        SwarmNode {
            id: "nat-cloud-p2p".to_string(),
            label: "NAT Cloud P2P".to_string(),
            status: if *system_state == SystemState::Rescue {
                "SYNCING".to_string()
            } else {
                "ONLINE".to_string()
            },
            transport: "RELAY-MESH".to_string(),
            latency_ms: if *system_state == SystemState::Rescue {
                94
            } else {
                47
            },
            collections: vec![
                "seo".to_string(),
                custom.get(1).cloned().unwrap_or_else(|| "ops".to_string()),
            ],
            zone: "Mesh".to_string(),
        },
    ]
}

#[tauri::command]
pub fn list_local_modes() -> Result<AssistantModesResponse, String> {
    let mut seen = BTreeSet::new();
    let mut modes = Vec::new();

    for dir in user_file_dirs(
        "ORDO_MODES_PATH",
        &["user-files/modes", "ordo-studio/user-files/modes"],
    )? {
        if !dir.exists() {
            continue;
        }
        for entry in fs::read_dir(&dir).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let content = fs::read_to_string(&path).map_err(|error| error.to_string())?;
            let mut mode: AssistantModeManifest = serde_json::from_str(&content)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            if mode.id.trim().is_empty() {
                mode.id = path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("mode")
                    .to_string();
            }
            if mode.label.trim().is_empty() {
                mode.label = mode.id.clone();
            }
            if seen.insert(mode.id.clone()) {
                modes.push(mode);
            }
        }
    }

    modes.sort_by(|left, right| left.label.cmp(&right.label));
    Ok(AssistantModesResponse {
        count: modes.len(),
        modes,
    })
}

#[tauri::command]
pub fn list_local_plugins() -> Result<PluginsResponse, String> {
    let mut seen = BTreeSet::new();
    let mut plugins = Vec::new();

    for dir in user_file_dirs(
        "ORDO_PLUGINS_PATH",
        &["user-files/plugins", "ordo-studio/user-files/plugins"],
    )? {
        if !dir.exists() {
            continue;
        }
        for entry in fs::read_dir(&dir).map_err(|error| error.to_string())? {
            let plugin_dir = entry.map_err(|error| error.to_string())?.path();
            if !plugin_dir.is_dir() {
                continue;
            }
            let manifest_path = plugin_dir.join("plugin.json");
            if !manifest_path.exists() {
                continue;
            }
            let content = fs::read_to_string(&manifest_path).map_err(|error| error.to_string())?;
            let manifest: PluginManifest = serde_json::from_str(&content)
                .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
            let fallback_name = plugin_dir
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("plugin")
                .to_string();
            let name = if manifest.name.trim().is_empty() {
                fallback_name
            } else {
                manifest.name.clone()
            };
            if !seen.insert(name.clone()) {
                continue;
            }
            plugins.push(PluginStatus {
                name,
                version: manifest.version,
                description: manifest.description,
                state: if manifest.enabled {
                    "Active"
                } else {
                    "Disabled"
                }
                .to_string(),
                tool_count: 0,
                expected_lanes: manifest.expected_lanes,
                enabled: manifest.enabled,
                command: manifest.command,
                args: manifest.args,
                required_env: manifest.required_env,
                env: manifest.env,
                core_override: manifest.core_override,
                manifest_path: relative_config_path(&manifest_path),
                failure: None,
            });
        }
    }

    plugins.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(PluginsResponse {
        count: plugins.len(),
        plugins,
    })
}

#[tauri::command]
pub fn install_local_plugin(manifest: PluginManifest) -> Result<PluginStatus, String> {
    let manifest = validate_plugin_manifest(manifest)?;
    let plugin_root = writable_plugin_root()?;
    fs::create_dir_all(&plugin_root).map_err(|error| error.to_string())?;
    let plugin_dir = plugin_root.join(&manifest.name);
    if plugin_dir.exists() {
        return Err(format!("plugin '{}' already exists", manifest.name));
    }
    fs::create_dir_all(&plugin_dir).map_err(|error| error.to_string())?;
    write_plugin_manifest(&plugin_dir.join("plugin.json"), &manifest)?;
    read_plugin_status(&plugin_dir.join("plugin.json"))
}

#[tauri::command]
pub fn update_local_plugin(name: String, manifest: PluginManifest) -> Result<PluginStatus, String> {
    let current_path = find_plugin_manifest_path(&name)?;
    let manifest = validate_plugin_manifest(manifest)?;
    if manifest.name != name {
        return Err(
            "plugin name cannot be changed; delete and reinstall with the new name".to_string(),
        );
    }
    write_plugin_manifest(&current_path, &manifest)?;
    read_plugin_status(&current_path)
}

#[tauri::command]
pub fn set_local_plugin_enabled(name: String, enabled: bool) -> Result<PluginStatus, String> {
    let manifest_path = find_plugin_manifest_path(&name)?;
    let content = fs::read_to_string(&manifest_path).map_err(|error| error.to_string())?;
    let mut manifest: PluginManifest = serde_json::from_str(&content)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    manifest.enabled = enabled;
    let manifest = validate_plugin_manifest(manifest)?;
    write_plugin_manifest(&manifest_path, &manifest)?;
    read_plugin_status(&manifest_path)
}

#[tauri::command]
pub fn delete_local_plugin(name: String) -> Result<Value, String> {
    let manifest_path = find_plugin_manifest_path(&name)?;
    let plugin_dir = manifest_path
        .parent()
        .ok_or_else(|| "plugin manifest has no parent directory".to_string())?;
    ensure_deletable_plugin_dir(plugin_dir)?;
    fs::remove_dir_all(plugin_dir).map_err(|error| error.to_string())?;
    Ok(json!({
        "deleted": true,
        "name": name
    }))
}

#[tauri::command]
pub fn get_local_skill(id: String) -> Result<Value, String> {
    let skill_path = find_skill_file_path(&id)?;
    let content = fs::read_to_string(&skill_path).map_err(|error| error.to_string())?;
    Ok(json!({
        "id": id,
        "content": content,
        "path": relative_config_path(&skill_path)
    }))
}

#[tauri::command]
pub fn update_local_skill(id: String, content: String) -> Result<Value, String> {
    let skill_path = find_skill_file_path(&id)?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err("skill content cannot be empty".to_string());
    }
    fs::write(&skill_path, format!("{trimmed}\n")).map_err(|error| error.to_string())?;
    Ok(json!({
        "id": id,
        "updated": true,
        "path": relative_config_path(&skill_path)
    }))
}

#[tauri::command]
pub fn delete_local_skill(id: String) -> Result<Value, String> {
    let skill_path = find_skill_file_path(&id)?;
    let skill_dir = skill_path
        .parent()
        .ok_or_else(|| "skill file has no parent directory".to_string())?;
    ensure_deletable_skill_dir(skill_dir)?;
    fs::remove_dir_all(skill_dir).map_err(|error| error.to_string())?;
    Ok(json!({
        "deleted": true,
        "id": id
    }))
}

#[tauri::command]
pub fn list_local_mcp_servers() -> Result<McpServersResponse, String> {
    let mut servers = Vec::new();
    for (server_id, manifest) in local_mcp_manifests()? {
        let tool_count = manifest
            .get("tool_catalog")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        servers.push(McpServerStatus {
            server_id,
            trust_state: "Available".to_string(),
            installed_at: String::new(),
            tool_count,
            privilege_tier: Some("local package".to_string()),
            drift: None,
            lockfile_hash: None,
        });
    }
    servers.sort_by(|left, right| left.server_id.cmp(&right.server_id));
    Ok(McpServersResponse {
        count: servers.len(),
        servers,
    })
}

#[tauri::command]
pub fn list_local_mcp_capabilities() -> Result<CapabilitiesResponse, String> {
    let descriptors = local_mcp_capability_descriptors()?;
    Ok(CapabilitiesResponse {
        count: descriptors.len(),
        descriptors,
    })
}

fn local_mcp_capability_descriptors() -> Result<Vec<CapabilityDescriptor>, String> {
    let mut descriptors = Vec::new();
    for (server_id, manifest) in local_mcp_manifests()? {
        let label = manifest
            .pointer("/identity/name")
            .and_then(Value::as_str)
            .unwrap_or(&server_id)
            .to_string();
        if let Some(tools) = manifest.get("tool_catalog").and_then(Value::as_array) {
            for tool in tools {
                let Some(name) = tool.get("name").and_then(Value::as_str) else {
                    continue;
                };
                descriptors.push(CapabilityDescriptor {
                    capability: name.to_string(),
                    description: tool
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("Local MCP capability")
                        .to_string(),
                    provider: server_id.clone(),
                    tier: "Optional".to_string(),
                    activation: "Lazy".to_string(),
                    lane: CapabilityLane {
                        group: "interface".to_string(),
                        name: "mcp".to_string(),
                        label: label.clone(),
                    },
                    input_schema: tool.get("input_schema").cloned(),
                });
            }
        }
    }
    descriptors.sort_by(|left, right| left.capability.cmp(&right.capability));
    Ok(descriptors)
}

#[tauri::command]
pub fn list_local_capabilities() -> Result<CapabilitiesResponse, String> {
    let mut descriptors = local_mcp_capability_descriptors()?;

    descriptors.extend(local_skill_descriptors()?);

    descriptors.push(CapabilityDescriptor {
        capability: "ordo.plugins.manage".to_string(),
        description: "Install, edit, pause, resume, and delete Ordo plugins from the native Plugin tab while keeping plugins separate from MCP servers.".to_string(),
        provider: "ordo-desktop".to_string(),
        tier: "Core".to_string(),
        activation: "Eager".to_string(),
        lane: CapabilityLane {
            group: "interface".to_string(),
            name: "plugins".to_string(),
            label: "Plugin Management".to_string(),
        },
        input_schema: Some(json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["install", "edit", "pause", "resume", "delete"]
                },
                "plugin_name": {
                    "type": "string"
                }
            },
            "required": ["action", "plugin_name"]
        })),
    });

    if let Ok(plugin_inventory) = list_local_plugins() {
        for plugin in plugin_inventory.plugins {
            for lane in plugin.expected_lanes {
                descriptors.push(CapabilityDescriptor {
                    capability: format!("{}*", lane.trim_end_matches('.')),
                    description: plugin.description.clone(),
                    provider: plugin.name.clone(),
                    tier: "Optional".to_string(),
                    activation: "Lazy".to_string(),
                    lane: CapabilityLane {
                        group: "interface".to_string(),
                        name: "plugin".to_string(),
                        label: plugin.name.clone(),
                    },
                    input_schema: None,
                });
            }
        }
    }

    descriptors.sort_by(|left, right| left.capability.cmp(&right.capability));
    Ok(CapabilitiesResponse {
        count: descriptors.len(),
        descriptors,
    })
}

fn local_skill_descriptors() -> Result<Vec<CapabilityDescriptor>, String> {
    let mut descriptors = Vec::new();
    let mut seen = BTreeSet::new();

    for dir in user_file_dirs(
        "ORDO_SKILLS_PATH",
        &["user-files/skills", "ordo-studio/user-files/skills"],
    )? {
        if !dir.exists() {
            continue;
        }
        for entry in fs::read_dir(&dir).map_err(|error| error.to_string())? {
            let skill_dir = entry.map_err(|error| error.to_string())?.path();
            if !skill_dir.is_dir() {
                continue;
            }
            let skill_path = skill_dir.join("skill.md");
            if !skill_path.exists() {
                continue;
            }
            let id = skill_dir
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("skill")
                .trim()
                .to_string();
            if id.is_empty() || !seen.insert(id.clone()) {
                continue;
            }
            let content = fs::read_to_string(&skill_path).map_err(|error| error.to_string())?;
            descriptors.push(CapabilityDescriptor {
                capability: id.clone(),
                description: skill_loader_description(&content),
                provider: "ordo-skill".to_string(),
                tier: "Optional".to_string(),
                activation: "Lazy".to_string(),
                lane: CapabilityLane {
                    group: "interface".to_string(),
                    name: "skill".to_string(),
                    label: skill_lane_label(&content),
                },
                input_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "mode": {
                            "type": "string",
                            "enum": ["quick_scan", "full_forensic_analysis", "argument_strengthening", "opposition_analysis"]
                        },
                        "text": {
                            "type": "string"
                        }
                    },
                    "required": ["text"]
                })),
            });
        }
    }

    descriptors.sort_by(|left, right| left.capability.cmp(&right.capability));
    Ok(descriptors)
}

fn skill_loader_description(content: &str) -> String {
    extract_markdown_section(content, "## Loader Hook")
        .or_else(|| extract_markdown_section(content, "## Purpose"))
        .unwrap_or_else(|| "Operator-installed procedural skill.".to_string())
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("```"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn skill_lane_label(content: &str) -> String {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("lane:") {
            let label = value.trim();
            if !label.is_empty() {
                return label.to_string();
            }
        }
    }
    "Installed Skills".to_string()
}

fn extract_markdown_section(content: &str, heading: &str) -> Option<String> {
    let mut collecting = false;
    let mut lines = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == heading {
            collecting = true;
            continue;
        }
        if collecting && trimmed.starts_with("## ") {
            break;
        }
        if collecting {
            lines.push(line);
        }
    }
    let out = lines.join("\n").trim().to_string();
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[tauri::command]
pub fn get_local_runtime_profile() -> Result<Value, String> {
    Ok(local_runtime_profile())
}

#[tauri::command]
pub fn get_local_runtime_storage() -> Result<Value, String> {
    Ok(local_runtime_storage())
}

#[tauri::command]
pub fn get_local_runtime_settings() -> Result<Value, String> {
    Ok(json!({
        "effective": {
            "profile": "standard",
            "rag_enabled": false,
            "rag_activation_mode": "lazy",
            "knowledge_enabled": true,
            "knowledge_activation_mode": "lazy",
            "embedding_backend": "native",
            "embedding_dimensions": 0,
            "llama_cpp_configured": false,
            "control_api_bind": "tauri-native",
            "control_api_enabled": false,
            "rag_budget_bytes": 268435456u64,
            "memory_pinned_budget_bytes": 268435456u64,
            "memory_working_budget_bytes": 134217728u64,
            "self_heal_history_budget_bytes": 268435456u64,
            "self_heal_model_context_size": 8192u64,
            "self_heal_model_max_tokens": 1024u64,
            "self_heal_model_temperature": 0.2
        },
        "persisted": {}
    }))
}

#[tauri::command]
pub fn list_local_rag_collections() -> Result<Value, String> {
    Ok(json!({
        "collections": [
            {
                "name": "main",
                "label": "Main Knowledge",
                "group": "shared",
                "document_count": 0,
                "chunk_count": 0,
                "sample_titles": []
            },
            {
                "name": "project",
                "label": "Project Notes",
                "group": "domain",
                "document_count": 0,
                "chunk_count": 0,
                "sample_titles": []
            },
            {
                "name": "ops",
                "label": "Runtime Ops",
                "group": "interface",
                "document_count": 0,
                "chunk_count": 0,
                "sample_titles": []
            }
        ]
    }))
}

#[tauri::command]
pub fn preview_local_rag_collections(query: String) -> Result<Value, String> {
    let _ = query;
    Ok(json!({
        "effective_collections": ["main", "project", "ops"],
        "effective_collection_labels": ["Main Knowledge", "Project Notes", "Runtime Ops"],
        "hit_count": 0,
        "hits": []
    }))
}

#[tauri::command]
pub fn list_local_pinned_memory(limit: usize) -> Result<Value, String> {
    let _ = limit;
    Ok(json!([]))
}

#[tauri::command]
pub fn list_local_working_memory(limit: usize) -> Result<Value, String> {
    let _ = limit;
    Ok(json!([]))
}

#[tauri::command]
pub fn list_local_cloud_credentials() -> Result<Value, String> {
    Ok(json!({
        "credentials": [],
        "count": 0
    }))
}

#[tauri::command]
pub fn install_local_api_key_env(
    env_var: String,
    api_key: String,
) -> Result<LocalApiKeyInstallResult, String> {
    let name = validate_operator_env_var(&env_var)?;
    let secret = api_key.trim();
    if secret.is_empty() {
        return Err("API key cannot be empty.".to_string());
    }
    set_current_process_env(&name, secret);
    let local_env_path = persist_ordo_local_env_var(&name, secret)?;
    let installed_for = persist_platform_env_var(&name, secret)?;
    Ok(LocalApiKeyInstallResult {
        env_var: name,
        platform: platform_label().to_string(),
        installed_for,
        local_env_path: local_env_path.to_string_lossy().to_string(),
        current_process_ready: true,
        restart_recommended: true,
    })
}

fn validate_operator_env_var(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Environment variable name is required.".to_string());
    }
    if trimmed.len() > 128 {
        return Err("Environment variable name is too long.".to_string());
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
    {
        return Err("Use uppercase letters, numbers, and underscores only.".to_string());
    }
    if !trimmed.ends_with("_API_KEY") && trimmed != "OPENAI_API_KEY" {
        return Err("API key environment variables must end with _API_KEY.".to_string());
    }
    Ok(trimmed.to_string())
}

fn set_current_process_env(name: &str, secret: &str) {
    unsafe {
        env::set_var(name, secret);
    }
}

#[cfg(windows)]
fn persist_platform_env_var(name: &str, secret: &str) -> Result<String, String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (env_key, _) = hkcu
        .create_subkey("Environment")
        .map_err(|error| format!("failed to open user environment registry key: {error}"))?;
    env_key
        .set_value(name, &secret)
        .map_err(|error| format!("failed to save environment variable: {error}"))?;
    Ok("Windows user environment and Ordo local environment".to_string())
}

#[cfg(not(windows))]
fn persist_platform_env_var(_name: &str, _secret: &str) -> Result<String, String> {
    Ok("Ordo local environment".to_string())
}

fn persist_ordo_local_env_var(name: &str, secret: &str) -> Result<PathBuf, String> {
    let path = ordo_local_env_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut values = if path.exists() {
        let raw = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        serde_json::from_str::<BTreeMap<String, String>>(&raw).unwrap_or_default()
    } else {
        BTreeMap::new()
    };
    values.insert(name.to_string(), secret.to_string());
    let serialized = serde_json::to_string_pretty(&values).map_err(|error| error.to_string())?;
    fs::write(&path, serialized).map_err(|error| error.to_string())?;
    restrict_secret_file_permissions(&path)?;
    Ok(path)
}

fn ordo_local_env_path() -> Result<PathBuf, String> {
    if cfg!(windows) {
        if let Some(appdata) = env::var_os("APPDATA") {
            return Ok(PathBuf::from(appdata)
                .join("Ordo")
                .join("env")
                .join("api-keys.json"));
        }
        if let Some(home) = env::var_os("USERPROFILE") {
            return Ok(PathBuf::from(home)
                .join("AppData")
                .join("Roaming")
                .join("Ordo")
                .join("env")
                .join("api-keys.json"));
        }
    } else if let Some(config_home) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(config_home)
            .join("ordo")
            .join("env")
            .join("api-keys.json"));
    } else if let Some(home) = env::var_os("HOME") {
        return Ok(PathBuf::from(home)
            .join(".config")
            .join("ordo")
            .join("env")
            .join("api-keys.json"));
    }
    Err("could not resolve local Ordo configuration directory".to_string())
}

#[cfg(unix)]
fn restrict_secret_file_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .map_err(|error| error.to_string())?
        .permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn restrict_secret_file_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn platform_label() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    }
}

#[tauri::command]
pub fn detect_local_llm(provider: String) -> Result<Value, String> {
    let (base_url, host, port) = match provider.as_str() {
        "ollama" => ("http://localhost:11434/v1", "127.0.0.1", 11434u16),
        "lmstudio" => ("http://localhost:1234/v1", "127.0.0.1", 1234u16),
        other => {
            return Ok(json!({
                "provider": other,
                "base_url": "",
                "reachable": false,
                "models": [],
                "error": format!("unknown local provider: {other}")
            }));
        }
    };

    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|error| format!("invalid local provider address: {error}"))?;
    let timeout = Duration::from_millis(2500);
    let mut stream = match TcpStream::connect_timeout(&addr, timeout) {
        Ok(stream) => stream,
        Err(error) => {
            return Ok(json!({
                "provider": provider,
                "base_url": base_url,
                "reachable": false,
                "models": [],
                "error": error.to_string()
            }));
        }
    };
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| error.to_string())?;

    let request = format!(
        "GET /v1/models HTTP/1.1\r\nHost: localhost:{port}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    if let Err(error) = stream.write_all(request.as_bytes()) {
        return Ok(json!({
            "provider": provider,
            "base_url": base_url,
            "reachable": false,
            "models": [],
            "error": error.to_string()
        }));
    }

    let mut response = String::new();
    if let Err(error) = stream.read_to_string(&mut response) {
        return Ok(json!({
            "provider": provider,
            "base_url": base_url,
            "reachable": false,
            "models": [],
            "error": error.to_string()
        }));
    }

    let status_ok = response
        .lines()
        .next()
        .map(|line| line.contains(" 200 "))
        .unwrap_or(false);
    if !status_ok {
        let status = response.lines().next().unwrap_or("no HTTP status");
        return Ok(json!({
            "provider": provider,
            "base_url": base_url,
            "reachable": false,
            "models": [],
            "error": status
        }));
    }

    let body = response
        .split("\r\n\r\n")
        .nth(1)
        .or_else(|| response.split("\n\n").nth(1))
        .unwrap_or("")
        .trim();
    let parsed: Value = serde_json::from_str(body)
        .map_err(|error| format!("local provider returned invalid model JSON: {error}"))?;
    let models = parsed
        .get("data")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("id").and_then(Value::as_str))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(json!({
        "provider": provider,
        "base_url": base_url,
        "reachable": true,
        "models": models
    }))
}

#[tauri::command]
pub fn list_local_webhooks() -> Result<Value, String> {
    Ok(json!({
        "subscriptions": []
    }))
}

#[tauri::command]
pub fn list_local_connection_types() -> Result<Value, String> {
    Ok(json!({
        "types": [
            {
                "id": "openai",
                "label": "OpenAI",
                "description": "OpenAI-compatible model endpoint.",
                "service": "openai",
                "fields": [
                    { "key": "base_url", "label": "Base URL", "required": false, "secret": false },
                    { "key": "api_key", "label": "API key", "required": true, "secret": true },
                    { "key": "model", "label": "Model", "required": false, "secret": false }
                ]
            },
            {
                "id": "anthropic",
                "label": "Anthropic",
                "description": "Anthropic Messages API credential.",
                "service": "anthropic",
                "fields": [
                    { "key": "api_key", "label": "API key", "required": true, "secret": true },
                    { "key": "model", "label": "Model", "required": false, "secret": false }
                ]
            },
            {
                "id": "local_openai",
                "label": "Local OpenAI API",
                "description": "Local model server with an OpenAI-compatible API.",
                "service": "local_openai",
                "fields": [
                    { "key": "base_url", "label": "Base URL", "required": true, "secret": false },
                    { "key": "model", "label": "Model", "required": false, "secret": false }
                ]
            },
            {
                "id": "ssh",
                "label": "SSH",
                "description": "Remote host access for infrastructure tasks.",
                "service": "ssh",
                "fields": [
                    { "key": "host", "label": "Host", "required": true, "secret": false },
                    { "key": "username", "label": "Username", "required": true, "secret": false },
                    { "key": "private_key", "label": "Private key", "required": false, "secret": true }
                ]
            },
            {
                "id": "email",
                "label": "Email",
                "description": "IMAP inbox polling and SMTP replies for Ordo command intake.",
                "service": "email",
                "fields": [
                    { "key": "email_address", "label": "Email address", "required": true, "secret": false },
                    { "key": "display_name", "label": "Display name", "required": false, "secret": false },
                    { "key": "imap_host", "label": "IMAP host", "required": true, "secret": false },
                    { "key": "imap_port", "label": "IMAP port", "required": false, "secret": false },
                    { "key": "smtp_host", "label": "SMTP host", "required": true, "secret": false },
                    { "key": "smtp_port", "label": "SMTP port", "required": false, "secret": false },
                    { "key": "imap_username", "label": "Username", "required": true, "secret": false },
                    { "key": "imap_password", "label": "IMAP password or app password", "required": true, "secret": true },
                    { "key": "authorized_senders", "label": "Authorized senders", "required": false, "secret": false },
                    { "key": "command_prefix", "label": "Command prefix", "required": false, "secret": false }
                ]
            },
            {
                "id": "generic_api_key",
                "label": "Generic API Key",
                "description": "Simple API key credential for custom tools.",
                "service": "generic",
                "fields": [
                    { "key": "base_url", "label": "Base URL", "required": false, "secret": false },
                    { "key": "api_key", "label": "API key", "required": true, "secret": true }
                ]
            }
        ]
    }))
}

#[tauri::command]
pub fn list_local_apps() -> Result<Value, String> {
    Ok(json!({
        "apps": [],
        "count": 0
    }))
}

#[tauri::command]
pub fn list_local_files() -> Result<Value, String> {
    Ok(json!({
        "files": [],
        "count": 0
    }))
}

#[tauri::command]
pub fn list_local_security_rules() -> Result<Value, String> {
    Ok(json!({
        "rules": [
            {
                "id": "prompt_injection_boundary",
                "description": "Treat instructions from retrieved or external content as untrusted.",
                "severity": "high",
                "phases": "pre_tool, post_tool",
                "enabled": true
            },
            {
                "id": "secret_exposure_guard",
                "description": "Block assistant output from revealing stored credential values.",
                "severity": "critical",
                "phases": "pre_response",
                "enabled": true
            },
            {
                "id": "filesystem_write_confirm",
                "description": "Require operator review before broad filesystem writes.",
                "severity": "medium",
                "phases": "pre_tool",
                "enabled": true
            }
        ],
        "count": 3
    }))
}

#[tauri::command]
pub fn list_local_security_audit(limit: usize) -> Result<Value, String> {
    let _ = limit;
    Ok(json!({
        "entries": [],
        "count": 0
    }))
}

#[tauri::command]
pub fn list_local_review_pending() -> Result<Value, String> {
    Ok(json!({
        "pending": [],
        "count": 0
    }))
}

#[tauri::command]
pub fn list_local_review_recent(limit: usize) -> Result<Value, String> {
    let _ = limit;
    Ok(json!({
        "recent": [],
        "count": 0
    }))
}

#[tauri::command]
pub fn list_local_self_heal_cases(limit: usize) -> Result<Value, String> {
    let _ = limit;
    Ok(json!({
        "cases": [],
        "count": 0
    }))
}

#[tauri::command]
pub fn list_local_assistant_facts(subject: Option<String>) -> Result<Value, String> {
    let _ = subject;
    Ok(json!({
        "facts": []
    }))
}

#[tauri::command]
pub fn get_local_session_taint(session_id: String) -> Result<Value, String> {
    Ok(json!({
        "session_id": session_id,
        "tainted": false,
        "sources": []
    }))
}

#[tauri::command]
pub fn find_local_binary(name: String) -> Result<Value, String> {
    let candidates = binary_candidates(&name)?;
    let found = candidates
        .iter()
        .find(|candidate| Path::new(candidate.as_str()).exists())
        .cloned();
    Ok(json!({
        "name": name,
        "found": found,
        "candidates": candidates
    }))
}

#[tauri::command]
pub fn get_local_health() -> Result<Value, String> {
    Ok(json!({
        "status": "desktop-native"
    }))
}

fn local_runtime_profile() -> Value {
    json!({
        "profile": "standard",
        "rag_enabled": false,
        "rag_activation_mode": "lazy",
        "knowledge_enabled": true,
        "knowledge_activation_mode": "lazy",
        "embedding_backend": "native",
        "embedding_dimensions": 0,
        "llama_cpp_configured": false,
        "control_api_bind": "tauri-native",
        "control_api_enabled": false
    })
}

fn local_runtime_storage() -> Value {
    json!({
        "rag_budget_bytes": 268435456u64,
        "memory_pinned_budget_bytes": 268435456u64,
        "memory_working_budget_bytes": 134217728u64,
        "self_heal_history_budget_bytes": 268435456u64,
        "self_heal_model_context_size": 8192u64,
        "self_heal_model_max_tokens": 1024u64,
        "self_heal_model_temperature": 0.2
    })
}

fn binary_candidates(name: &str) -> Result<Vec<String>, String> {
    let root = repo_root()?;
    let names = binary_names(name);
    let mut candidates = Vec::new();

    for base in [
        root.clone(),
        root.join("target").join("debug"),
        root.join("target").join("release"),
        root.join("ordo-studio")
            .join("src-tauri")
            .join("target")
            .join("debug"),
        root.join("ordo-studio")
            .join("src-tauri")
            .join("target")
            .join("release"),
    ] {
        for binary_name in &names {
            candidates.push(base.join(binary_name).display().to_string());
        }
    }

    if let Some(paths) = env::var_os("PATH") {
        for dir in env::split_paths(&paths) {
            for binary_name in &names {
                candidates.push(dir.join(binary_name).display().to_string());
            }
        }
    }

    Ok(dedup_strings(candidates))
}

fn binary_names(name: &str) -> Vec<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let mut names = vec![trimmed.to_string()];
    if cfg!(windows) && Path::new(trimmed).extension().is_none() {
        names.push(format!("{trimmed}.exe"));
    }
    dedup_strings(names)
}

fn find_skill_file_path(id: &str) -> Result<PathBuf, String> {
    let target = normalize_skill_id(id)?;
    for dir in user_file_dirs(
        "ORDO_SKILLS_PATH",
        &["user-files/skills", "ordo-studio/user-files/skills"],
    )? {
        if !dir.exists() {
            continue;
        }
        let skill_path = dir.join(&target).join("skill.md");
        if skill_path.exists() {
            return Ok(skill_path);
        }
    }
    Err(format!("skill '{target}' was not found"))
}

fn normalize_skill_id(id: &str) -> Result<String, String> {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return Err("skill id is required".to_string());
    }
    if trimmed.contains(['/', '\\']) || trimmed.contains("..") {
        return Err("skill id must be a single directory name".to_string());
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-')
    {
        return Err(
            "skill id must use lowercase letters, digits, underscores, or hyphens".to_string(),
        );
    }
    Ok(trimmed.to_string())
}

fn validate_plugin_manifest(mut manifest: PluginManifest) -> Result<PluginManifest, String> {
    manifest.name = manifest.name.trim().to_ascii_lowercase();
    manifest.version = manifest.version.trim().to_string();
    manifest.description = manifest.description.trim().to_string();
    manifest.command = manifest.command.trim().to_string();
    manifest.expected_lanes = normalize_plugin_list(manifest.expected_lanes);
    manifest.required_env = normalize_plugin_list(manifest.required_env);
    manifest.args = manifest
        .args
        .into_iter()
        .map(|arg| arg.trim().to_string())
        .filter(|arg| !arg.is_empty())
        .collect();

    if manifest.version.is_empty() {
        manifest.version = default_plugin_version();
    }
    if manifest.name.is_empty() {
        return Err("plugin name is required".to_string());
    }
    if !manifest
        .name
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
    {
        return Err(
            "plugin name must use lowercase letters, digits, hyphens, or underscores".to_string(),
        );
    }
    if manifest.command.is_empty() {
        return Err("plugin command is required".to_string());
    }
    if manifest.expected_lanes.is_empty() {
        return Err("plugin must advertise at least one expected lane".to_string());
    }
    if manifest
        .expected_lanes
        .iter()
        .any(|lane| lane.starts_with("mcp."))
    {
        return Err(
            "MCP tools belong in the MCP tab; plugin lanes must not start with mcp.".to_string(),
        );
    }
    if !manifest.core_override {
        for lane in &manifest.expected_lanes {
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
                    "lane '{lane}' is reserved for core runtime capabilities"
                ));
            }
        }
    }
    Ok(manifest)
}

fn normalize_plugin_list(values: Vec<String>) -> Vec<String> {
    dedup_strings(
        values
            .into_iter()
            .flat_map(|value| {
                value
                    .split([',', '\n', '\r'])
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect(),
    )
}

fn writable_plugin_root() -> Result<PathBuf, String> {
    Ok(repo_root()?.join("user-files").join("plugins"))
}

fn write_plugin_manifest(path: &Path, manifest: &PluginManifest) -> Result<(), String> {
    let serialized = serde_json::to_string_pretty(manifest).map_err(|error| error.to_string())?;
    fs::write(path, format!("{serialized}\n")).map_err(|error| error.to_string())
}

fn read_plugin_status(manifest_path: &Path) -> Result<PluginStatus, String> {
    let content = fs::read_to_string(manifest_path).map_err(|error| error.to_string())?;
    let manifest: PluginManifest = serde_json::from_str(&content)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    Ok(PluginStatus {
        name: manifest.name.clone(),
        version: manifest.version,
        description: manifest.description,
        state: if manifest.enabled {
            "Active"
        } else {
            "Disabled"
        }
        .to_string(),
        tool_count: 0,
        expected_lanes: manifest.expected_lanes,
        enabled: manifest.enabled,
        command: manifest.command,
        args: manifest.args,
        required_env: manifest.required_env,
        env: manifest.env,
        core_override: manifest.core_override,
        manifest_path: relative_config_path(manifest_path),
        failure: None,
    })
}

fn find_plugin_manifest_path(name: &str) -> Result<PathBuf, String> {
    let target = name.trim();
    if target.is_empty() {
        return Err("plugin name is required".to_string());
    }
    for dir in user_file_dirs(
        "ORDO_PLUGINS_PATH",
        &["user-files/plugins", "ordo-studio/user-files/plugins"],
    )? {
        if !dir.exists() {
            continue;
        }
        for entry in fs::read_dir(&dir).map_err(|error| error.to_string())? {
            let plugin_dir = entry.map_err(|error| error.to_string())?.path();
            let manifest_path = plugin_dir.join("plugin.json");
            if !manifest_path.exists() {
                continue;
            }
            let content = fs::read_to_string(&manifest_path).map_err(|error| error.to_string())?;
            let manifest: PluginManifest = serde_json::from_str(&content)
                .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
            if manifest.name == target {
                return Ok(manifest_path);
            }
        }
    }
    Err(format!("plugin '{target}' was not found"))
}

fn ensure_deletable_plugin_dir(plugin_dir: &Path) -> Result<(), String> {
    let canonical_dir = plugin_dir
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let allowed_roots = user_file_dirs(
        "ORDO_PLUGINS_PATH",
        &["user-files/plugins", "ordo-studio/user-files/plugins"],
    )?;
    let mut allowed = false;
    for root in allowed_roots {
        if !root.exists() {
            continue;
        }
        let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
        if canonical_dir.starts_with(&canonical_root) && canonical_dir != canonical_root {
            allowed = true;
            break;
        }
    }
    if allowed {
        Ok(())
    } else {
        Err("refusing to delete a path outside the plugin roots".to_string())
    }
}

fn ensure_deletable_skill_dir(skill_dir: &Path) -> Result<(), String> {
    let canonical_dir = skill_dir
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let allowed_roots = user_file_dirs(
        "ORDO_SKILLS_PATH",
        &["user-files/skills", "ordo-studio/user-files/skills"],
    )?;
    let mut allowed = false;
    for root in allowed_roots {
        if !root.exists() {
            continue;
        }
        let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
        if canonical_dir.starts_with(&canonical_root) && canonical_dir != canonical_root {
            allowed = true;
            break;
        }
    }
    if allowed {
        Ok(())
    } else {
        Err("refusing to delete a path outside the skill roots".to_string())
    }
}

fn dedup_strings(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn local_mcp_manifests() -> Result<Vec<(String, Value)>, String> {
    let mut manifests = Vec::new();
    let mut seen = BTreeSet::new();
    for dir in repo_dirs(&["mcp-servers"])? {
        if !dir.exists() {
            continue;
        }
        for entry in fs::read_dir(&dir).map_err(|error| error.to_string())? {
            let server_dir = entry.map_err(|error| error.to_string())?.path();
            if !server_dir.is_dir() {
                continue;
            }
            let manifest_path = server_dir.join("manifest.json");
            if !manifest_path.exists() {
                continue;
            }
            let content = fs::read_to_string(&manifest_path).map_err(|error| error.to_string())?;
            let manifest: Value = serde_json::from_str(&content)
                .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
            let server_id = manifest
                .pointer("/identity/name")
                .and_then(Value::as_str)
                .or_else(|| server_dir.file_name().and_then(|value| value.to_str()))
                .unwrap_or("mcp-server")
                .to_string();
            if seen.insert(server_id.clone()) {
                manifests.push((server_id, manifest));
            }
        }
    }
    Ok(manifests)
}

fn user_file_dirs(env_key: &str, relative_dirs: &[&str]) -> Result<Vec<PathBuf>, String> {
    let mut dirs = env_path_dirs(env_key);
    dirs.extend(repo_dirs(relative_dirs)?);
    Ok(dedup_paths(dirs))
}

fn repo_dirs(relative_dirs: &[&str]) -> Result<Vec<PathBuf>, String> {
    let root = repo_root()?;
    Ok(relative_dirs.iter().map(|dir| root.join(dir)).collect())
}

fn env_path_dirs(env_key: &str) -> Vec<PathBuf> {
    env::var_os(env_key)
        .map(|value| env::split_paths(&value).collect())
        .unwrap_or_default()
}

fn dedup_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(path.display().to_string()))
        .collect()
}

fn build_niche_module(name: &str) -> NicheModule {
    let id = slugify(name);
    let accent = accent_from_seed(hash_seed(&id));
    let config_path = niche_config_dir()
        .map(|dir| dir.join(format!("{id}.json")))
        .unwrap_or_else(|_| {
            PathBuf::from("user-files")
                .join("niches")
                .join(format!("{id}.json"))
        });

    NicheModule {
        id: id.clone(),
        label: name.trim().to_string(),
        r#type: "CUSTOM_NICHE".to_string(),
        collection_id: format!("niche-{id}"),
        focus: format!("{} workflow lane", name.trim()),
        status: "CARVING".to_string(),
        accent,
        config_path: relative_config_path(&config_path),
    }
}

fn load_niche_modules() -> Result<Vec<NicheModule>, String> {
    let config_dir = niche_config_dir()?;
    if !config_dir.exists() {
        return Ok(Vec::new());
    }

    let mut modules = Vec::new();
    for entry in fs::read_dir(config_dir).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        modules.push(load_niche_module_file(&path)?);
    }
    modules.sort_by(|left, right| left.label.cmp(&right.label));
    Ok(modules)
}

fn load_niche_module_file(path: &Path) -> Result<NicheModule, String> {
    let content = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let mut module: NicheModule =
        serde_json::from_str(&content).map_err(|error| error.to_string())?;
    module.config_path = relative_config_path(path);
    Ok(module)
}

fn niche_config_dir() -> Result<PathBuf, String> {
    let repo_root = repo_root()?;
    Ok(repo_root.join("user-files").join("niches"))
}

fn repo_root() -> Result<PathBuf, String> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .ok_or_else(|| "Failed to resolve Ordo repo root.".to_string())
}

fn relative_config_path(path: &Path) -> String {
    repo_root()
        .ok()
        .and_then(|root| path.strip_prefix(root).ok().map(Path::to_path_buf))
        .unwrap_or_else(|| path.to_path_buf())
        .display()
        .to_string()
}

pub fn emit_log(
    app: &AppHandle,
    source: &str,
    message: impl Into<String>,
    level: LogLevel,
) -> Result<(), String> {
    let entry = LogEntry {
        id: format!(
            "{}-{}",
            source.to_ascii_lowercase().replace(' ', "-"),
            Utc::now().timestamp_millis()
        ),
        source: source.to_string(),
        message: message.into(),
        level,
        timestamp: Utc::now().to_rfc3339(),
    };
    app.emit("bus-event", entry)
        .map_err(|error| error.to_string())
}

fn lock_state<T>(mutex: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>, String> {
    mutex
        .lock()
        .map_err(|_| "Failed to lock shared studio state.".to_string())
}

fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;

    for ch in value.trim().to_ascii_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            previous_dash = false;
        } else if !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
    }

    slug.trim_matches('-').to_string()
}

fn hash_seed(value: &str) -> u16 {
    let mut seed = 0u16;
    for byte in value.bytes() {
        seed = (seed.wrapping_mul(31)).wrapping_add(byte as u16) % 997;
    }
    seed
}

fn accent_from_seed(seed: u16) -> String {
    let accents = [
        "#2dd4bf", "#38bdf8", "#22c55e", "#60a5fa", "#06b6d4", "#f59e0b",
    ];
    accents[(seed as usize) % accents.len()].to_string()
}
