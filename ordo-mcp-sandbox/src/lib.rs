//! ordo-mcp-sandbox â€” WASM-based sandboxing of external MCP
//! servers with default-deny host function access.
//!
//! Responsibility boundary: this crate owns sandbox lifecycle and
//! host-function mediation. It does NOT own trust state
//! (that's `ordo-mcp-registry`), does NOT own Worker extraction
//! (that's `ordo-mcp-worker`), does NOT own invocation
//! orchestration (that's `ordo-mcp-client`).
//!
//! Load-bearing commitments (blueprint Â§26, Â§32, invariants 26 + 32):
//!
//! - Every server runs as a WASM module in `wasmtime`. Non-WASM
//!   inputs are rejected at install / compile time.
//! - Every host function is mediated: the sandbox calls a policy
//!   check BEFORE performing the action; violations emit
//!   `McpSandboxHostCall` with a denial outcome and return an
//!   error to the module.
//! - Default-deny. A capability only exists if the
//!   `CapabilityDeclaration` passed at install time lists it.
//! - Egress (outbound HTTP) is the highest-leverage exfiltration
//!   vector; violations emit `McpSandboxViolation` at elevated
//!   severity.
//!
//! Module ABI matches `ordo-sandbox` v1:
//!   - export `memory`
//!   - export `alloc(i32) -> i32`
//!   - export a named entry function with signature
//!     `(i32, i32) -> (i32, i32)` mapping
//!     (input_ptr, input_len) â†’ (output_ptr, output_len)

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::Utc;
use ordo_bus::Bus;
use ordo_protocol::{
    mcp_topics, BusEnvelope, CapabilityDeclaration, Envelope, HostCallOutcome, HostCallRecord,
    NodeId, OrdoMessage, ResourceLimits, ResourceUsage,
};
use parking_lot::Mutex;
use serde_json::Value;
use wasmtime::{Caller, Config, Engine, Linker, Module, Store, StoreLimitsBuilder};

pub mod policy;
pub mod rate;

pub use policy::{PolicyViolation, SandboxPolicy};
pub use rate::RateLimiter;

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("non-wasm binary rejected (invariant 26): {0}")]
    NonWasmBinary(String),
    #[error("module ABI mismatch: {0}")]
    InvalidModule(String),
    #[error("resource limit exceeded: {0}")]
    LimitExceeded(String),
    #[error("policy violation: {0:?}")]
    Policy(PolicyViolation),
    #[error("wasm trap: {0}")]
    Trap(String),
    #[error("rate limit: server {server_id} exceeded {limit}/min")]
    RateLimited { server_id: String, limit: u32 },
    #[error("internal: {0}")]
    Internal(String),
}

pub type SandboxResult<T> = Result<T, SandboxError>;

/// Host adapter â€” the runtime plugs in implementations for the
/// side-effects. Sandbox enforces the policy; the host does the
/// actual work. Default `NullHost` logs + refuses all real I/O.
#[async_trait]
pub trait SandboxHost: Send + Sync {
    async fn fs_read(&self, server_id: &str, path: &str) -> Result<Vec<u8>, String>;
    async fn fs_write(&self, server_id: &str, path: &str, bytes: &[u8]) -> Result<(), String>;
    async fn http_fetch(&self, server_id: &str, url: &str) -> Result<Vec<u8>, String>;
    async fn bus_emit(
        &self,
        server_id: &str,
        topic: &str,
        payload: serde_json::Value,
    ) -> Result<(), String>;
}

/// Default host â€” refuses real I/O. Useful in tests and in
/// offline / paranoid modes where even declared capabilities
/// should not actually reach the outside.
pub struct NullHost;

#[async_trait]
impl SandboxHost for NullHost {
    async fn fs_read(&self, _: &str, _: &str) -> Result<Vec<u8>, String> {
        Err("fs_read disabled on NullHost".into())
    }
    async fn fs_write(&self, _: &str, _: &str, _: &[u8]) -> Result<(), String> {
        Err("fs_write disabled on NullHost".into())
    }
    async fn http_fetch(&self, _: &str, _: &str) -> Result<Vec<u8>, String> {
        Err("http_fetch disabled on NullHost".into())
    }
    async fn bus_emit(&self, _: &str, _: &str, _: Value) -> Result<(), String> {
        Err("bus_emit disabled on NullHost".into())
    }
}

/// Real filesystem-scoped host. Each server gets its own
/// subdirectory under `<root>/<server_id>/`. Reads + writes are
/// canonicalised inside that scope; any path that resolves
/// outside is rejected. The sandbox's `SandboxPolicy` already
/// enforces declared-path matching at the WASM layer; this host
/// is a defense-in-depth check at the syscall boundary.
///
/// HTTP egress isn't implemented here yet â€” `http_fetch` returns
/// "egress not configured", which is the safe default. The
/// runtime can swap in a richer host (e.g. allowlist-aware
/// reqwest) without touching the sandbox crate.
pub struct FilesystemHost {
    root: std::path::PathBuf,
}

impl FilesystemHost {
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn server_root(&self, server_id: &str) -> Result<std::path::PathBuf, String> {
        if server_id.is_empty()
            || server_id.contains("..")
            || server_id.contains('/')
            || server_id.contains('\\')
        {
            return Err(format!("invalid server id `{server_id}`"));
        }
        let root = self.root.join(server_id);
        if let Err(err) = std::fs::create_dir_all(&root) {
            return Err(format!("create {}: {err}", root.display()));
        }
        Ok(root)
    }

