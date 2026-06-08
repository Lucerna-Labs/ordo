//! `ordo mcp` subcommands â€” install, list, inspect, uninstall,
//! quarantine, and re-authorize external MCP servers via the
//! running runtime's control API.
//!
//! Every command talks HTTP to `/api/mcp/*` so the CLI stays
//! out of the runtime's process space.

use std::path::PathBuf;

use serde_json::{json, Value};

type DynError = Box<dyn std::error::Error + Send + Sync>;

fn default_origin() -> String {
    std::env::var("ORDO_CONTROL_ORIGIN").unwrap_or_else(|_| "http://127.0.0.1:4141".to_string())
}

pub fn run(args: &[String]) -> Result<(), DynError> {
    match args.first().map(String::as_str) {
        Some("list") | None => run_list(),
        Some("install") => run_install(&args[1..]),
        Some("uninstall") | Some("remove") | Some("rm") => run_uninstall(&args[1..]),
        Some("quarantine") => run_quarantine(&args[1..]),
        Some("re-authorize") | Some("reauthorize") => run_re_authorize(&args[1..]),
        Some("inspect") | Some("show") => run_inspect(&args[1..]),
        Some("invoke") | Some("call") => run_invoke(&args[1..]),
        Some("help") | Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }
        Some(other) => {
            eprintln!("unknown `ordo mcp` subcommand: {other}");
            print_help();
            std::process::exit(2);
        }
    }
}

fn print_help() {
    println!(
        "Usage:\n  \
         ordo mcp list\n  \
         ordo mcp install <server-id> --module <path-to-wasm> --manifest <path-to-json>\n  \
         ordo mcp uninstall <server-id>\n  \
         ordo mcp quarantine <server-id> --reason <text>\n  \
         ordo mcp re-authorize <server-id> --manifest <path-to-json>\n  \
         ordo mcp inspect <server-id>\n  \
         ordo mcp invoke <server-id> <tool> [--args <json-or-path>]\n\n\
         The manifest JSON declares identity + capability + tool catalog. Shape:\n  \
         {{\n    \"identity\": {{ \"name\": ..., \"version\": ..., \"publisher\": ..., \"sigstore_cert\": [..u8..], \"identity_hash\": [..32..] }},\n    \"declaration\": {{ \"host_functions\": [...], \"domains\": [...], \"filesystem_paths\": [...], \"bus_topics\": [...], \"secret_classes\": [...] }},\n    \"tool_catalog\": [{{ \"name\": ..., \"description\": ..., \"input_schema\": {{}}, \"output_schema\": {{}}, \"risk_level\": \"read_only|mutating|sensitive|high_risk\" }}],\n    \"limits\": {{ \"fuel_per_invocation\": ..., \"memory_bytes\": ..., \"max_response_size_bytes\": ..., \"max_nesting_depth\": ..., \"rate_limit_per_minute\": ... }}\n  }}\n\n\
         Honors ORDO_CONTROL_ORIGIN (default: http://127.0.0.1:4141)."
    );
}

fn run_list() -> Result<(), DynError> {
    let origin = default_origin();
    let url = format!("{origin}/api/mcp/servers");
    block_on(async move {
        let client = http_client()?;
        let response = client.get(&url).send().await?;
        if !response.status().is_success() {
            return Err(format!("{} returned {}", url, response.status()).into());
        }
        let body: Value = response.json().await?;
        println!("{}", serde_json::to_string_pretty(&body)?);
        Ok(())
    })
}

fn run_install(args: &[String]) -> Result<(), DynError> {
    let mut server_id = None;
    let mut module_path: Option<PathBuf> = None;
    let mut manifest_path: Option<PathBuf> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--module" => module_path = Some(iter.next().ok_or("missing --module value")?.into()),
            "--manifest" => {
                manifest_path = Some(iter.next().ok_or("missing --manifest value")?.into())
            }
            other if !other.starts_with("--") && server_id.is_none() => {
                server_id = Some(other.to_string());
            }
            other => return Err(format!("unexpected arg: {other}").into()),
        }
    }
    let server_id = server_id.ok_or("server id is required (positional)")?;
    let module_path = module_path.ok_or("--module is required")?;
    let manifest_path = manifest_path.ok_or("--manifest is required")?;

    let module_bytes = std::fs::read(&module_path)
        .map_err(|err| format!("read {}: {err}", module_path.display()))?;
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(&manifest_path)
            .map_err(|err| format!("read {}: {err}", manifest_path.display()))?,
    )
    .map_err(|err| format!("parse {}: {err}", manifest_path.display()))?;

    let body = json!({
        "server_id": server_id,
        "module_b64": base64_encode_standard(&module_bytes),
        "identity": manifest.get("identity").ok_or("manifest missing `identity`")?,
        "declaration": manifest.get("declaration").ok_or("manifest missing `declaration`")?,
        "tool_catalog": manifest.get("tool_catalog").ok_or("manifest missing `tool_catalog`")?,
        "limits": manifest.get("limits"),
    });

    let origin = default_origin();
    let url = format!("{origin}/api/mcp/servers/install");
    block_on(async move {
        let client = http_client()?;
        let response = client.post(&url).json(&body).send().await?;
        let status = response.status();
        let payload: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(format!("{} returned {} {}", url, status, payload).into());
        }
        println!("{}", serde_json::to_string_pretty(&payload)?);
        Ok(())
    })
}

