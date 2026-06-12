//! `CodeService` — write + run code in a confined workspace.
//!
//! Owns a workspace root (all paths are confined under it) and two
//! `Sandbox` backends: a low-risk WASM runner (`code.run`, default) and
//! a higher-privilege native subprocess runner (`code.run_native`,
//! opt-in + gated). The language → program/args mapping for the native
//! runner lives HERE; the `SubprocessSandbox` stays a thin "run this
//! exact command" seam.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use ordo_sandbox::{Sandbox, SandboxLimits, SandboxRequest};
use serde_json::{json, Value};

use crate::types::CodePolicy;

/// Headless service behind the `code.*` / `workspace.*` capabilities.
#[derive(Clone)]
pub struct CodeService {
    workspace_root: PathBuf,
    wasm: Arc<dyn Sandbox>,
    native: Arc<dyn Sandbox>,
    policy: CodePolicy,
}

impl CodeService {
    pub fn new(
        workspace_root: PathBuf,
        wasm: Arc<dyn Sandbox>,
        native: Arc<dyn Sandbox>,
        policy: CodePolicy,
    ) -> Self {
        Self {
            workspace_root,
            wasm,
            native,
            policy,
        }
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    // ---- workspace.* (files) -------------------------------------

    pub async fn write_file(&self, args: &Value) -> Result<Value, String> {
        let rel = str_arg(args, "path")?;
        let content = str_arg(args, "content")?;
        let path = self.resolve_path(rel)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| e.to_string())?;
        }
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| e.to_string())?;
        Ok(json!({ "path": rel, "bytes": content.len() }))
    }

    pub async fn read_file(&self, args: &Value) -> Result<Value, String> {
        let rel = str_arg(args, "path")?;
        let path = self.resolve_path(rel)?;
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| e.to_string())?;
        Ok(json!({ "path": rel, "content": content }))
    }

    pub async fn list(&self, args: &Value) -> Result<Value, String> {
        let rel = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let dir = self.resolve_path(rel)?;
        let mut reader = tokio::fs::read_dir(&dir).await.map_err(|e| e.to_string())?;
        let mut entries = Vec::new();
        while let Some(entry) = reader.next_entry().await.map_err(|e| e.to_string())? {
            let file_type = entry.file_type().await.map_err(|e| e.to_string())?;
            entries.push(json!({
                "name": entry.file_name().to_string_lossy(),
                "is_dir": file_type.is_dir(),
            }));
        }
        Ok(json!({ "path": rel, "entries": entries }))
    }

    // ---- code.run (wasm) -----------------------------------------

    pub async fn run_wasm(&self, args: &Value) -> Result<Value, String> {
        let wasm_bytes = match args.get("wasm_base64").and_then(|v| v.as_str()) {
            Some(b64) => base64_decode(b64).map_err(|e| format!("invalid wasm_base64: {e}"))?,
            None => Vec::new(),
        };
        let script = args
            .get("script")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if wasm_bytes.is_empty() && script.is_empty() {
            return Err("provide `wasm_base64` (a compiled WASM module) or `script`".into());
        }
        let mut limits = SandboxLimits {
            max_duration_ms: args
                .get("max_duration_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(self.policy.default_timeout_ms),
            ..Default::default()
        };
        if let Some(v) = args.get("max_instructions").and_then(|v| v.as_u64()) {
            limits.max_instructions = v;
        }
        if let Some(v) = args.get("max_memory_bytes").and_then(|v| v.as_u64()) {
            limits.max_memory_bytes = v;
        }
        let request = SandboxRequest {
            wasm_bytes,
            script,
            input: args.get("input").cloned().unwrap_or(Value::Null),
            entry: args
                .get("entry")
                .and_then(|v| v.as_str())
                .unwrap_or("ordo_entry")
                .to_string(),
            limits,
        };
        let exec = self
            .wasm
            .execute(request)
            .await
            .map_err(|e| e.to_string())?;
        Ok(json!({
            "backend": self.wasm.name(),
            "output": exec.output,
            "stdout": exec.stdout,
            "instructions_used": exec.instructions_used,
            "duration_ms": exec.duration_ms,
        }))
    }

    // ---- code.run_native (subprocess) ----------------------------

    pub async fn run_native(&self, args: &Value) -> Result<Value, String> {
        if !self.policy.allow_native {
            return Err(
                "native code execution is disabled (build the runtime with --features \
                        native-exec and set ORDO_CODE_ALLOW_NATIVE=true)"
                    .into(),
            );
        }
        let language = args
            .get("language")
            .and_then(|v| v.as_str())
            .map(|s| s.to_lowercase());
        if let Some(lang) = &language {
            if !self.policy.enabled_languages.is_empty()
                && !self
                    .policy
                    .enabled_languages
                    .iter()
                    .any(|l| l.eq_ignore_ascii_case(lang))
            {
                return Err(format!(
                    "language '{lang}' is not enabled (enabled: {:?})",
                    self.policy.enabled_languages
                ));
            }
        }
        let cwd = args.get("cwd").and_then(|v| v.as_str()).map(str::to_string);
        let stdin = args
            .get("stdin")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let source = args.get("source").and_then(|v| v.as_str());
        let explicit_program = args.get("program").and_then(|v| v.as_str());
        let extra_args: Vec<String> = args
            .get("args")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        let (program, run_args) = self
            .resolve_invocation(
                language.as_deref(),
                explicit_program,
                source,
                extra_args,
                cwd.as_deref(),
            )
            .await?;

        let timeout_ms = args
            .get("timeout_ms")
            .or_else(|| args.get("max_duration_ms"))
            .and_then(|v| v.as_u64())
            .unwrap_or(self.policy.default_timeout_ms);

        tracing::info!(
            target: "ordo_code",
            program = %program,
            cwd = %cwd.as_deref().unwrap_or("."),
            "code.run_native executing"
        );

        let request = SandboxRequest {
            wasm_bytes: Vec::new(),
            script: String::new(),
            input: json!({
                "program": program,
                "args": run_args,
                "cwd": cwd,
                "stdin": stdin,
            }),
            entry: "ordo_entry".to_string(),
            limits: SandboxLimits {
                max_duration_ms: timeout_ms,
                ..Default::default()
            },
        };
        let exec = self
            .native
            .execute(request)
            .await
            .map_err(|e| e.to_string())?;
        Ok(json!({
            "backend": self.native.name(),
            "exit_code": exec.output.get("exit_code").cloned().unwrap_or(Value::Null),
            "success": exec.output.get("success").cloned().unwrap_or(Value::Bool(false)),
            "stdout": exec.stdout,
            "stderr": exec.output.get("stderr").cloned().unwrap_or(Value::Null),
            "duration_ms": exec.duration_ms,
        }))
    }

    /// Map (language, program, source, args) to a concrete (program,
    /// args) pair, materializing an inline `source` snippet into the
    /// workspace when needed. An explicit `program` always wins.
    async fn resolve_invocation(
        &self,
        language: Option<&str>,
        explicit_program: Option<&str>,
        source: Option<&str>,
        extra_args: Vec<String>,
        cwd: Option<&str>,
    ) -> Result<(String, Vec<String>), String> {
        if let Some(program) = explicit_program {
            // Escape hatch: run exactly what the caller asked for.
            return Ok((program.to_string(), extra_args));
        }
        let lang = language.ok_or("`language` or `program` is required")?;
        match lang {
            "python" | "py" => {
                self.snippet_invocation("python", "ordo_snippet.py", source, extra_args, cwd)
                    .await
            }
            "node" | "javascript" | "js" => {
                self.snippet_invocation("node", "ordo_snippet.js", source, extra_args, cwd)
                    .await
            }
            "shell" | "sh" | "bash" | "pwsh" | "powershell" => {
                let cmdline = match source {
                    Some(s) => s.to_string(),
                    None if !extra_args.is_empty() => extra_args.join(" "),
                    None => return Err("shell requires `source` or `args`".into()),
                };
                #[cfg(windows)]
                {
                    Ok(("cmd".to_string(), vec!["/C".to_string(), cmdline]))
                }
                #[cfg(not(windows))]
                {
                    Ok(("sh".to_string(), vec!["-c".to_string(), cmdline]))
                }
            }
            "rust" | "cargo" => {
                if source.is_some() {
                    return Err(
                        "for Rust, write project files with workspace.write_file then run \
                                program=\"cargo\" args=[\"run\"]; inline `source` isn't supported"
                            .into(),
                    );
                }
                let run_args = if extra_args.is_empty() {
                    vec!["run".to_string()]
                } else {
                    extra_args
                };
                Ok(("cargo".to_string(), run_args))
            }
            other => Err(format!(
                "unsupported language '{other}' (use rust|python|node|shell, or pass `program`)"
            )),
        }
    }

    async fn snippet_invocation(
        &self,
        program: &str,
        filename: &str,
        source: Option<&str>,
        mut extra_args: Vec<String>,
        cwd: Option<&str>,
    ) -> Result<(String, Vec<String>), String> {
        match source {
            Some(src) => {
                // Write the snippet into the (confined) cwd, then run it
                // by its relative name with cwd set on the process.
                let dir = self.resolve_path(cwd.unwrap_or(""))?;
                tokio::fs::create_dir_all(&dir)
                    .await
                    .map_err(|e| e.to_string())?;
                tokio::fs::write(dir.join(filename), src)
                    .await
                    .map_err(|e| e.to_string())?;
                let mut run_args = vec![filename.to_string()];
                run_args.append(&mut extra_args);
                Ok((program.to_string(), run_args))
            }
            None => Ok((program.to_string(), extra_args)),
        }
    }

    // ---- path confinement ----------------------------------------

    fn resolve_path(&self, requested: &str) -> Result<PathBuf, String> {
        let base = normalize_lexical(&self.workspace_root);
        let target = if requested.is_empty() {
            base.clone()
        } else {
            let candidate = Path::new(requested);
            // Reject absolute AND Windows drive-relative / prefixed paths
            // (e.g. `C:foo`) that `is_absolute()` alone misses.
            if candidate.is_absolute()
                || candidate
                    .components()
                    .any(|c| matches!(c, Component::Prefix(_)))
            {
                return Err("path must be relative to the workspace".to_string());
            }
            normalize_lexical(&base.join(candidate))
        };
        if target.starts_with(&base) {
            Ok(target)
        } else {
            Err(format!("path '{requested}' escapes the workspace"))
        }
    }
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("missing `{key}`"))
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

