//! `ordo chat` â€” primary conversational entry point into the platform.
//!
//! - `ordo chat "your message"` â€” one-shot turn against the default
//!   (or `--session <id>`) session. Prints the assistant's reply.
//! - `ordo chat` with no args â€” interactive REPL. Each line is a turn
//!   in a fresh session (or the session passed via `--session`).
//!
//! All work goes through the running runtime's control API, so the
//! assistant keeps its memory across invocations.

use std::io::{self, Write};

use serde_json::{json, Value};

type DynError = Box<dyn std::error::Error + Send + Sync>;

fn default_origin() -> String {
    std::env::var("ORDO_CONTROL_ORIGIN").unwrap_or_else(|_| "http://127.0.0.1:4141".to_string())
}

pub fn run(args: &[String]) -> Result<(), DynError> {
    // Parse flags. The only two supported today are `--session <id>`
    // and `--no-rag` / `--no-memory` mostly for debugging.
    let mut session_id: Option<String> = None;
    let mut use_rag = true;
    let mut use_memory = true;
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--session" | "-s" => {
                i += 1;
                session_id = Some(
                    args.get(i)
                        .cloned()
                        .ok_or_else(|| "--session expected a value".to_string())?,
                );
            }
            "--no-rag" => use_rag = false,
            "--no-memory" => use_memory = false,
            "--help" | "-h" | "help" => {
                print_help();
                return Ok(());
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown flag: {other}").into());
            }
            _ => positional.push(args[i].clone()),
        }
        i += 1;
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()?;

        if positional.is_empty() {
            repl(client, session_id, use_rag, use_memory).await
        } else {
            let message = positional.join(" ");
            one_shot(
                &client,
                session_id.as_deref(),
                &message,
                use_rag,
                use_memory,
            )
            .await
        }
    })
}

fn print_help() {
    println!(
        "Usage:\n  \
         ordo chat \"your message\"           one-shot turn; prints reply\n  \
         ordo chat                             interactive REPL\n  \
         ordo chat --session <id> \"msg\"     continue an existing session\n  \
         ordo chat --no-rag --no-memory \"msg\"  disable retrieval for this turn\n\n\
         Honors ORDO_CONTROL_ORIGIN (default http://127.0.0.1:4141)."
    );
}

async fn one_shot(
    client: &reqwest::Client,
    session_id: Option<&str>,
    message: &str,
    use_rag: bool,
    use_memory: bool,
) -> Result<(), DynError> {
    let origin = default_origin();
    let result = submit_turn(client, &origin, session_id, message, use_rag, use_memory).await?;
    let response = result
        .get("turn")
        .and_then(|t| t.get("assistant_response"))
        .and_then(|v| v.as_str())
        .unwrap_or("(empty response)");
    let session = result.get("session_id").and_then(|v| v.as_str());
    println!("{response}");
    if let Some(id) = session {
        eprintln!("\n--- session: {id} ---");
    }
    print_retrieval_footer(&result);
    Ok(())
}

async fn repl(
    client: reqwest::Client,
    mut session_id: Option<String>,
    use_rag: bool,
    use_memory: bool,
) -> Result<(), DynError> {
    let origin = default_origin();
    if session_id.is_none() {
        eprintln!("[chat] Starting a fresh session. Type `/quit` to exit.\n");
    } else {
        eprintln!(
            "[chat] Resuming session {}\n",
            session_id.as_deref().unwrap_or("?")
        );
    }
    let stdin = io::stdin();
    loop {
        print!("you> ");
        io::stdout().flush().ok();
        let mut buffer = String::new();
        let read = stdin.read_line(&mut buffer)?;
        if read == 0 {
            eprintln!("\n[chat] EOF, goodbye.");
            return Ok(());
        }
        let message = buffer.trim();
        if message.is_empty() {
            continue;
        }
        if matches!(message, "/quit" | "/exit" | "/q") {
            eprintln!("[chat] bye.");
            return Ok(());
        }
        match submit_turn(
            &client,
            &origin,
            session_id.as_deref(),
            message,
            use_rag,
            use_memory,
        )
        .await
        {
            Ok(result) => {
                if session_id.is_none() {
                    session_id = result
                        .get("session_id")
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                    if let Some(id) = &session_id {
                        eprintln!("[chat] session {id}");
                    }
                }
                let response = result
                    .get("turn")
                    .and_then(|t| t.get("assistant_response"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("(empty response)");
                println!("assistant> {response}\n");
                print_retrieval_footer(&result);
            }
            Err(err) => {
                eprintln!("[chat] error: {err}\n");
            }
        }
    }
}

async fn submit_turn(
    client: &reqwest::Client,
    origin: &str,
    session_id: Option<&str>,
    user_message: &str,
    use_rag: bool,
    use_memory: bool,
) -> Result<Value, DynError> {
    let mut body = json!({
        "user_message": user_message,
        "use_rag": use_rag,
        "use_memory": use_memory,
    });
    if let Some(id) = session_id {
        body["session_id"] = json!(id);
    }
    let response = client
        .post(format!("{origin}/api/assistant/turn"))
        .json(&body)
        .send()
        .await?;
    let status = response.status();
    let payload: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let message = payload
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("control API returned an error");
        return Err(format!("{status}: {message}").into());
    }
    Ok(payload)
}

fn print_retrieval_footer(result: &Value) {
    let fact_count = result
        .get("retrieved_facts")
        .and_then(|v| v.as_array())
        .map(|arr| arr.len())
        .unwrap_or(0);
    let rag_count = result
        .get("retrieved_rag")
        .and_then(|v| v.as_array())
        .map(|arr| arr.len())
        .unwrap_or(0);
    if fact_count > 0 || rag_count > 0 {
        eprintln!("[chat] grounded on {fact_count} fact(s), {rag_count} rag hit(s)");
    }
}