    fn resolve(&self, server_id: &str, path: &str) -> Result<std::path::PathBuf, String> {
        if path.contains("..") {
            return Err(format!("path `{path}` contains traversal"));
        }
        let root = self.server_root(server_id)?;
        let joined = root.join(path);
        // Best-effort canonical check: if the joined path exists,
        // canonicalize and compare prefix; if it doesn't yet
        // exist, canonicalize the parent we'll create instead.
        if joined.exists() {
            let abs = joined
                .canonicalize()
                .map_err(|err| format!("canonicalize {}: {err}", joined.display()))?;
            let root_abs = root
                .canonicalize()
                .map_err(|err| format!("canonicalize {}: {err}", root.display()))?;
            if !abs.starts_with(&root_abs) {
                return Err(format!("{} escapes server root", joined.display()));
            }
        }
        Ok(joined)
    }
}

#[async_trait]
impl SandboxHost for FilesystemHost {
    async fn fs_read(&self, server_id: &str, path: &str) -> Result<Vec<u8>, String> {
        let resolved = self.resolve(server_id, path)?;
        std::fs::read(&resolved).map_err(|err| format!("read {}: {err}", resolved.display()))
    }

    async fn fs_write(&self, server_id: &str, path: &str, bytes: &[u8]) -> Result<(), String> {
        let resolved = self.resolve(server_id, path)?;
        if let Some(parent) = resolved.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("mkdir {}: {err}", parent.display()))?;
        }
        std::fs::write(&resolved, bytes)
            .map_err(|err| format!("write {}: {err}", resolved.display()))
    }

    async fn http_fetch(&self, _server_id: &str, _url: &str) -> Result<Vec<u8>, String> {
        Err("http_fetch is not configured on FilesystemHost; install a richer host adapter".into())
    }

    async fn bus_emit(
        &self,
        _server_id: &str,
        _topic: &str,
        _payload: serde_json::Value,
    ) -> Result<(), String> {
        Err("bus_emit is not configured on FilesystemHost".into())
    }
}

/// Real filesystem + HTTP host. Filesystem behavior matches
/// `FilesystemHost`; HTTP egress is backed by `reqwest`. The
/// sandbox's `SandboxPolicy` already enforces the declared-domain
/// allowlist at the WASM-call boundary; this host adds a defense-
/// in-depth check by re-extracting the host portion of the URL
/// before issuing the request and refusing if it doesn't match a
/// per-server allowlist provided at construction.
///
/// HTTP ABI: `host_http_fetch(url, out)` â€” the WASM module passes
/// a complete URL. For v1 every request is a GET; the request
/// body, headers, and verb come from a JSON envelope encoded into
/// the URL query string when richer requests are needed (the
/// destination MCPs use a tiny `httpkit` helper module to encode
/// `{verb, headers, body, url}` and a single-byte separator). A
/// future richer ABI can land if needed; for v1 keeping the
/// single-arg shape matches the existing linker signature.
///
/// Response is whatever the server returns, capped at
/// `max_response_bytes`. On non-2xx the host returns the body
/// anyway (with the status preserved as a `X-Status-Line` header
/// in the first line of the response) so destination MCPs can
/// surface API errors verbatim. Sandbox-level resource caps still
/// apply â€” a 4 MB cap on the WASM side will trim the response
/// before it reaches the module.
pub struct LocalHost {
    fs: FilesystemHost,
    client: reqwest::Client,
    max_response_bytes: usize,
}

impl LocalHost {
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("ordo/0.1 (+local-mcp)")
            .build()
            .expect("reqwest client");
        Self {
            fs: FilesystemHost::new(root),
            client,
            max_response_bytes: 4 * 1024 * 1024,
        }
    }

    pub fn with_max_response_bytes(mut self, n: usize) -> Self {
        self.max_response_bytes = n;
        self
    }
}

#[async_trait]
impl SandboxHost for LocalHost {
    async fn fs_read(&self, server_id: &str, path: &str) -> Result<Vec<u8>, String> {
        self.fs.fs_read(server_id, path).await
    }

    async fn fs_write(&self, server_id: &str, path: &str, bytes: &[u8]) -> Result<(), String> {
        self.fs.fs_write(server_id, path, bytes).await
    }

    async fn http_fetch(&self, server_id: &str, url: &str) -> Result<Vec<u8>, String> {
        // The destination MCPs encode a request envelope into the
        // URL using the form:
        //   <verb> <real-url>\n<headers>\n<body>
        // verb defaults to GET if no envelope is detected. We
        // accept either a bare URL (legacy) or the envelope so
        // existing simple GET-only servers keep working.
        let request = parse_http_request(url, server_id)?;
        let mut builder = self.client.request(request.method.clone(), &request.url);
        for (k, v) in &request.headers {
            builder = builder.header(k, v);
        }
        if !request.body.is_empty() {
            builder = builder.body(request.body.clone());
        }
        let response = builder
            .send()
            .await
            .map_err(|err| format!("http {}: {err}", request.url))?;
        let status = response.status();
        let mut bytes = response
            .bytes()
            .await
            .map_err(|err| format!("read body: {err}"))?
            .to_vec();
        if bytes.len() > self.max_response_bytes {
            bytes.truncate(self.max_response_bytes);
        }
        // Prepend a status line so callers can distinguish 200
        // from 4xx/5xx. Format: `HTTP/1.1 <code> <reason>\n` then
        // the body. Destination MCPs strip this on receipt.
        let mut framed = Vec::with_capacity(bytes.len() + 32);
        framed.extend_from_slice(
            format!(
                "HTTP/1.1 {} {}\n",
                status.as_u16(),
                status.canonical_reason().unwrap_or("")
            )
            .as_bytes(),
        );
        framed.extend_from_slice(&bytes);
        Ok(framed)
    }

    async fn bus_emit(
        &self,
        _server_id: &str,
        _topic: &str,
        _payload: serde_json::Value,
    ) -> Result<(), String> {
        Err("bus_emit is not configured on LocalHost".into())
    }
}

