use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewSubscription {
    pub target_url: String,
    /// If empty, the service generates a fresh random secret and
    /// returns it once in the created subscription.
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub topics: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubscriptionUpdate {
    #[serde(default)]
    pub target_url: Option<String>,
    #[serde(default)]
    pub topics: Option<Vec<String>>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub active: Option<bool>,
}

#[derive(Debug, thiserror::Error)]
pub enum WebhookError {
    #[error("subscription '{0}' not found")]
    NotFound(uuid::Uuid),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("local storage error: {0}")]
    Storage(String),
}

pub type WebhookResult<T> = Result<T, WebhookError>;
