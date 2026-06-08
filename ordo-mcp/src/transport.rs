//! Transport layer â€” reads JSON-RPC messages in, writes them out.
//!
//! Two transports:
//!   - **stdio** (Phase 2.2): line-delimited JSON over stdin/stdout.
//!     This is what Claude Desktop, Cursor, and Cline use. The loop
//!     reads a line, parses a `Request`, hands it to the `Server`,
//!     writes the response back.
//!   - **HTTP+SSE** (Phase 2.3): `POST /` for requests,
//!     `GET /events` for server-initiated notifications. Used by
//!     remote installs and web clients.

use std::sync::Arc;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::rpc::{codes, Request, Response};
use crate::server::Server;

/// Run the stdio transport loop until stdin closes. Spawns a task
/// per request so slow tool calls (e.g. LLM round-trips) don't block
/// subsequent requests. Responses are serialized back to stdout in
/// completion order â€” MCP clients match on `id`, not ordering.
pub async fn run_stdio(server: Server) {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();
    let writer = Arc::new(tokio::sync::Mutex::new(stdout));
    let server = Arc::new(server);

    loop {
        let line = match reader.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => {
                tracing::info!(target: "ordo_mcp", "stdin closed â€” exiting");
                break;
            }
            Err(err) => {
                tracing::error!(
                    target: "ordo_mcp",
                    error = %err,
                    "stdin read error â€” exiting"
                );
                break;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let writer = writer.clone();
        let server = server.clone();
        let input = trimmed.to_string();
        tokio::spawn(async move {
            let response = handle_line(&input, server.as_ref()).await;
            if let Some(response) = response {
                let mut out = writer.lock().await;
                if let Err(err) = write_response(&mut out, &response).await {
                    tracing::error!(
                        target: "ordo_mcp",
                        error = %err,
                        "failed to write response"
                    );
                }
            }
        });
    }
}

/// Parse a single line. On parse error, produce a synthetic error
/// response keyed by id=null so at least the client sees a JSON-RPC
/// error (per spec Â§5.1).
async fn handle_line(line: &str, server: &Server) -> Option<Response> {
    match serde_json::from_str::<Request>(line) {
        Ok(request) => server.handle(request).await,
        Err(err) => Some(Response::error(
            Value::Null,
            codes::PARSE_ERROR,
            format!("parse error: {err}"),
        )),
    }
}

async fn write_response(out: &mut tokio::io::Stdout, response: &Response) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(response).unwrap_or_else(|_| {
        br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"serialize failed"}}"#
            .to_vec()
    });
    bytes.push(b'\n');
    out.write_all(&bytes).await?;
    out.flush().await
}

// -- HTTP transport (Phase 2.3) -------------------------------------
//
// Plain `POST /mcp` that accepts a single JSON-RPC request and
// returns the response body. Enough for MCP clients that use the
// streamable-HTTP transport without the SSE up-stream channel (which
// we don't need today because our server emits no notifications).
//
// Mount with `ordo-mcp --http 127.0.0.1:4242`.

/// Build an axum router that handles `POST /mcp` and `GET /health`.
/// Kept separate from `run_stdio` so the two transports don't share
/// state beyond the `Server` instance.
pub fn http_router(server: Server) -> axum::Router {
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use std::sync::Arc;

    #[derive(Clone)]
    struct HttpState {
        server: Arc<Server>,
    }

    async fn post_mcp(
        State(state): State<HttpState>,
        Json(request): Json<Request>,
    ) -> (StatusCode, Json<serde_json::Value>) {
        let id = request.id.clone();
        match state.server.handle(request).await {
            Some(response) => (
                StatusCode::OK,
                Json(serde_json::to_value(&response).unwrap_or(Value::Null)),
            ),
            // Notifications (no id) â†’ 204-ish, but we return 202 with
            // an empty ack body so clients that insist on JSON don't
            // choke.
            None => (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {"acknowledged": true}
                })),
            ),
        }
    }

    async fn health() -> Json<serde_json::Value> {
        Json(serde_json::json!({"status": "ok"}))
    }

    let state = HttpState {
        server: Arc::new(server),
    };
    Router::new()
        .route("/mcp", post(post_mcp))
        .route("/health", get(health))
        .with_state(state)
}

pub async fn run_http(server: Server, bind: &str) -> std::io::Result<()> {
    let router = http_router(server);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(
        target: "ordo_mcp",
        bind = %bind,
        "http transport listening"
    );
    axum::serve(listener, router)
        .await
        .map_err(|err| std::io::Error::other(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, runtime::RuntimeClient};

    fn server() -> Server {
        let client = RuntimeClient::new(Config::default()).expect("client");
        Server::new(client)
    }

    #[tokio::test]
    async fn handle_line_returns_parse_error_on_bad_json() {
        let resp = handle_line("this is not json", &server())
            .await
            .expect("should respond");
        let err = resp.error.expect("error");
        assert_eq!(err.code, codes::PARSE_ERROR);
    }

    #[tokio::test]
    async fn http_router_handles_initialize_post() {
        use axum::body::Body;
        use axum::http::{Request as HttpRequest, StatusCode};
        use tower::ServiceExt;

        let router = http_router(server());
        let body = serde_json::to_vec(&Request {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::json!(1)),
            method: "initialize".into(),
            params: None,
        })
        .unwrap();

        let response = router
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: Value = serde_json::from_slice(&body_bytes).expect("json");
        assert_eq!(value["result"]["serverInfo"]["name"], "ordo-mcp");
    }

    #[tokio::test]
    async fn http_router_acks_notifications_with_202() {
        use axum::body::Body;
        use axum::http::{Request as HttpRequest, StatusCode};
        use tower::ServiceExt;

        let router = http_router(server());
        let body = serde_json::to_vec(&Request {
            jsonrpc: "2.0".into(),
            id: None,
            method: "notifications/initialized".into(),
            params: None,
        })
        .unwrap();

        let response = router
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn handle_line_roundtrips_initialize() {
        let line = serde_json::to_string(&Request {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::json!(1)),
            method: "initialize".into(),
            params: None,
        })
        .unwrap();
        let resp = handle_line(&line, &server()).await.expect("response");
        assert!(resp.result.is_some());
    }
}