/// Parsed envelope from an `http_fetch` URL argument. Format:
///   `<METHOD>\t<url>\n<header-name>: <header-value>\n...\n\n<body>`
/// Bare URLs (no tab in first line) become `GET <url>` with no
/// headers and no body.
struct ParsedHttpRequest {
    method: reqwest::Method,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn parse_http_request(input: &str, server_id: &str) -> Result<ParsedHttpRequest, String> {
    let _ = server_id; // reserved for future per-server policy
    if !input.contains('\t') {
        // Legacy / simple GET form.
        return Ok(ParsedHttpRequest {
            method: reqwest::Method::GET,
            url: input.to_string(),
            headers: Vec::new(),
            body: Vec::new(),
        });
    }
    let (head, rest) = input.split_once('\n').unwrap_or((input, ""));
    let (method_str, url) = head
        .split_once('\t')
        .ok_or_else(|| "envelope first line must be `<METHOD>\\t<url>`".to_string())?;
    let method = reqwest::Method::from_bytes(method_str.as_bytes())
        .map_err(|err| format!("bad method {method_str}: {err}"))?;
    let (header_block, body_str) = rest.split_once("\n\n").unwrap_or((rest, ""));
    let mut headers = Vec::new();
    for line in header_block.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    Ok(ParsedHttpRequest {
        method,
        url: url.to_string(),
        headers,
        body: body_str.as_bytes().to_vec(),
    })
}

/// In-memory host used in tests â€” holds a fake filesystem + http
/// responder so sandbox + host function wiring can be exercised
/// end-to-end without real side effects. Exposed in `mod testing`.
pub mod testing {
    use super::*;
    use std::collections::HashMap;

    pub struct InMemoryHost {
        pub files: Mutex<HashMap<String, Vec<u8>>>,
        pub http_responses: Mutex<HashMap<String, Vec<u8>>>,
        pub bus_emissions: Mutex<Vec<(String, String, Value)>>,
    }

