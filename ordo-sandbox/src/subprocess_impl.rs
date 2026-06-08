//! Native subprocess sandbox — the `Sandbox` impl the trait docs
//! anticipate ("a subprocess with `unshare`, etc."). Runs a real
//! command (cargo/rustc/python/node/pwsh/cmd) confined to a workspace
//! directory, with a wall-clock timeout, captured + size-capped
//! stdout/stderr, and process-tree kill on timeout.
//!
//! This is the HIGHER-PRIVILEGE runner: unlike `WasmtimeSandbox` it has
//! the host's filesystem reach under the workspace root and (by design,
//! see the `ordo-code` policy) network access, so it is compiled only
//! behind the `subprocess` feature and gated upstream. It is deliberately
//! LOW-LEVEL: it runs the exact command it is handed. The language →
//! program/args mapping and source-file materialization live upstream in
//! `ordo-code`; this keeps the runner a thin, auditable execution seam.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use tokio::process::Command;

use crate::types::{SandboxError, SandboxExecution, SandboxRequest, SandboxResult};
use crate::Sandbox;

/// Static configuration for the native runner. Set once at wiring time
/// (in `ordo-runtime`), not per call.
#[derive(Debug, Clone)]
pub struct SubprocessConfig {
    /// All execution is confined to this directory: a request's `cwd` is
    /// resolved under it and rejected if it escapes.
    pub root: PathBuf,
    /// Programs the runner is allowed to spawn. A request naming any
    /// other program is rejected before spawn. Empty = nothing allowed.
    pub allowed_programs: Vec<String>,
    /// Captured stdout/stderr are truncated to these many bytes so a
    /// runaway build log can't blow up memory or the bus.
    pub max_stdout_bytes: usize,
    pub max_stderr_bytes: usize,
}

impl Default for SubprocessConfig {
    fn default() -> Self {
        Self {
            root: PathBuf::from("."),
            allowed_programs: Vec::new(),
            max_stdout_bytes: 1 << 20,
            max_stderr_bytes: 1 << 20,
        }
    }
}

/// The native subprocess runner. See module docs.
pub struct SubprocessSandbox {
    config: SubprocessConfig,
}

impl SubprocessSandbox {
    pub fn new(config: SubprocessConfig) -> Self {
        Self { config }
    }
}

/// Wire-shape carried through `SandboxRequest.input` so the shared
/// `Sandbox` request/result structs need no new fields. The language →
/// program/args mapping lives upstream in `ordo-code`; this runner just
/// executes the exact command it is handed.
#[derive(Debug, Default, Deserialize)]
struct SubprocessSpec {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    stdin: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

#[async_trait]
impl Sandbox for SubprocessSandbox {
    fn name(&self) -> &'static str {
        "subprocess"
    }

    async fn execute(&self, request: SandboxRequest) -> SandboxResult {
        let limit_ms = request.limits.max_duration_ms.max(1);
        let spec: SubprocessSpec = serde_json::from_value(request.input)
            .map_err(|e| SandboxError::InvalidModule(format!("invalid native run spec: {e}")))?;

        if spec.program.is_empty() {
            return Err(SandboxError::InvalidModule("no program specified".into()));
        }
        if !self
            .config
            .allowed_programs
            .iter()
            .any(|p| p.eq_ignore_ascii_case(&spec.program))
        {
            return Err(SandboxError::Unavailable(format!(
                "program '{}' is not in the allowed list ({:?})",
                spec.program, self.config.allowed_programs
            )));
        }

        let cwd = resolve_within(&self.config.root, spec.cwd.as_deref())?;
        // The workspace root exists (created at boot), but a nested cwd
        // may not — create it so callers can target subdirectories.
        tokio::fs::create_dir_all(&cwd)
            .await
            .map_err(|e| SandboxError::Internal(format!("could not create cwd {}: {e}", cwd.display())))?;

        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args)
            .current_dir(&cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // The full parent environment is inherited (tokio's default) so
        // cargo/pip/npm find their toolchains + proxies and can fetch
        // dependencies — network is allowed by design. Per-call `env`
        // entries are overlaid on top.
        for (key, value) in &spec.env {
            cmd.env(key, value);
        }
        #[cfg(windows)]
        {
            // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP — no console
            // window pops, and the child leads its own group so a
            // timeout taskkill /T reaches the whole tree. `creation_flags`
            // is inherent on tokio's Windows `Command`.
            cmd.creation_flags(0x0800_0000 | 0x0000_0200);
        }

        let started = Instant::now();
        let mut child = cmd
            .spawn()
            .map_err(|e| SandboxError::Internal(format!("failed to spawn '{}': {e}", spec.program)))?;

        let pid = child.id();
        // Feed stdin CONCURRENTLY with collecting stdout/stderr. Writing
        // all of stdin before reading stdout deadlocks when the child
        // fills its stdout pipe while still reading stdin.
        let stdin_pipe = child.stdin.take();
        let stdin_data = spec.stdin.clone();
        let run = async move {
            let feed = async move {
                if let (Some(mut pipe), Some(data)) = (stdin_pipe, stdin_data) {
                    use tokio::io::AsyncWriteExt;
                    let _ = pipe.write_all(data.as_bytes()).await;
                    let _ = pipe.shutdown().await;
                }
            };
            let (_, output) = tokio::join!(feed, child.wait_with_output());
            output
        };
        let output = match tokio::time::timeout(Duration::from_millis(limit_ms), run).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return Err(SandboxError::Internal(format!("process error: {e}"))),
            Err(_) => {
                if let Some(pid) = pid {
                    kill_tree(pid).await;
                }
                return Err(SandboxError::LimitExceeded(format!(
                    "native execution exceeded {limit_ms} ms"
                )));
            }
        };

