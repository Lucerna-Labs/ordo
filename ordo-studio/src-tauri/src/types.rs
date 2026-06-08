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