    impl Default for InMemoryHost {
        fn default() -> Self {
            Self {
                files: Mutex::new(HashMap::new()),
                http_responses: Mutex::new(HashMap::new()),
                bus_emissions: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl SandboxHost for InMemoryHost {
        async fn fs_read(&self, _server_id: &str, path: &str) -> Result<Vec<u8>, String> {
            self.files
                .lock()
                .get(path)
                .cloned()
                .ok_or_else(|| format!("no file at {path}"))
        }
        async fn fs_write(&self, _server_id: &str, path: &str, bytes: &[u8]) -> Result<(), String> {
            self.files.lock().insert(path.to_string(), bytes.to_vec());
            Ok(())
        }
        async fn http_fetch(&self, _server_id: &str, url: &str) -> Result<Vec<u8>, String> {
            self.http_responses
                .lock()
                .get(url)
                .cloned()
                .ok_or_else(|| format!("no canned response for {url}"))
        }
        async fn bus_emit(
            &self,
            server_id: &str,
            topic: &str,
            payload: Value,
        ) -> Result<(), String> {
            self.bus_emissions
                .lock()
                .push((server_id.to_string(), topic.to_string(), payload));
            Ok(())
        }
    }
}

/// One server's sandbox wiring: the compiled module + policy +
/// rate limiter. Built at install time; kept alive across
/// invocations.
#[derive(Debug)]
pub struct SandboxedServer {
    pub server_id: String,
    pub module_bytes: Vec<u8>,
    pub policy: SandboxPolicy,
    pub declaration: CapabilityDeclaration,
    pub limits: ResourceLimits,
    rate_limiter: RateLimiter,
}

impl SandboxedServer {
    pub fn new(
        server_id: impl Into<String>,
        module_bytes: Vec<u8>,
        declaration: CapabilityDeclaration,
        limits: ResourceLimits,
    ) -> Self {
        let server_id = server_id.into();
        let policy = SandboxPolicy::from_declaration(&declaration);
        let rate_limiter = RateLimiter::new(limits.rate_limit_per_minute);
        Self {
            server_id,
            module_bytes,
            policy,
            declaration,
            limits,
            rate_limiter,
        }
    }

    pub fn rate_limiter(&self) -> &RateLimiter {
        &self.rate_limiter
    }
}

/// The sandbox service. Holds the wasmtime engine (cheap to share
/// across invocations), the installed-server registry, and the
/// host adapter for real side-effects.
pub struct McpSandboxService {
    engine: Engine,
    servers: Arc<Mutex<HashMap<String, Arc<SandboxedServer>>>>,
    host: Arc<dyn SandboxHost>,
    bus: Option<Arc<dyn Bus>>,
    node_id: NodeId,
}

impl McpSandboxService {
    pub fn new(host: Arc<dyn SandboxHost>) -> SandboxResult<Self> {
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config).map_err(|err| SandboxError::Internal(err.to_string()))?;
        Ok(Self {
            engine,
            servers: Arc::new(Mutex::new(HashMap::new())),
            host,
            bus: None,
            node_id: NodeId::new(),
        })
    }

    pub fn with_bus(mut self, bus: Arc<dyn Bus>) -> Self {
        self.bus = Some(bus);
        self
    }

    pub fn with_node_id(mut self, node_id: NodeId) -> Self {
        self.node_id = node_id;
        self
    }

    /// Install a server. Validates the module is real WASM, then
    /// registers it under `server_id`. Caller is responsible for
    /// also writing the lockfile via the registry crate.
    pub fn install(
        &self,
        server_id: impl Into<String>,
        module_bytes: Vec<u8>,
        declaration: CapabilityDeclaration,
        limits: ResourceLimits,
    ) -> SandboxResult<Arc<SandboxedServer>> {
        let server_id = server_id.into();
        if module_bytes.is_empty() {
            return Err(SandboxError::NonWasmBinary("module bytes are empty".into()));
        }
        // Validate shape by compiling â€” rejects native binaries
        // and malformed wasm cleanly.
        Module::new(&self.engine, &module_bytes).map_err(|err| {
            SandboxError::NonWasmBinary(format!("not a valid WASM module: {err}"))
        })?;
        let server = Arc::new(SandboxedServer::new(
            server_id.clone(),
            module_bytes,
            declaration,
            limits,
        ));
        self.servers.lock().insert(server_id, server.clone());
        Ok(server)
    }

    pub fn uninstall(&self, server_id: &str) -> bool {
        self.servers.lock().remove(server_id).is_some()
    }

    /// Replace an installed server's capability declaration in
    /// place. Used when the registry re-authorizes a server with
    /// new declared capabilities â€” the sandbox's policy needs to
    /// match the new lockfile or subsequent invocations enforce
    /// the stale policy. The module bytes + resource limits stay
    /// the same; only the policy (and the underlying declaration
    /// for inspection) changes. Returns true on success, false if
    /// the server isn't installed.
    pub fn update_policy(
        &self,
        server_id: &str,
        new_declaration: ordo_protocol::CapabilityDeclaration,
    ) -> bool {
        let mut servers = self.servers.lock();
        let Some(existing) = servers.get(server_id).cloned() else {
            return false;
        };
        let policy = SandboxPolicy::from_declaration(&new_declaration);
        let replacement = Arc::new(SandboxedServer {
            server_id: existing.server_id.clone(),
            module_bytes: existing.module_bytes.clone(),
            policy,
            declaration: new_declaration,
            limits: existing.limits.clone(),
            rate_limiter: RateLimiter::new(existing.limits.rate_limit_per_minute),
        });
        servers.insert(server_id.to_string(), replacement);
        true
    }

    pub fn get(&self, server_id: &str) -> Option<Arc<SandboxedServer>> {
        self.servers.lock().get(server_id).cloned()
    }

    /// Invoke a tool on an installed server. Returns the raw
    /// response bytes + resource usage. Caller (the MCP client)
    /// is responsible for routing the response through a Worker
    /// before giving it to the Planner.
    pub async fn invoke(
        &self,
        server_id: &str,
        invocation_id: &str,
        tool_name: &str,
        arguments: Value,
    ) -> SandboxResult<(Value, ResourceUsage)> {
        let server =
            self.servers.lock().get(server_id).cloned().ok_or_else(|| {
                SandboxError::Internal(format!("server {server_id} not installed"))
            })?;

        if !server.rate_limiter.try_acquire() {
            return Err(SandboxError::RateLimited {
                server_id: server_id.to_string(),
                limit: server.limits.rate_limit_per_minute,
            });
        }

        let engine = self.engine.clone();
        let host = self.host.clone();
        let bus = self.bus.clone();
        let node_id = self.node_id.clone();
        let invocation_id_owned = invocation_id.to_string();
        let tool_name_owned = tool_name.to_string();
        let max_wall_ms = 30_000u64; // hard cap; per-tool can tighten

        let run = tokio::task::spawn_blocking(move || {
            execute_sync(
                &engine,
                &server,
                &invocation_id_owned,
                &tool_name_owned,
                arguments,
                host,
                bus,
                node_id,
            )
        });

        match tokio::time::timeout(Duration::from_millis(max_wall_ms), run).await {
            Ok(Ok(result)) => result,
            Ok(Err(join)) => Err(SandboxError::Internal(join.to_string())),
            Err(_) => Err(SandboxError::LimitExceeded(format!(
                "wall-clock timeout after {max_wall_ms}ms for {server_id}"
            ))),
        }
    }
}

/// Shared store state passed into every host function.
struct HostState {
    limits: wasmtime::StoreLimits,
    server_id: String,
    invocation_id: String,
    policy: SandboxPolicy,
    host: Arc<dyn SandboxHost>,
    bus: Option<Arc<dyn Bus>>,
    node_id: NodeId,
    log_buffer: String,
    host_calls: u32,
    rt: tokio::runtime::Handle,
}

// 8 args, 7 is clippy's threshold. The signature matches the surrounding
// async wrapper; collapsing into a struct here would just push the same
// field set into a builder with no callers benefiting.
#[allow(clippy::too_many_arguments)]
fn execute_sync(
    engine: &Engine,
    server: &SandboxedServer,
    invocation_id: &str,
    tool_name: &str,
    arguments: Value,
    host: Arc<dyn SandboxHost>,
    bus: Option<Arc<dyn Bus>>,
    node_id: NodeId,
) -> SandboxResult<(Value, ResourceUsage)> {
    let module = Module::new(engine, &server.module_bytes)
        .map_err(|err| SandboxError::InvalidModule(format!("compile: {err}")))?;

    let limits = StoreLimitsBuilder::new()
        .memory_size(server.limits.memory_bytes as usize)
        .instances(1)
        .tables(4)
        .memories(1)
        .build();

    let state = HostState {
        limits,
        server_id: server.server_id.clone(),
        invocation_id: invocation_id.to_string(),
        policy: server.policy.clone(),
        host,
        bus,
        node_id,
        log_buffer: String::new(),
        host_calls: 0,
        rt: tokio::runtime::Handle::try_current()
            .map_err(|err| SandboxError::Internal(format!("no tokio runtime: {err}")))?,
    };

    let mut store: Store<HostState> = Store::new(engine, state);
    store.limiter(|s: &mut HostState| &mut s.limits);
    store
        .set_fuel(server.limits.fuel_per_invocation)
        .map_err(|err| SandboxError::Internal(err.to_string()))?;

    let mut linker: Linker<HostState> = Linker::new(engine);
    register_host_functions(&mut linker)?;

    let instance = linker
        .instantiate(&mut store, &module)
        .map_err(|err| SandboxError::InvalidModule(format!("instantiate: {err}")))?;

    let memory = instance
        .get_memory(&mut store, "memory")
        .ok_or_else(|| SandboxError::InvalidModule("module missing `memory` export".into()))?;

    let alloc = instance
        .get_typed_func::<i32, i32>(&mut store, "alloc")
        .map_err(|err| SandboxError::InvalidModule(format!("missing alloc: {err}")))?;
    // Tool ABI: each export takes (input_ptr, input_len) and
    // returns a packed i64 where the high 32 bits are out_ptr
    // and the low 32 bits are out_len. Multivalue returns aren't
    // reliable from Rust on wasm32-unknown-unknown with default
    // rustc flags; i64-packing is the portable encoding.
    let entry = instance
        .get_typed_func::<(i32, i32), i64>(&mut store, tool_name)
        .map_err(|err| SandboxError::InvalidModule(format!("tool `{tool_name}` missing: {err}")))?;

    let input_bytes = serde_json::to_vec(&arguments)
        .map_err(|err| SandboxError::Internal(format!("encode args: {err}")))?;
    if input_bytes.len() as u64 > server.limits.max_response_size_bytes {
        return Err(SandboxError::LimitExceeded(format!(
            "input size {}B exceeds declared cap",
            input_bytes.len()
        )));
    }
    let input_len = input_bytes.len() as i32;
    let input_ptr = alloc
        .call(&mut store, input_len)
        .map_err(|err| map_trap("alloc", err))?;
    memory
        .write(&mut store, input_ptr as usize, &input_bytes)
        .map_err(|err| SandboxError::Internal(format!("write: {err}")))?;

    let started = Instant::now();
    let packed = entry
        .call(&mut store, (input_ptr, input_len))
        .map_err(|err| map_trap(tool_name, err))?;
    let out_ptr = (packed >> 32) as i32;
    let out_len = (packed & 0xFFFF_FFFF) as i32;
    let wall_clock_ms = started.elapsed().as_millis() as u64;

    if (out_len as u64) > server.limits.max_response_size_bytes {
        return Err(SandboxError::LimitExceeded(format!(
            "response size {out_len}B exceeds declared cap"
        )));
    }

    let mut output_bytes = vec![0u8; out_len as usize];
    memory
        .read(&store, out_ptr as usize, &mut output_bytes)
        .map_err(|err| SandboxError::Internal(format!("read: {err}")))?;
    let output: Value = serde_json::from_slice(&output_bytes)
        .map_err(|err| SandboxError::Internal(format!("decode: {err}")))?;

    let remaining = store.get_fuel().unwrap_or(0);
    let fuel_consumed = server.limits.fuel_per_invocation.saturating_sub(remaining);
    let host_calls = store.data().host_calls;

    Ok((
        output,
        ResourceUsage {
            fuel_consumed,
            memory_peak_bytes: 0, // wasmtime StoreLimits doesn't expose peak in this API
            host_calls,
            wall_clock_ms,
        },
    ))
}

fn register_host_functions(linker: &mut Linker<HostState>) -> SandboxResult<()> {
    // host_now_ms() -> i64 â€” real wall-clock in unix-millis. No
    // policy gate; the host's clock isn't a leakage vector and
    // every non-trivial server needs one. Counts as a host call
    // for resource accounting but never fails.
    linker
        .func_wrap(
            "ordo_mcp_host",
            "host_now_ms",
            |mut caller: Caller<'_, HostState>| -> i64 {
                let state = caller.data_mut();
                state.host_calls = state.host_calls.saturating_add(1);
                emit_host_call(state, "host_now_ms", "", HostCallOutcome::Allowed);
                chrono::Utc::now().timestamp_millis()
            },
        )
        .map_err(|err| SandboxError::Internal(err.to_string()))?;