fn run_uninstall(args: &[String]) -> Result<(), DynError> {
    let server_id = args
        .first()
        .ok_or("usage: ordo mcp uninstall <server-id>")?
        .clone();
    let origin = default_origin();
    let url = format!("{origin}/api/mcp/servers/{server_id}");
    block_on(async move {
        let client = http_client()?;
        let response = client.delete(&url).send().await?;
        if !response.status().is_success() {
            return Err(format!("{} returned {}", url, response.status()).into());
        }
        println!("{}", response.text().await?);
        Ok(())
    })
}

fn run_quarantine(args: &[String]) -> Result<(), DynError> {
    let mut server_id = None;
    let mut reason = String::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--reason" => reason = iter.next().ok_or("missing --reason value")?.clone(),
            other if !other.starts_with("--") && server_id.is_none() => {
                server_id = Some(other.to_string());
            }
            other => return Err(format!("unexpected arg: {other}").into()),
        }
    }
    let server_id = server_id.ok_or("server id is required")?;
    if reason.is_empty() {
        reason = "manual quarantine".into();
    }
    let origin = default_origin();
    let url = format!("{origin}/api/mcp/servers/{server_id}/quarantine");
    let body = json!({ "reason": reason });
    block_on(async move {
        let client = http_client()?;
        let response = client.post(&url).json(&body).send().await?;
        let status = response.status();
        let payload: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(format!("{} returned {} {}", url, status, payload).into());
        }
        println!("{}", serde_json::to_string_pretty(&payload)?);
        Ok(())
    })
}

fn run_re_authorize(args: &[String]) -> Result<(), DynError> {
    let mut server_id = None;
    let mut manifest_path: Option<PathBuf> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--manifest" => {
                manifest_path = Some(iter.next().ok_or("missing --manifest value")?.into())
            }
            other if !other.starts_with("--") && server_id.is_none() => {
                server_id = Some(other.to_string());
            }
            other => return Err(format!("unexpected arg: {other}").into()),
        }
    }
    let server_id = server_id.ok_or("server id is required")?;
    let manifest_path = manifest_path.ok_or("--manifest is required")?;
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(&manifest_path)
            .map_err(|err| format!("read {}: {err}", manifest_path.display()))?,
    )?;
    let body = json!({
        "declaration": manifest.get("declaration").ok_or("manifest missing `declaration`")?,
        "tool_catalog": manifest.get("tool_catalog").ok_or("manifest missing `tool_catalog`")?,
    });
    let origin = default_origin();
    let url = format!("{origin}/api/mcp/servers/{server_id}/re-authorize");
    block_on(async move {
        let client = http_client()?;
        let response = client.post(&url).json(&body).send().await?;
        let status = response.status();
        let payload: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(format!("{} returned {} {}", url, status, payload).into());
        }
        println!("{}", serde_json::to_string_pretty(&payload)?);
        Ok(())
    })
}

fn run_invoke(args: &[String]) -> Result<(), DynError> {
    let mut server_id: Option<String> = None;
    let mut tool: Option<String> = None;
    let mut arguments: Value = json!({});
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--args" => {
                let value = iter.next().ok_or("missing --args value")?.clone();
                arguments = if std::path::Path::new(&value).exists() {
                    serde_json::from_slice(&std::fs::read(&value)?)?
                } else {
                    serde_json::from_str(&value)?
                };
            }
            other if !other.starts_with("--") && server_id.is_none() => {
                server_id = Some(other.to_string());
            }
            other if !other.starts_with("--") && tool.is_none() => {
                tool = Some(other.to_string());
            }
            other => return Err(format!("unexpected arg: {other}").into()),
        }
    }
    let server_id = server_id.ok_or("usage: ordo mcp invoke <server-id> <tool> [--args ...]")?;
    let tool = tool.ok_or("usage: ordo mcp invoke <server-id> <tool> [--args ...]")?;
    let origin = default_origin();
    let url = format!("{origin}/api/mcp/servers/{server_id}/invoke/{tool}");
    let body = json!({ "arguments": arguments });
    block_on(async move {
        let client = http_client()?;
        let response = client.post(&url).json(&body).send().await?;
        let status = response.status();
        let payload: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(format!("{} returned {} {}", url, status, payload).into());
        }
        println!("{}", serde_json::to_string_pretty(&payload)?);
        Ok(())
    })
}

fn run_inspect(args: &[String]) -> Result<(), DynError> {
    let server_id = args
        .first()
        .ok_or("usage: ordo mcp inspect <server-id>")?
        .clone();
    let origin = default_origin();
    let url = format!("{origin}/api/mcp/servers/{server_id}/lockfile");
    block_on(async move {
        let client = http_client()?;
        let response = client.get(&url).send().await?;
        if !response.status().is_success() {
            return Err(format!("{} returned {}", url, response.status()).into());
        }
        let body: Value = response.json().await?;
        println!("{}", serde_json::to_string_pretty(&body)?);
        Ok(())
    })
}

fn http_client() -> Result<reqwest::Client, DynError> {
    Ok(reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?)
}

fn block_on<F>(fut: F) -> Result<(), DynError>
where
    F: std::future::Future<Output = Result<(), DynError>>,
{
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(fut)
}

fn base64_encode_standard(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(ALPHABET[(b0 >> 2) as usize] as char);
        out.push(ALPHABET[(((b0 & 0b11) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(((b1 & 0b1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(b2 & 0b111111) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}
