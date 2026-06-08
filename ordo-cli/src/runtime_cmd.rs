//! `ordo runtime` subcommands â€” headless introspection of a running
//! runtime via its local control API.

use serde_json::Value;

type DynError = Box<dyn std::error::Error + Send + Sync>;

fn default_origin() -> String {
    std::env::var("ORDO_CONTROL_ORIGIN").unwrap_or_else(|_| "http://127.0.0.1:4141".to_string())
}

pub fn run(args: &[String]) -> Result<(), DynError> {
    match args.first().map(String::as_str) {
        Some("status") | None => run_status(),
        Some("help") | Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }
        Some(other) => {
            eprintln!("unknown `ordo runtime` subcommand: {other}");
            print_help();
            std::process::exit(2);
        }
    }
}

fn print_help() {
    println!(
        "Usage:\n  \
         ordo runtime status\n\n\
         Honors ORDO_CONTROL_ORIGIN (default: http://127.0.0.1:4141)."
    );
}

fn run_status() -> Result<(), DynError> {
    let origin = default_origin();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        let mut report = serde_json::Map::new();
        for (key, path) in [
            ("profile", "/api/runtime/profile"),
            ("storage", "/api/runtime/storage"),
            ("settings", "/api/runtime/settings"),
            ("capabilities", "/api/capabilities"),
            ("cloud_credentials", "/api/cloud/credentials"),
        ] {
            let url = format!("{origin}{path}");
            let response = client.get(&url).send().await;
            match response {
                Ok(response) => {
                    if !response.status().is_success() {
                        report.insert(
                            key.to_string(),
                            serde_json::json!({
                                "error": format!("{} returned {}", url, response.status()),
                            }),
                        );
                        continue;
                    }
                    let body: Value = response.json().await.unwrap_or(Value::Null);
                    report.insert(key.to_string(), body);
                }
                Err(err) => {
                    report.insert(
                        key.to_string(),
                        serde_json::json!({
                            "error": err.to_string(),
                            "url": url,
                        }),
                    );
                }
            }
        }
        println!("{}", serde_json::to_string_pretty(&Value::Object(report))?);
        Ok::<_, DynError>(())
    })?;
    Ok(())
}