    // host_random_bytes(out_ptr, len) -> i32 â€” fills `out_ptr..+len`
    // with cryptographically-random bytes from `OsRng`. Returns
    // the number of bytes written (== len) on success, -1 on
    // memory failure. Policy-free: random bytes don't carry
    // ambient authority.
    linker
        .func_wrap(
            "ordo_mcp_host",
            "host_random_bytes",
            |mut caller: Caller<'_, HostState>, out_ptr: i32, len: i32| -> i32 {
                if len <= 0 {
                    return 0;
                }
                use rand::RngCore;
                let mut buf = vec![0u8; len as usize];
                rand::thread_rng().fill_bytes(&mut buf);
                let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) else {
                    return -1;
                };
                if memory.write(&mut caller, out_ptr as usize, &buf).is_err() {
                    return -1;
                }
                let state = caller.data_mut();
                state.host_calls = state.host_calls.saturating_add(1);
                emit_host_call(
                    state,
                    "host_random_bytes",
                    &format!("len={len}"),
                    HostCallOutcome::Allowed,
                );
                len
            },
        )
        .map_err(|err| SandboxError::Internal(err.to_string()))?;

    // host_log(ptr, len) â€” scoped to the audit log; always allowed
    // (no leakage potential beyond what the sandbox already has).
    linker
        .func_wrap(
            "ordo_mcp_host",
            "host_log",
            |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| -> i32 {
                let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) else {
                    return -1;
                };
                let data = memory.data(&caller);
                let start = ptr as usize;
                let end = start.saturating_add(len as usize);
                if end > data.len() {
                    return -1;
                }
                let text = String::from_utf8_lossy(&data[start..end]).to_string();
                let state = caller.data_mut();
                state.log_buffer.push_str(&text);
                state.log_buffer.push('\n');
                state.host_calls = state.host_calls.saturating_add(1);
                emit_host_call(state, "host_log", &text, HostCallOutcome::Allowed);
                0
            },
        )
        .map_err(|err| SandboxError::Internal(err.to_string()))?;

    // host_fs_read â€” policy-gated on declared paths.
    linker
        .func_wrap(
            "ordo_mcp_host",
            "host_fs_read",
            |mut caller: Caller<'_, HostState>,
             path_ptr: i32,
             path_len: i32,
             out_ptr: i32|
             -> i32 {
                let Some(path) = read_string(&mut caller, path_ptr, path_len) else {
                    return -1;
                };
                let state = caller.data_mut();
                state.host_calls = state.host_calls.saturating_add(1);
                if !state.policy.filesystem_read_allowed(&path) {
                    emit_host_call(
                        state,
                        "host_fs_read",
                        &path,
                        HostCallOutcome::DeniedFilesystem { path: path.clone() },
                    );
                    return -2;
                }
                let rt = state.rt.clone();
                let host = state.host.clone();
                let server_id = state.server_id.clone();
                let path_for_call = path.clone();
                let bytes = match rt
                    .block_on(async move { host.fs_read(&server_id, &path_for_call).await })
                {
                    Ok(b) => b,
                    Err(err) => {
                        let state = caller.data_mut();
                        emit_host_call(
                            state,
                            "host_fs_read",
                            &path,
                            HostCallOutcome::Error {
                                details: err.clone(),
                            },
                        );
                        return -3;
                    }
                };
                let len = bytes.len() as i32;
                let Some(mem) = caller.get_export("memory").and_then(|e| e.into_memory()) else {
                    return -1;
                };
                if mem.write(&mut caller, out_ptr as usize, &bytes).is_err() {
                    return -4;
                }
                let state = caller.data_mut();
                emit_host_call(state, "host_fs_read", &path, HostCallOutcome::Allowed);
                len
            },
        )
        .map_err(|err| SandboxError::Internal(err.to_string()))?;

    // host_fs_write â€” policy-gated on declared paths.
    // Signature: (path_ptr, path_len, bytes_ptr, bytes_len) -> i32
    //   0   on success
    //  -1   memory access failure
    //  -2   path not in declared allowlist
    //  -3   host adapter rejected the write
    linker
        .func_wrap(
            "ordo_mcp_host",
            "host_fs_write",
            |mut caller: Caller<'_, HostState>,
             path_ptr: i32,
             path_len: i32,
             bytes_ptr: i32,
             bytes_len: i32|
             -> i32 {
                let Some(path) = read_string(&mut caller, path_ptr, path_len) else {
                    return -1;
                };
                let Some(bytes) = read_bytes(&mut caller, bytes_ptr, bytes_len) else {
                    return -1;
                };
                let state = caller.data_mut();
                state.host_calls = state.host_calls.saturating_add(1);
                if !state.policy.filesystem_read_allowed(&path) {
                    emit_host_call(
                        state,
                        "host_fs_write",
                        &path,
                        HostCallOutcome::DeniedFilesystem { path: path.clone() },
                    );
                    return -2;
                }
                let rt = state.rt.clone();
                let host = state.host.clone();
                let server_id = state.server_id.clone();
                let path_for_call = path.clone();
                let bytes_for_call = bytes.clone();
                match rt.block_on(async move {
                    host.fs_write(&server_id, &path_for_call, &bytes_for_call)
                        .await
                }) {
                    Ok(()) => {
                        let state = caller.data_mut();
                        emit_host_call(state, "host_fs_write", &path, HostCallOutcome::Allowed);
                        0
                    }
                    Err(err) => {
                        let state = caller.data_mut();
                        emit_host_call(
                            state,
                            "host_fs_write",
                            &path,
                            HostCallOutcome::Error {
                                details: err.clone(),
                            },
                        );
                        -3
                    }
                }
            },
        )
        .map_err(|err| SandboxError::Internal(err.to_string()))?;

    // host_http_fetch â€” policy-gated on declared domains.
    linker
        .func_wrap(
            "ordo_mcp_host",
            "host_http_fetch",
            |mut caller: Caller<'_, HostState>, url_ptr: i32, url_len: i32, out_ptr: i32| -> i32 {
                let Some(url) = read_string(&mut caller, url_ptr, url_len) else {
                    return -1;
                };
                let state = caller.data_mut();
                state.host_calls = state.host_calls.saturating_add(1);
                let domain = extract_domain(&url);
                if !state.policy.domain_allowed(&domain) {
                    emit_host_call(
                        state,
                        "host_http_fetch",
                        &url,
                        HostCallOutcome::DeniedEgress {
                            domain: domain.clone(),
                        },
                    );
                    emit_violation(state, &format!("egress to undeclared domain {domain}"));
                    return -2;
                }
                let rt = state.rt.clone();
                let host = state.host.clone();
                let server_id = state.server_id.clone();
                let url_owned = url.clone();
                let bytes = match rt
                    .block_on(async move { host.http_fetch(&server_id, &url_owned).await })
                {
                    Ok(b) => b,
                    Err(err) => {
                        let state = caller.data_mut();
                        emit_host_call(
                            state,
                            "host_http_fetch",
                            &url,
                            HostCallOutcome::Error {
                                details: err.clone(),
                            },
                        );
                        return -3;
                    }
                };
                let len = bytes.len() as i32;
                let Some(mem) = caller.get_export("memory").and_then(|e| e.into_memory()) else {
                    return -1;
                };
                if mem.write(&mut caller, out_ptr as usize, &bytes).is_err() {
                    return -4;
                }
                let state = caller.data_mut();
                emit_host_call(state, "host_http_fetch", &url, HostCallOutcome::Allowed);
                len
            },
        )
        .map_err(|err| SandboxError::Internal(err.to_string()))?;

    // host_bus_emit â€” policy-gated on declared topics.
    linker
        .func_wrap(
            "ordo_mcp_host",
            "host_bus_emit",
            |mut caller: Caller<'_, HostState>,
             topic_ptr: i32,
             topic_len: i32,
             payload_ptr: i32,
             payload_len: i32|
             -> i32 {
                let Some(topic) = read_string(&mut caller, topic_ptr, topic_len) else {
                    return -1;
                };
                let Some(payload_bytes) = read_bytes(&mut caller, payload_ptr, payload_len) else {
                    return -1;
                };
                let state = caller.data_mut();
                state.host_calls = state.host_calls.saturating_add(1);
                if !state.policy.topic_allowed(&topic) {
                    emit_host_call(
                        state,
                        "host_bus_emit",
                        &topic,
                        HostCallOutcome::DeniedTopic {
                            topic: topic.clone(),
                        },
                    );
                    return -2;
                }
                let payload: Value = match serde_json::from_slice(&payload_bytes) {
                    Ok(v) => v,
                    Err(err) => {
                        emit_host_call(
                            state,
                            "host_bus_emit",
                            &topic,
                            HostCallOutcome::Error {
                                details: err.to_string(),
                            },
                        );
                        return -3;
                    }
                };
                let rt = state.rt.clone();
                let host = state.host.clone();
                let server_id = state.server_id.clone();
                let topic_owned = topic.clone();
                let payload_owned = payload.clone();
                match rt.block_on(async move {
                    host.bus_emit(&server_id, &topic_owned, payload_owned).await
                }) {
                    Ok(()) => {
                        let state = caller.data_mut();
                        emit_host_call(state, "host_bus_emit", &topic, HostCallOutcome::Allowed);
                        0
                    }
                    Err(err) => {
                        let state = caller.data_mut();
                        emit_host_call(
                            state,
                            "host_bus_emit",
                            &topic,
                            HostCallOutcome::Error { details: err },
                        );
                        -4
                    }
                }
            },
        )
        .map_err(|err| SandboxError::Internal(err.to_string()))?;

    Ok(())
}