/// Minimal dependency-free base64 decoder for the optional
/// `wasm_base64` argument (matches ordo-files' approach).
fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    let mut buf = Vec::with_capacity(input.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for c in input.chars() {
        if c == '=' {
            break;
        }
        if c.is_whitespace() {
            continue;
        }
        let v: u32 = match c {
            'A'..='Z' => (c as u32) - b'A' as u32,
            'a'..='z' => (c as u32) - b'a' as u32 + 26,
            '0'..='9' => (c as u32) - b'0' as u32 + 52,
            '+' => 62,
            '/' => 63,
            _ => return Err(format!("invalid character '{c}'")),
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            buf.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ordo_sandbox::NullSandbox;

    fn service(allow_native: bool) -> (CodeService, PathBuf) {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        // Unique dir per call so parallel tests don't collide without
        // pulling in a temp-dir crate.
        let dir = std::env::temp_dir().join(format!(
            "ordo-code-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let svc = CodeService::new(
            dir.clone(),
            Arc::new(NullSandbox),
            Arc::new(NullSandbox),
            CodePolicy {
                allow_native,
                ..Default::default()
            },
        );
        (svc, dir)
    }

    #[tokio::test]
    async fn write_then_read_round_trips() {
        let (svc, dir) = service(false);
        svc.write_file(&json!({"path": "a/b.txt", "content": "hi"}))
            .await
            .expect("write");
        let read = svc
            .read_file(&json!({"path": "a/b.txt"}))
            .await
            .expect("read");
        assert_eq!(read["content"], json!("hi"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn write_rejects_escape() {
        let (svc, dir) = service(false);
        let err = svc
            .write_file(&json!({"path": "../escape.txt", "content": "x"}))
            .await
            .expect_err("must reject");
        assert!(err.contains("escapes"), "got: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn native_disabled_by_policy_is_actionable() {
        let (svc, dir) = service(false);
        let err = svc
            .run_native(&json!({"language": "python", "source": "print(1)"}))
            .await
            .expect_err("native must be disabled");
        assert!(err.contains("disabled"), "got: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
