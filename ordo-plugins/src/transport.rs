//! MCP transport abstraction so the client can talk to either a real
//! subprocess (newline-delimited JSON-RPC over stdin/stdout) or an
//! in-memory fake during tests.

use async_trait::async_trait;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

/// Half-duplex transport: send raw JSON lines, receive raw JSON lines.
#[async_trait]
pub trait McpTransport: Send + Sync {
    async fn send(&self, line: &str) -> Result<(), TransportError>;
    async fn recv(&self) -> Result<String, TransportError>;
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("transport closed")]
    Closed,
    #[error("transport io error: {0}")]
    Io(String),
}

impl From<std::io::Error> for TransportError {
    fn from(err: std::io::Error) -> Self {
        TransportError::Io(err.to_string())
    }
}

/// Subprocess-backed transport. Holds the child process handle so the
/// plugin stays alive as long as the transport is alive; dropping this
/// kills the subprocess.
pub struct StdioTransport {
    #[allow(dead_code)]
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<ChildStdin>>,
    stdout: Arc<Mutex<BufReader<ChildStdout>>>,
}

impl StdioTransport {
    pub fn new(mut child: Child) -> Result<Self, TransportError> {
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| TransportError::Io("plugin stdin not captured".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| TransportError::Io("plugin stdout not captured".into()))?;
        Ok(Self {
            child: Arc::new(Mutex::new(child)),
            stdin: Arc::new(Mutex::new(stdin)),
            stdout: Arc::new(Mutex::new(BufReader::new(stdout))),
        })
    }

    /// Kill the underlying subprocess. Best-effort; ignores errors.
    pub async fn shutdown(&self) {
        let mut child = self.child.lock().await;
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send(&self, line: &str) -> Result<(), TransportError> {
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await?;
        if !line.ends_with('\n') {
            stdin.write_all(b"\n").await?;
        }
        stdin.flush().await?;
        Ok(())
    }

    async fn recv(&self) -> Result<String, TransportError> {
        let mut stdout = self.stdout.lock().await;
        let mut buffer = String::new();
        let bytes = stdout.read_line(&mut buffer).await?;
        if bytes == 0 {
            return Err(TransportError::Closed);
        }
        Ok(buffer.trim_end_matches(['\r', '\n']).to_string())
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! In-memory transport for unit tests. The "client side" sends
    //! messages and receives responses; the "server side" is a
    //! handler closure the test supplies.

    use super::*;
    use tokio::sync::mpsc;

    pub struct ChannelTransport {
        to_server: Arc<Mutex<mpsc::UnboundedSender<String>>>,
        from_server: Arc<Mutex<mpsc::UnboundedReceiver<String>>>,
    }

    impl ChannelTransport {
        pub fn pair() -> (ChannelTransport, TestServerHandle) {
            let (client_to_server_tx, client_to_server_rx) = mpsc::unbounded_channel::<String>();
            let (server_to_client_tx, server_to_client_rx) = mpsc::unbounded_channel::<String>();
            let client = ChannelTransport {
                to_server: Arc::new(Mutex::new(client_to_server_tx)),
                from_server: Arc::new(Mutex::new(server_to_client_rx)),
            };
            let handle = TestServerHandle {
                from_client: Arc::new(Mutex::new(client_to_server_rx)),
                to_client: Arc::new(Mutex::new(server_to_client_tx)),
            };
            (client, handle)
        }
    }

    #[async_trait]
    impl McpTransport for ChannelTransport {
        async fn send(&self, line: &str) -> Result<(), TransportError> {
            self.to_server
                .lock()
                .await
                .send(line.to_string())
                .map_err(|_| TransportError::Closed)
        }

        async fn recv(&self) -> Result<String, TransportError> {
            self.from_server
                .lock()
                .await
                .recv()
                .await
                .ok_or(TransportError::Closed)
        }
    }

    pub struct TestServerHandle {
        from_client: Arc<Mutex<mpsc::UnboundedReceiver<String>>>,
        to_client: Arc<Mutex<mpsc::UnboundedSender<String>>>,
    }

    impl TestServerHandle {
        pub async fn recv_line(&self) -> Option<String> {
            self.from_client.lock().await.recv().await
        }

        pub async fn send_line(&self, line: impl Into<String>) {
            let _ = self.to_client.lock().await.send(line.into());
        }
    }
}