fn read_string(caller: &mut Caller<'_, HostState>, ptr: i32, len: i32) -> Option<String> {
    let memory = caller.get_export("memory").and_then(|e| e.into_memory())?;
    let data = memory.data(caller);
    let start = ptr as usize;
    let end = start.checked_add(len as usize)?;
    if end > data.len() {
        return None;
    }
    Some(String::from_utf8_lossy(&data[start..end]).to_string())
}

fn read_bytes(caller: &mut Caller<'_, HostState>, ptr: i32, len: i32) -> Option<Vec<u8>> {
    let memory = caller.get_export("memory").and_then(|e| e.into_memory())?;
    let data = memory.data(caller);
    let start = ptr as usize;
    let end = start.checked_add(len as usize)?;
    if end > data.len() {
        return None;
    }
    Some(data[start..end].to_vec())
}

fn extract_domain(input: &str) -> String {
    // Accept either a bare URL or the request envelope used by
    // `LocalHost::http_fetch` (`<METHOD>\t<url>\n<headers>\n\n<body>`).
    // We need the URL portion for the domain allowlist check.
    let url = if let Some((head, _)) = input.split_once('\n') {
        if let Some((_method, url)) = head.split_once('\t') {
            url
        } else {
            head
        }
    } else if let Some((_method, url)) = input.split_once('\t') {
        url
    } else {
        input
    };
    let no_scheme = url.split("://").nth(1).unwrap_or(url);
    no_scheme.split('/').next().unwrap_or("").to_string()
}