        let duration_ms = started.elapsed().as_millis() as u64;
        let stdout = cap_utf8(&output.stdout, self.config.max_stdout_bytes);
        let stderr = cap_utf8(&output.stderr, self.config.max_stderr_bytes);

        // A non-zero exit is a SUCCESSFUL run that returned an error
        // (e.g. a compile failure) — return Ok so the caller/model sees
        // exit_code + stderr, not an opaque sandbox error.
        Ok(SandboxExecution {
            output: json!({
                "exit_code": output.status.code(),
                "success": output.status.success(),
                "stderr": stderr,
                "program": spec.program,
                "args": spec.args,
                "cwd": cwd.display().to_string(),
            }),
            instructions_used: 0,
            duration_ms,
            stdout,
        })
    }
}

/// Truncate a byte buffer to at most `max` bytes, lossily decode as
/// UTF-8, and append a marker when truncated.
fn cap_utf8(bytes: &[u8], max: usize) -> String {
    if bytes.len() <= max {
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        let mut s = String::from_utf8_lossy(&bytes[..max]).into_owned();
        s.push_str("\n…[output truncated]");
        s
    }
}

/// Resolve a (relative) `requested` path under `root`, rejecting any
/// path that escapes the root after lexical `..` normalization.
fn resolve_within(root: &Path, requested: Option<&str>) -> Result<PathBuf, SandboxError> {
    let base = normalize_lexical(root);
    let joined = match requested {
        None | Some("") => base.clone(),
        Some(p) => {
            let candidate = Path::new(p);
            // Reject absolute paths AND Windows drive-relative / prefixed
            // paths (e.g. `C:foo`, `\\?\..`), which `is_absolute()` alone
            // misses and which `join` would re-anchor outside the root.
            if candidate.is_absolute()
                || candidate
                    .components()
                    .any(|c| matches!(c, Component::Prefix(_)))
            {
                return Err(SandboxError::Internal(
                    "cwd must be relative to the workspace root".into(),
                ));
            }
            normalize_lexical(&base.join(candidate))
        }
    };
    if joined.starts_with(&base) {
        Ok(joined)
    } else {
        Err(SandboxError::Internal(format!(
            "path '{}' escapes the workspace root",
            joined.display()
        )))
    }
}

/// Collapse `.`/`..` components without touching the filesystem.
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(windows)]
async fn kill_tree(pid: u32) {
    // taskkill /T kills the whole process tree (cargo→rustc, npm→node).
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output()
        .await;
}

#[cfg(not(windows))]
async fn kill_tree(pid: u32) {
    // Best effort: try the process group (negative pid) first to catch
    // descendants, then the pid itself. (Windows uses taskkill /T for a
    // guaranteed tree-kill.)
    let _ = Command::new("kill")
        .args(["-9", &format!("-{pid}")])
        .output()
        .await;
    let _ = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .output()
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SandboxLimits;

    fn request(input: serde_json::Value, max_duration_ms: u64) -> SandboxRequest {
        SandboxRequest {
            wasm_bytes: vec![],
            script: String::new(),
            input,
            entry: "ordo_entry".into(),
            limits: SandboxLimits {
                max_duration_ms,
                ..Default::default()
            },
        }
    }

    #[tokio::test]
    async fn runs_allowed_program_and_captures_output() {
        let dir = std::env::temp_dir().join(format!("ordo-subproc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        #[cfg(windows)]
        let (program, args) = ("cmd", vec!["/C".to_string(), "echo hi".to_string()]);
        #[cfg(not(windows))]
        let (program, args) = ("sh", vec!["-c".to_string(), "echo hi".to_string()]);

        let sandbox = SubprocessSandbox::new(SubprocessConfig {
            root: dir.clone(),
            allowed_programs: vec![program.to_string()],
            ..Default::default()
        });
        let exec = sandbox
            .execute(request(json!({ "program": program, "args": args }), 10_000))
            .await
            .expect("should run");
        assert!(exec.stdout.contains("hi"), "stdout: {}", exec.stdout);
        assert_eq!(exec.output["exit_code"], json!(0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn rejects_disallowed_program() {
        let sandbox = SubprocessSandbox::new(SubprocessConfig {
            root: std::env::temp_dir(),
            allowed_programs: vec!["cargo".to_string()],
            ..Default::default()
        });
        let err = sandbox
            .execute(request(json!({ "program": "definitely-not-allowed" }), 5_000))
            .await
            .expect_err("should reject");
        assert!(matches!(err, SandboxError::Unavailable(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn rejects_cwd_escape() {
        let err = resolve_within(Path::new("root/work"), Some("../../etc"))
            .expect_err("escape must be rejected");
        assert!(matches!(err, SandboxError::Internal(_)));
    }

    #[cfg(windows)]
    #[test]
    fn rejects_drive_relative_cwd() {
        let err = resolve_within(Path::new(r"C:\root\work"), Some("C:evil"))
            .expect_err("drive-relative cwd must be rejected");
        assert!(matches!(err, SandboxError::Internal(_)));
    }
}
