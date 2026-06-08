//! `ordo security` subcommands â€” inspect a running runtime's security
//! posture via the control API. There is no offline mode yet because
//! the audit log lives in the runtime's memory.

use serde_json::Value;

type DynError = Box<dyn std::error::Error + Send + Sync>;

fn default_origin() -> String {
    std::env::var("ORDO_CONTROL_ORIGIN").unwrap_or_else(|_| "http://127.0.0.1:4141".to_string())
}

pub fn run(args: &[String]) -> Result<(), DynError> {
    match args.first().map(String::as_str) {
        Some("audit") | None => run_audit(&args.iter().skip(1).cloned().collect::<Vec<_>>()),
        Some("rules") => run_rules(),
        Some("help") | Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }
        Some(other) => {
            eprintln!("unknown `ordo security` subcommand: {other}");
            print_help();
            std::process::exit(2);
        }
    }
}

fn print_help() {
    println!(
        "Usage:\n  \
         ordo security audit [--limit <n>]  recent audit events from the running runtime\n  \
         ordo security rules                built-in classifier inventory\n\n\
         Honors ORDO_CONTROL_ORIGIN (default http://127.0.0.1:4141)."
    );
}

fn run_audit(args: &[String]) -> Result<(), DynError> {
    let mut limit: u64 = 25;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--limit" {
            i += 1;
            limit = args
                .get(i)
                .ok_or_else(|| "--limit expects a value".to_string())?
                .parse()?;
        } else {
            return Err(format!("unknown flag: {}", args[i]).into());
        }
        i += 1;
    }

    let origin = default_origin();
    let url = format!("{origin}/api/security/audit?limit={limit}");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        let response = client.get(&url).send().await?;
        if !response.status().is_success() {
            return Err(format!("{url} returned {}", response.status()).into());
        }
        let body: Value = response.json().await?;
        println!("{}", serde_json::to_string_pretty(&body)?);
        Ok::<_, DynError>(())
    })?;
    Ok(())
}

fn run_rules() -> Result<(), DynError> {
    let origin = default_origin();
    let url = format!("{origin}/api/security/rules");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        let response = client.get(&url).send().await?;
        if !response.status().is_success() {
            return Err(format!("{url} returned {}", response.status()).into());
        }
        let body: Value = response.json().await?;
        println!("{}", serde_json::to_string_pretty(&body)?);
        Ok::<_, DynError>(())
    })?;
    Ok(())
}