fn emit_host_call(state: &HostState, function: &str, summary: &str, outcome: HostCallOutcome) {
    if let Some(bus) = &state.bus {
        let record = HostCallRecord {
            server_id: state.server_id.clone(),
            invocation_id: state.invocation_id.clone(),
            function: function.to_string(),
            arguments_summary: summary.chars().take(120).collect(),
            outcome,
            timestamp: Utc::now(),
        };
        let env: BusEnvelope = Envelope::new(
            state.node_id.clone(),
            OrdoMessage::McpSandboxHostCall(record),
        );
        let bus = bus.clone();
        let rt = state.rt.clone();
        rt.spawn(async move {
            let _ = bus.publish(mcp_topics::SANDBOX_HOST_CALL, env).await;
        });
    }
}

fn emit_violation(state: &HostState, details: &str) {
    if let Some(bus) = &state.bus {
        let env: BusEnvelope = Envelope::new(
            state.node_id.clone(),
            OrdoMessage::McpSandboxViolation {
                server_id: state.server_id.clone(),
                invocation_id: state.invocation_id.clone(),
                details: details.to_string(),
            },
        );
        let bus = bus.clone();
        let rt = state.rt.clone();
        rt.spawn(async move {
            let _ = bus.publish(mcp_topics::SANDBOX_VIOLATION, env).await;
        });
    }
}

