//! `ordo webhooks` subcommands â€” thin HTTP wrappers over `/api/webhooks`.

use serde_json::{json, Value};

type DynError = Box<dyn std::error::Error + Send + Sync>;

fn default_origin() -> String {
    std::env::var("ORDO_CONTROL_ORIGIN").unwrap_or_else(|_| "http://127.0.0.1:4141".to_string())
}

pub fn run(args: &[String]) -> Result<(), DynError> {
    match args.first().map(String::as_str) {
        Some("list") | None => run_list(),
        Some("add") | Some("register") => run_add(&args[1..]),
        Some("delete") | Some("remove") | Some("rm") => run_delete(&args[1..]),
        Some("get") | Some("show") => run_get(&args[1..]),
        Some("help") | Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }
        Some(other) => {
            eprintln!("unknown `ordo webhooks` subcommand: {other}");
            print_help();
            std::process::exit(2);
        }
    }
}

fn print_help() {
    println!(
        "Usage:\n  \
         ordo webhooks list\n  \
         ordo webhooks add <url> --topics <topic1,topic2,...> [--secret <hex>] [--label <text>]\n  \
         ordo webhooks get <id>\n  \
         ordo webhooks delete <id>\n\n\
         Honors ORDO_CONTROL_ORIGIN (default: http://127.0.0.1:4141)."
    );
}

fn run_list() -> Result<(), DynError> {
    get_json("/api/webhooks")
}

fn run_get(args: &[String]) -> Result<(), DynError> {
    let id = args.first().ok_or("usage: ordo webhooks get <id>")?.clone();
    get_json(&format!("/api/webhooks/{id}"))
}

fn run_add(args: &[String]) -> Result<(), DynError> {
    let mut url_arg = None;
    let mut topics: Vec<String> = Vec::new();
    let mut secret: Option<String> = None;
    let mut label: Option<String> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--topics" => {
                let value = iter.next().ok_or("missing --topics value")?;
                topics = value
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            "--secret" => secret = Some(iter.next().ok_or("missing --secret value")?.clone()),
            "--label" => label = Some(iter.next().ok_or("missing --label value")?.clone()),
            other if !other.starts_with("--") && url_arg.is_none() => {
                url_arg = Some(other.to_string());
            }
            other => return Err(format!("unexpected arg: {other}").into()),
        }
    }
    let url_value = url_arg.ok_or("usage: ordo webhooks add <url> --topics ...")?;
    if topics.is_empty() {
        return Err("--topics is required".into());
    }
    let mut body = json!({
        "url": url_value,
        "topics": topics,
    });
    if let Some(s) = secret {
        body["secret"] = json!(s);
    }
    if let Some(l) = label {
        body["label"] = json!(l);
    }
    post_json("/api/webhooks", body)
}

fn run_delete(args: &[String]) -> Result<(), DynError> {
    let id = args
        .first()
        .ok_or("usage: ordo webhooks delete <id>")?
        .clone();
    let origin = default_origin();
    let url = format!("{origin}/api/webhooks/{id}");
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
