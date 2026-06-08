//! `ordo cloud` subcommands â€” credential CRUD against the local SQLite
//! store, plus a `test` command that actually exercises the configured
//! provider.

use std::path::PathBuf;

use ordo_cloud::{CloudCredentialStore, CloudCredentialUpdate, CloudHttp, CloudResult, Method};
use ordo_runtime::RuntimeConfig;
use serde_json::{json, Value};

type DynError = Box<dyn std::error::Error + Send + Sync>;

fn database_path() -> PathBuf {
    RuntimeConfig::local_default().database_path
}

fn open_store() -> CloudResult<CloudCredentialStore> {
    CloudCredentialStore::open(database_path())
}

pub fn run(args: &[String]) -> Result<(), DynError> {
    match args.first().map(String::as_str) {
        Some("list") | None => run_list(),
        Some("add") => run_add(&args[1..]),
        Some("delete") | Some("remove") | Some("rm") => run_delete(&args[1..]),
        Some("test") => run_test(&args[1..]),
        Some("help") | Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }
        Some(other) => {
            eprintln!("unknown `ordo cloud` subcommand: {other}");
            print_help();
            std::process::exit(2);
        }
    }
}

fn print_help() {
    println!(
        "Usage:\n  \
         ordo cloud list\n  \
         ordo cloud add <service> --secret <value> \\\n    \
             [--auth bearer|basic|api_key_header|api_key_query|anthropic] \\\n    \
             [--label <label>] [--base-url <url>] [--extra key=value ...]\n  \
         ordo cloud delete <service>\n  \
         ordo cloud test <service> [--prompt <text>]"
    );
}

fn run_list() -> Result<(), DynError> {
    let store = open_store()?;
    let credentials = store.list()?;
    let redacted: Vec<Value> = credentials.iter().map(|c| c.redacted()).collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "vault": store.vault_name(),
            "count": redacted.len(),
            "credentials": redacted,
        }))?
    );
    Ok(())
}

fn run_add(args: &[String]) -> Result<(), DynError> {
    if args.is_empty() {
        return Err("expected a service name as the first positional argument".into());
    }
    let service = args[0].clone();
    let mut update = CloudCredentialUpdate {
        service: service.clone(),
        ..Default::default()
    };
    let mut extras: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    let mut i = 1;
    while i < args.len() {
        let flag = args[i].as_str();
        let take_value = |i: &mut usize, flag: &str| -> Result<String, DynError> {
            *i += 1;
            args.get(*i)
                .cloned()
                .ok_or_else(|| format!("{flag} expected a value").into())
        };
        match flag {
            "--secret" => update.secret = Some(take_value(&mut i, "--secret")?),
            "--auth" | "--auth-style" => update.auth_style = Some(take_value(&mut i, "--auth")?),
            "--label" => update.label = Some(take_value(&mut i, "--label")?),
            "--base-url" => update.base_url = Some(take_value(&mut i, "--base-url")?),
            "--extra" => {
                let raw = take_value(&mut i, "--extra")?;
                let (key, value) = raw
                    .split_once('=')
                    .ok_or_else(|| "--extra expects key=value".to_string())?;
                extras.insert(key.trim().to_string(), value.trim().to_string());
            }
            other => return Err(format!("unknown flag: {other}").into()),
        }
        i += 1;
    }
    if !extras.is_empty() {
        update.extras = Some(extras);
    }
    if update.secret.is_none() {
        return Err(
            "no --secret supplied. Pass it explicitly or pipe the value via the environment."
                .into(),
        );
    }

    let mut store = open_store()?;
    let credential = store.upsert(update)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "vault": store.vault_name(),
            "credential": credential.redacted(),
        }))?
    );
    Ok(())
}

fn run_delete(args: &[String]) -> Result<(), DynError> {
    let service = args
        .first()
        .cloned()
        .ok_or_else(|| "expected a service name as the first positional argument".to_string())?;
    let mut store = open_store()?;
    let outcome = store.delete(&service)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "service": service,
            "removed": outcome.removed,
            "default_cleared": outcome.default_cleared,
        }))?
    );
    Ok(())
}

fn run_test(args: &[String]) -> Result<(), DynError> {
    let service = args
        .first()
        .cloned()
        .ok_or_else(|| "expected a service name as the first positional argument".to_string())?;
    let mut prompt =
        "Reply with a short acknowledgement so the operator knows the credential works."
            .to_string();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--prompt" {
            i += 1;
            prompt = args
                .get(i)
                .cloned()
                .ok_or_else(|| "--prompt expected a value".to_string())?;
        } else {
            return Err(format!("unknown flag: {}", args[i]).into());
        }
        i += 1;
    }

    let store = open_store()?;
    let credential = store
        .get(&service)?
        .ok_or_else(|| format!("no credential configured for '{service}'"))?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let http = CloudHttp::new();
        let result: Value = match credential.auth_style.as_str() {
            "anthropic" => {
                ordo_cloud::anthropic::messages(
                    &http,
                    &credential,
                    &json!({
                        "messages": [
                            { "role": "user", "content": prompt }
                        ],
                        "max_tokens": 64,
                    }),
                )
                .await?
            }
            "bearer" => {
                // Default OpenAI chat probe. Any bearer-auth vendor that
                // speaks the OpenAI API shape (Groq, TogetherAI, etc.)
                // also works here.
                ordo_cloud::openai::chat(
                    &http,
                    &credential,
                    &json!({
                        "messages": [
                            { "role": "user", "content": prompt }
                        ],
                        "temperature": 0.1,
                    }),
                )
                .await?
            }
            _ => {
                // Generic authenticated GET â€” operators using
                // api_key_header/api_key_query/basic can still ping the
                // root of their configured `base_url`.
                http.send_json(&credential, Method::GET, "", None, &[])
                    .await?
            }
        };
        println!("{}", serde_json::to_string_pretty(&result)?);
        Ok::<_, DynError>(())
    })?;
    Ok(())
}