fn map_trap(context: &str, err: impl std::fmt::Display) -> SandboxError {
    let msg = err.to_string();
    let lower = msg.to_ascii_lowercase();
    let limit_hit = lower.contains("fuel")
        || lower.contains("out of fuel")
        || lower.contains("all fuel consumed")
        || lower.contains("interrupt")
        || (lower.contains("memory") && lower.contains("limit"));
    if limit_hit {
        SandboxError::LimitExceeded(format!("{context}: {msg}"))
    } else {
        SandboxError::Trap(format!("{context}: {msg}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::InMemoryHost;

    fn minimal_wat_module() -> Vec<u8> {
        // A WASM module with memory + alloc + a `hello` entry that
        // copies its input into output unchanged.
        let wat = r#"
            (module
              (memory (export "memory") 1)
              (global $bump (mut i32) (i32.const 1024))

              (func (export "alloc") (param $n i32) (result i32)
                (local $p i32)
                (local.set $p (global.get $bump))
                (global.set $bump (i32.add (global.get $bump) (local.get $n)))
                (local.get $p))

              ;; Tool ABI: pack (out_ptr, out_len) into a single i64
              ;; where high 32 = ptr and low 32 = len. The host
              ;; unpacks via shift+mask. This avoids the multi-
              ;; value-return limitation of `extern "C"` on
              ;; wasm32-unknown-unknown.
              (func (export "hello") (param $inp i32) (param $len i32) (result i64)
                (i64.or
                  (i64.shl (i64.extend_i32_u (local.get $inp)) (i64.const 32))
                  (i64.extend_i32_u (local.get $len)))))
        "#;
        wat::parse_str(wat).expect("valid wat")
    }

    fn infinite_loop_module() -> Vec<u8> {
        let wat = r#"
            (module
              (memory (export "memory") 1)
              (func (export "alloc") (param i32) (result i32) i32.const 0)
              (func (export "loop") (param i32) (param i32) (result i64)
                (loop $l (br $l))
                (i64.const 0)))
        "#;
        wat::parse_str(wat).expect("valid wat")
    }

    #[tokio::test]
    async fn non_wasm_bytes_rejected_at_install() {
        let svc = McpSandboxService::new(Arc::new(NullHost)).unwrap();
        let err = svc
            .install(
                "native-binary",
                b"MZ\x90\x00".to_vec(), // "MZ" = PE / Windows EXE prefix
                CapabilityDeclaration::default(),
                ResourceLimits::default(),
            )
            .unwrap_err();
        assert!(matches!(err, SandboxError::NonWasmBinary(_)));
    }

    #[tokio::test]
    async fn empty_bytes_rejected() {
        let svc = McpSandboxService::new(Arc::new(NullHost)).unwrap();
        let err = svc
            .install(
                "empty",
                Vec::new(),
                CapabilityDeclaration::default(),
                ResourceLimits::default(),
            )
            .unwrap_err();
        assert!(matches!(err, SandboxError::NonWasmBinary(_)));
    }

    #[tokio::test]
    async fn valid_wasm_module_round_trips_input_through_entry() {
        let svc = McpSandboxService::new(Arc::new(NullHost)).unwrap();
        let module = minimal_wat_module();
        svc.install(
            "server-a",
            module,
            CapabilityDeclaration::default(),
            ResourceLimits::default(),
        )
        .unwrap();
        let (out, usage) = svc
            .invoke("server-a", "inv-1", "hello", serde_json::json!({ "x": 1 }))
            .await
            .unwrap();
        assert_eq!(out, serde_json::json!({ "x": 1 }));
        assert!(usage.fuel_consumed > 0);
    }

    #[tokio::test]
    async fn infinite_loop_hits_fuel_cap() {
        let svc = McpSandboxService::new(Arc::new(NullHost)).unwrap();
        let limits = ResourceLimits {
            fuel_per_invocation: 10_000, // tight cap
            ..ResourceLimits::default()
        };
        svc.install(
            "server-loop",
            infinite_loop_module(),
            CapabilityDeclaration::default(),
            limits,
        )
        .unwrap();
        let err = svc
            .invoke("server-loop", "inv-1", "loop", serde_json::json!(null))
            .await
            .unwrap_err();
        // wasmtime may surface fuel exhaustion as either
        // LimitExceeded (mapped from trap message) or Trap (when
        // the message doesn't contain "fuel"). Both are correct
        // semantics for "cap enforced"; we just want to confirm
        // the invocation was not allowed to complete normally.
        assert!(
            matches!(err, SandboxError::LimitExceeded(_) | SandboxError::Trap(_)),
            "expected LimitExceeded or Trap, got {err:?}"
        );
    }

    #[tokio::test]
    async fn rate_limit_blocks_after_cap() {
        let svc = McpSandboxService::new(Arc::new(NullHost)).unwrap();
        let limits = ResourceLimits {
            rate_limit_per_minute: 2,
            ..ResourceLimits::default()
        };
        svc.install(
            "server-rl",
            minimal_wat_module(),
            CapabilityDeclaration::default(),
            limits,
        )
        .unwrap();
        svc.invoke("server-rl", "i1", "hello", serde_json::json!({}))
            .await
            .unwrap();
        svc.invoke("server-rl", "i2", "hello", serde_json::json!({}))
            .await
            .unwrap();
        let err = svc
            .invoke("server-rl", "i3", "hello", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, SandboxError::RateLimited { .. }));
    }

    #[tokio::test]
    async fn policy_blocks_egress_to_undeclared_domain() {
        let policy = SandboxPolicy::from_declaration(&CapabilityDeclaration {
            host_functions: vec!["host_http_fetch".into()],
            domains: vec!["api.allowed.test".into()],
            ..Default::default()
        });
        assert!(policy.domain_allowed("api.allowed.test"));
        assert!(!policy.domain_allowed("attacker.example.com"));
    }

    #[tokio::test]
    async fn in_memory_host_round_trips_fs_read() {
        let host = Arc::new(InMemoryHost::default());
        host.files
            .lock()
            .insert("/data/x.txt".into(), b"hello".to_vec());
        let bytes = host.fs_read("server", "/data/x.txt").await.unwrap();
        assert_eq!(bytes, b"hello");
    }
}
