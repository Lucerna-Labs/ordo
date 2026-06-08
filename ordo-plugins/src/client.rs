//! Async MCP client. Initialises the plugin, lists its tools, and
//! forwards tool calls. Responses are correlated by request id via a
//! background dispatcher task.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tracing::{debug, warn};

use crate::protocol::{
    McpIncoming, McpRequest, McpResponse, McpToolDescriptor, McpToolResult, PROTOCOL_VERSION,
};
use crate::transport::{McpTransport, TransportError};

#[derive(Debug, thiserror::Error)]
pub enum McpClientError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("plugin responded with JSON-RPC error {code}: {message}")]
    Protocol { code: i64, message: String },
    #[error("response deserialization failed: {0}")]
    Deserialize(String),
    #[error("timed out waiting for plugin reply")]
    Timeout,
    #[error("plugin stream closed")]
    Closed,
}

impl From<TransportError> for McpClientError {
    fn from(err: TransportError) -> Self {
        match err {
            TransportError::Closed => McpClientError::Closed,
            TransportError::Io(msg) => McpClientError::Transport(msg),
        }
    }
}

type ResponseSlot = oneshot::Sender<Result<McpResponse, McpClientError>>;

pub struct McpClient {
    transport: Arc<dyn McpTransport>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, ResponseSlot>>>,
    dispatcher: Mutex<Option<JoinHandle<()>>>,
    call_timeout: Duration,
}

impl McpClient {
    pub fn new(transport: Arc<dyn McpTransport>) -> Self {
        Self {
            transport,
            next_id: AtomicU64::new(1),
            pending: Arc::new(Mutex::new(HashMap::new())),
            dispatcher: Mutex::new(None),
            call_timeout: Duration::from_secs(30),
        }
    }

    pub fn with_call_timeout(mut self, timeout: Duration) -> Self {
        self.call_timeout = timeout;
        self
    }

    /// Start the background dispatcher that reads responses off the
    /// transport and routes them to the matching pending request.
    pub async fn start_dispatcher(self: &Arc<Self>) {
        let mut slot = self.dispatcher.lock().await;
        if slot.is_some() {
            return;
        }
        let client = Arc::clone(self);
        let handle = tokio::spawn(async move {
            client.dispatch_loop().await;
        });
        *slot = Some(handle);
    }

    async fn dispatch_loop(self: Arc<Self>) {
        loop {
            match self.transport.recv().await {
                Ok(line) if line.trim().is_empty() => continue,
                Ok(line) => {
                    debug!(target: "ordo_plugins::mcp", "<- {line}");
                    match serde_json::from_str::<McpIncoming>(&line) {
                        Ok(McpIncoming::Response(response)) => {
                            let id = response.id;
                            if let Some(slot) = self.pending.lock().await.remove(&id) {
                                let _ = slot.send(Ok(response));
                            } else {
                                warn!(
                                    target: "ordo_plugins::mcp",
                                    id, "response arrived with no pending request"
                                );
                            }
                        }
                        Ok(McpIncoming::Notification(notification)) => {
                            debug!(
                                target: "ordo_plugins::mcp",
                                method = %notification.method,
                                "server notification (ignored for now)"
                            );
                        }
                        Err(err) => {
                            warn!(
                                target: "ordo_plugins::mcp",
                                error = %err,
                                raw = %line,
                                "failed to parse plugin message"
                            );
                        }
                    }
                }
                Err(err) => {
                    warn!(target: "ordo_plugins::mcp", error = %err, "transport closed");
                    // Fail every pending request so no caller waits forever.
                    let mut pending = self.pending.lock().await;
                    for (_, slot) in pending.drain() {
                        let _ = slot.send(Err(McpClientError::Closed));
                    }
                    return;
                }
            }
        }
    }

    async fn send_request(
        &self,
        method: &str,
        params: Value,
    ) -> Result<McpResponse, McpClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let envelope = McpRequest::new(id, method, params);
        let line = serde_json::to_string(&envelope)
            .map_err(|err| McpClientError::Deserialize(err.to_string()))?;
        debug!(target: "ordo_plugins::mcp", "-> {line}");
        if let Err(err) = self.transport.send(&line).await {
            self.pending.lock().await.remove(&id);
            return Err(err.into());
        }

        match timeout(self.call_timeout, rx).await {
            Ok(Ok(Ok(response))) => Ok(response),
            Ok(Ok(Err(err))) => Err(err),
            Ok(Err(_)) => Err(McpClientError::Closed),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(McpClientError::Timeout)
            }
        }
    }

    async fn send_notification(&self, method: &str, params: Value) -> Result<(), McpClientError> {
        let envelope = crate::protocol::McpNotification::new(method, params);
        let line = serde_json::to_string(&envelope)
            .map_err(|err| McpClientError::Deserialize(err.to_string()))?;
        debug!(target: "ordo_plugins::mcp", "-> (notification) {line}");
        self.transport.send(&line).await?;
        Ok(())
    }

    /// Run the `initialize` + `notifications/initialized` handshake.
    pub async fn initialize(
        &self,
        client_name: &str,
        client_version: &str,
    ) -> Result<Value, McpClientError> {
        let response = self
            .send_request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {
                        "name": client_name,
                        "version": client_version,
                    }
                }),
            )
            .await?;
        if let Some(err) = response.error {
            return Err(McpClientError::Protocol {
                code: err.code,
                message: err.message,
            });
        }
        self.send_notification("notifications/initialized", Value::Null)
            .await?;
        Ok(response.result.unwrap_or(Value::Null))
    }

    pub async fn list_tools(&self) -> Result<Vec<McpToolDescriptor>, McpClientError> {
        let response = self.send_request("tools/list", json!({})).await?;
        if let Some(err) = response.error {
            return Err(McpClientError::Protocol {
                code: err.code,
                message: err.message,
            });
        }
        let result = response.result.unwrap_or(Value::Null);
        let tools = result
            .get("tools")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        serde_json::from_value(tools).map_err(|err| McpClientError::Deserialize(err.to_string()))
    }

    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<McpToolResult, McpClientError> {
        let params = json!({
            "name": name,
            "arguments": arguments,
        });
        let response = self.send_request("tools/call", params).await?;
        if let Some(err) = response.error {
            return Err(McpClientError::Protocol {
                code: err.code,
                message: err.message,
            });
        }
        let result = response.result.unwrap_or(Value::Null);
        serde_json::from_value(result).map_err(|err| McpClientError::Deserialize(err.to_string()))
    }
}
