//! `ordo apps` subcommands â€” thin HTTP wrappers over `/api/apps`.

use std::path::PathBuf;

use serde_json::{json, Value};

type DynError = Box<dyn std::error::Error + Send + Sync>;

fn default_origin() -> String {
    std::env::var("ORDO_CONTROL_ORIGIN").unwrap_or_else(|_| "http://127.0.0.1:4141".to_string())
}

pub fn run(args: &[String]) -> Result<(), DynError> {
    match args.first().map(String::as_str) {
        Some("list") | None => run_list(&args[args.len().min(1)..]),
        Some("create") => run_create(&args[1..]),
        Some("get") | Some("show") => run_get(&args[1..]),
        Some("publish") => run_lifecycle(&args[1..], "publish"),
        Some("unpublish") => run_lifecycle(&args[1..], "unpublish"),
        Some("archive") => run_lifecycle(&args[1..], "archive"),
        Some("unarchive") => run_lifecycle(&args[1..], "unarchive"),
        Some("delete") | Some("remove") | Some("rm") => run_delete(&args[1..]),
        Some("events") => run_events(&args[1..]),
        Some("help") | Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }
        Some(other) => {
            eprintln!("unknown `ordo apps` subcommand: {other}");
            print_help();
            std::process::exit(2);
        }
    }
}

fn print_help() {
    println!(
        "Usage:\n  \
         ordo apps list [--status draft|published|archived] [--limit N]\n  \
         ordo apps create <slug> --name <name> [--description <text>] [--metadata <path-or-json>]\n  \
         ordo apps get <id>\n  \
         ordo apps publish <id>\n  \
         ordo apps unpublish <id>\n  \
         ordo apps archive <id>\n  \
         ordo apps unarchive <id>\n  \
         ordo apps delete <id>\n  \
         ordo apps events <id>\n\n\
         Honors ORDO_CONTROL_ORIGIN (default: http://127.0.0.1:4141)."
    );
}

fn run_list(args: &[String]) -> Result<(), DynError> {
    let mut params: Vec<(String, String)> = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--status" => params.push((
                "status".into(),
                iter.next().ok_or("missing --status value")?.clone(),
            )),
            "--limit" => params.push((
                "limit".into(),
                iter.next().ok_or("missing --limit value")?.clone(),
            )),
            other => return Err(format!("unexpected arg: {other}").into()),
        }
    }
    let origin = default_origin();
    let mut url = format!("{origin}/api/apps");
    if !params.is_empty() {
        let qs = params
            .iter()
            .map(|(k, v)| format!("{k}={}", urlencoding(v)))
            .collect::<Vec<_>>()
            .join("&");
        url.push('?');
        url.push_str(&qs);
    }
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

fn run_create(args: &[String]) -> Result<(), DynError> {
    let mut slug = None;
    let mut name = None;
    let mut description: Option<String> = None;
    let mut metadata: Option<Value> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--name" => name = Some(iter.next().ok_or("missing --name value")?.clone()),
            "--description" => {
                description = Some(iter.next().ok_or("missing --description value")?.clone())
            }
            "--metadata" => {
                let value = iter.next().ok_or("missing --metadata value")?.clone();
                let parsed: Value = if std::path::Path::new(&value).exists() {
                    serde_json::from_slice(&std::fs::read(&value)?)?
                } else {
                    serde_json::from_str(&value)?
                };
                metadata = Some(parsed);
            }
            other if !other.starts_with("--") && slug.is_none() => slug = Some(other.to_string()),
            other => return Err(format!("unexpected arg: {other}").into()),
        }
    }
    let slug = slug.ok_or("usage: ordo apps create <slug> --name <name>")?;
    let name = name.ok_or("--name is required")?;
    let mut body = json!({ "slug": slug, "name": name });
    if let Some(desc) = description {
        body["description"] = json!(desc);
    }
    if let Some(meta) = metadata {
        body["metadata"] = meta;
    }
    post_json("/api/apps", body)
}

fn run_get(args: &[String]) -> Result<(), DynError> {
    let id = args.first().ok_or("usage: ordo apps get <id>")?.clone();
    get_json(&format!("/api/apps/{id}"))
}

fn run_lifecycle(args: &[String], action: &str) -> Result<(), DynError> {
    let id = args
        .first()
        .ok_or_else(|| format!("usage: ordo apps {action} <id>"))?
        .clone();
    post_json(&format!("/api/apps/{id}/{action}"), json!({}))
}

fn run_delete(args: &[String]) -> Result<(), DynError> {
    let id = args.first().ok_or("usage: ordo apps delete <id>")?.clone();
    let origin = default_origin();
    let url = format!("{origin}/api/apps/{id}");
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

fn run_events(args: &[String]) -> Result<(), DynError> {
    let id = args.first().ok_or("usage: ordo apps events <id>")?.clone();
    get_json(&format!("/api/apps/{id}/events"))
}

fn get_json(path: &str) -> Result<(), DynError> {
    let origin = default_origin();
    let url = format!("{origin}{path}");
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

fn post_json(path: &str, body: Value) -> Result<(), DynError> {
    let origin = default_origin();
    let url = format!("{origin}{path}");
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

fn urlencoding(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

// quiet a couple unused warnings on `PathBuf` import for this simple shim
#[allow(dead_code)]
fn _unused_marker(_p: PathBuf) {}
