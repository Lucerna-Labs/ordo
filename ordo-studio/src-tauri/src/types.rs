use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SystemState {
    #[default]
    Healthy,
    Processing,
    Rescue,
    Critical,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub id: String,
    pub source: String,
    pub message: String,
    pub level: LogLevel,
    pub timestamp: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NicheModule {
    pub id: String,
    pub label: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub collection_id: String,
    pub focus: String,
    pub status: String,
    pub accent: String,
    pub config_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RagCollection {
    pub id: String,
    pub label: String,
    pub group: String,
    pub chunk_count: usize,
    pub document_count: usize,
    pub accent: String,
    pub summary: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwarmNode {
    pub id: String,
    pub label: String,
    pub status: String,
    pub transport: String,
    pub latency_ms: u16,
    pub collections: Vec<String>,
    pub zone: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct P2pStatus {
    pub mode: String,
    pub health: String,
    pub relay: String,
    pub summary: String,
    pub connected_peers: usize,
    pub nodes: Vec<SwarmNode>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibrarySnapshot {
    pub collections: Vec<RagCollection>,
    pub p2p_status: P2pStatus,
    pub last_sync: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellBootstrap {
    pub system_state: SystemState,
    pub niche_modules: Vec<NicheModule>,
    pub library: LibrarySnapshot,
    pub active_niches: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MechanicReply {
    pub state: SystemState,
    pub response: String,
    pub actions: Vec<String>,
}
