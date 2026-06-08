//! `ordo ext` subcommands â€” install, list, and uninstall UI extensions.
//!
//! `install` walks a local directory, base64-encodes every file
//! under it, and posts the bundle to `/api/ui-extensions/install`.
//! `ui.json` at the root is required (the runtime validates it).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
        Some("help") | Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }
        Some(other) => {
            eprintln!("unknown `ordo ext` subcommand: {other}");
            print_help();
            std::process::exit(2);
        }
    }
}

fn print_help() {
    println!(
        "Usage:\n  \
         ordo ext list\n  \
         ordo ext install <directory> [--name <override>]\n  \
         ordo ext uninstall <name>\n\n\
         Honors ORDO_CONTROL_ORIGIN (default: http://127.0.0.1:4141)."
    );
}

fn run_list() -> Result<(), DynError> {
    let origin = default_origin();
    let url = format!("{origin}/api/ui-extensions");
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
    let mut dir: Option<PathBuf> = None;
    let mut name_override: Option<String> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--name" => name_override = Some(iter.next().ok_or("missing --name value")?.clone()),
            other if !other.starts_with("--") && dir.is_none() => dir = Some(other.into()),
            other => return Err(format!("unexpected arg: {other}").into()),
        }
    }
    let dir = dir.ok_or("usage: ordo ext install <directory>")?;
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()).into());
    }
    let manifest_path = dir.join("ui.json");
    if !manifest_path.is_file() {
        return Err(format!("{} is missing ui.json", dir.display()).into());
    }

    let name = match name_override {
        Some(n) => n,
        None => dir
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
            .ok_or("could not infer extension name from directory")?,
    };

    let mut files: BTreeMap<String, String> = BTreeMap::new();
    walk_dir(&dir, &dir, &mut files)?;
    if !files.contains_key("ui.json") {
        return Err("walked tree does not contain ui.json at root".into());
    }

    let body = json!({
        "name": name,
        "files": files,
    });
    let origin = default_origin();
    let url = format!("{origin}/api/ui-extensions/install");
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
    let name = args
        .first()
        .ok_or("usage: ordo ext uninstall <name>")?
        .clone();
    let origin = default_origin();
    let url = format!("{origin}/api/ui-extensions/{name}");
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

fn walk_dir(
    root: &Path,
    cursor: &Path,
    files: &mut BTreeMap<String, String>,
) -> Result<(), DynError> {
    for entry in std::fs::read_dir(cursor)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_dir(root, &path, files)?;
        } else if path.is_file() {
            let rel = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = std::fs::read(&path)?;
            files.insert(rel, base64_encode_standard(&bytes));
        }
    }
    Ok(())
}

fn http_client() -> Result<reqwest::Client, DynError> {
    Ok(reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
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
