mod apps_cmd;
mod chat_cmd;
mod cloud_cmd;
mod ext_cmd;
mod mcp_cmd;
mod plugins_cmd;
mod runtime_cmd;
mod security_cmd;
mod webhooks_cmd;

use ordo_runtime::{init_tracing, PlanningOrdoRuntime, RuntimeConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    init_tracing();

    // Subcommand dispatch. Subcommands run synchronously and never
    // boot the full runtime (they hit the control API or operate
    // on local config). `ordo serve` boots the runtime and keeps
    // it alive; `ordo chat` is the primary user entrypoint.
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("cloud") => tokio::task::spawn_blocking(move || cloud_cmd::run(&args[1..]))
            .await
            .map_err(|err| Box::new(err) as Box<dyn std::error::Error + Send + Sync>)?,
        Some("runtime") => tokio::task::spawn_blocking(move || runtime_cmd::run(&args[1..]))
            .await
            .map_err(|err| Box::new(err) as Box<dyn std::error::Error + Send + Sync>)?,
        Some("plugins") => tokio::task::spawn_blocking(move || plugins_cmd::run(&args[1..]))
            .await
            .map_err(|err| Box::new(err) as Box<dyn std::error::Error + Send + Sync>)?,
        Some("security") => tokio::task::spawn_blocking(move || security_cmd::run(&args[1..]))
            .await
            .map_err(|err| Box::new(err) as Box<dyn std::error::Error + Send + Sync>)?,
        Some("chat") | Some("ask") => {
            tokio::task::spawn_blocking(move || chat_cmd::run(&args[1..]))
                .await
                .map_err(|err| Box::new(err) as Box<dyn std::error::Error + Send + Sync>)?
        }
        Some("mcp") => tokio::task::spawn_blocking(move || mcp_cmd::run(&args[1..]))
            .await
            .map_err(|err| Box::new(err) as Box<dyn std::error::Error + Send + Sync>)?,
        Some("apps") => tokio::task::spawn_blocking(move || apps_cmd::run(&args[1..]))
            .await
            .map_err(|err| Box::new(err) as Box<dyn std::error::Error + Send + Sync>)?,
        Some("webhooks") => tokio::task::spawn_blocking(move || webhooks_cmd::run(&args[1..]))
            .await
            .map_err(|err| Box::new(err) as Box<dyn std::error::Error + Send + Sync>)?,
        Some("ext") | Some("extensions") => {
            tokio::task::spawn_blocking(move || ext_cmd::run(&args[1..]))
                .await
                .map_err(|err| Box::new(err) as Box<dyn std::error::Error + Send + Sync>)?
        }
        Some("serve") => run_serve().await,
        Some("help") | Some("--help") | Some("-h") | None => {
            print_top_help();
            Ok(())
        }
        Some(other) => {
            eprintln!("unknown command: {other}\n");
            print_top_help();
            std::process::exit(2);
        }
    }
}

fn print_top_help() {
    println!(
        "Ordo CLI\n\n\
         Usage:\n  \
         ordo chat [message]      talk to the Assistant (primary entry point)\n  \
         ordo serve               boot the runtime and keep it running (Ctrl+C to stop)\n  \
         ordo cloud <subcommand>  manage cloud credentials (list/add/delete/test)\n  \
         ordo mcp <subcommand>    install/list/uninstall/quarantine/re-authorize external MCP servers\n  \
         ordo apps <subcommand>   create/list/publish/archive workflow apps via control API\n  \
         ordo ext <subcommand>    install/list/uninstall sandboxed UI extensions\n  \
         ordo webhooks <subcommand> manage outbound webhook subscriptions\n  \
         ordo plugins <subcommand> manage stdio plugins (list/enable/disable/install/uninstall)\n  \
         ordo security <subcommand> inspect audit events + classifier inventory\n  \
         ordo runtime status      introspect a running runtime via its control API\n  \
         ordo help                show this help\n\n\
         Environment:\n  \
         ORDO_LOG                 tracing filter (default: info)\n  \
         ORDO_LOG_JSON=1          emit structured JSON log lines\n  \
         ORDO_CONTROL_ORIGIN      target origin for `ordo runtime status`\n  \
         ORDO_CLOUD_VAULT         keyring|memory|plaintext (default: keyring)\n  \
         ORDO_CONTROL_API_BIND    control API bind address (default: 127.0.0.1:4141)"
    );
}

async fn run_serve() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("--- Ordo: serving runtime (Ctrl+C or close window to stop) ---");
    let runtime = PlanningOrdoRuntime::boot(RuntimeConfig::local_default()).await?;
    println!(
        "[runtime] profile={} components={:?}",
        runtime.config().profile.as_str(),
        runtime.component_names()
    );
    println!(
        "[runtime] database={} user_files={}",
        runtime.config().database_path.display(),
        runtime.config().user_files_path.display()
    );
    if let Some(bind_addr) = &runtime.config().control_api_bind {
        println!("[runtime] control API:  http://{bind_addr}");
        println!("[runtime] studio:       http://{bind_addr}/");
        println!("[runtime] dashboard:    http://{bind_addr}/dashboard");
        println!("[runtime] health check: http://{bind_addr}/health");
        println!("[runtime] capabilities: http://{bind_addr}/api/capabilities");
        println!("[runtime] credentials:  http://{bind_addr}/api/cloud/credentials");
    } else {
        println!("[runtime] control API disabled");
    }
    println!("[runtime] ready. waiting for shutdown signal (Ctrl+C / close window)...");

    // Wait for ANY signal that means "we are going away" — not just Ctrl+C.
    // Before this, `serve` only awaited `tokio::signal::ctrl_c()`, which on
    // Windows catches CTRL_C / CTRL_BREAK but NOT the console-close, logoff,
    // or system-shutdown events. Closing the (minimized) launcher console
    // therefore hard-killed the runtime with exit code -1 (0xFFFFFFFF), with
    // no graceful component shutdown and an uncheckpointed WAL. We now catch
    // every shutdown signal class and run the same clean path.
    let signal = wait_for_shutdown_signal().await;
    println!("\n[runtime] received {signal}; shutting down components...");

    // Capture the DB path before `shutdown` consumes the runtime, then fold
    // the write-ahead log back into the main database file once the runtime's
    // own connections have been torn down. WAL mode already keeps committed
    // data safe across a kill (it is replayed on the next open), so this is
    // about a clean, deterministic shutdown and a small on-disk footprint
    // rather than rescuing data.
    let database_path = runtime.config().database_path.clone();
    runtime.shutdown();
    match checkpoint_wal_with_retry(&database_path).await {
        Ok(stats) if stats.busy == 0 => println!("[runtime] WAL checkpoint complete ({stats})"),
        Ok(stats) => {
            // Still contended after the retry budget. Not data loss — WAL mode
            // replays the log on the next open — just a non-shrunk -wal file.
            println!("[runtime] WAL still busy, will fold on next open ({stats})");
        }
        Err(err) => eprintln!("[runtime] WAL checkpoint skipped: {err}"),
    }
    println!("[runtime] shutdown complete");
    Ok(())
}

/// Checkpoint the WAL on shutdown, retrying briefly while the runtime's
/// detached storage threads finish closing their connections.
///
/// `PlanningOrdoRuntime::shutdown` only *aborts* the component tasks; the
/// `StorageTask` OS threads that own the SQLite connections close a beat
/// later, when those aborts run their drop glue and drop the channel
/// senders. A checkpoint fired the instant `shutdown` returns therefore
/// races those threads and comes back `busy` having folded nothing. We
/// retry until an uncontended (`busy == 0`) checkpoint lands or the budget
/// (~1s, on top of each call's own 2s busy_timeout) is exhausted.
async fn checkpoint_wal_with_retry(
    path: &std::path::Path,
) -> Result<ordo_store::WalCheckpoint, Box<dyn std::error::Error + Send + Sync>> {
    let mut last = ordo_store::checkpoint_wal(path)?;
    for _ in 0..20 {
        if last.busy == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        last = ordo_store::checkpoint_wal(path)?;
    }
    Ok(last)
}

/// Block until the OS delivers a signal that means the runtime should stop.
///
/// On Windows this covers all five console control events. Note a hard
/// platform limit: for `CTRL_CLOSE` / `CTRL_LOGOFF` / `CTRL_SHUTDOWN` the OS
/// grants only a short grace period (~5s) after the handler is notified
/// before it force-terminates the process, so the graceful path is
/// best-effort for those. The guaranteed fix for the console-close vector is
/// to not run the runtime inside a closeable console window at all (see the
/// launcher hardening in the incident doc). Ctrl+C / Ctrl+Break are not
/// force-killed, so their graceful shutdown always completes.
#[cfg(windows)]
async fn wait_for_shutdown_signal() -> &'static str {
    use tokio::signal::windows;
    let mut ctrl_c = windows::ctrl_c().expect("register CTRL_C handler");
    let mut ctrl_break = windows::ctrl_break().expect("register CTRL_BREAK handler");
    let mut ctrl_close = windows::ctrl_close().expect("register CTRL_CLOSE handler");
    let mut ctrl_logoff = windows::ctrl_logoff().expect("register CTRL_LOGOFF handler");
    let mut ctrl_shutdown = windows::ctrl_shutdown().expect("register CTRL_SHUTDOWN handler");
    tokio::select! {
        _ = ctrl_c.recv() => "Ctrl+C",
        _ = ctrl_break.recv() => "Ctrl+Break",
        _ = ctrl_close.recv() => "console-close (CTRL_CLOSE)",
        _ = ctrl_logoff.recv() => "logoff (CTRL_LOGOFF)",
        _ = ctrl_shutdown.recv() => "system-shutdown (CTRL_SHUTDOWN)",
    }
}

/// Unix counterpart: stop on SIGINT (Ctrl+C) or SIGTERM (the signal
/// `docker stop`, `systemd`, and most supervisors send).
#[cfg(unix)]
async fn wait_for_shutdown_signal() -> &'static str {
    use tokio::signal::unix::{signal, SignalKind};
    let mut sigint = signal(SignalKind::interrupt()).expect("register SIGINT handler");
    let mut sigterm = signal(SignalKind::terminate()).expect("register SIGTERM handler");
    tokio::select! {
        _ = sigint.recv() => "SIGINT",
        _ = sigterm.recv() => "SIGTERM",
    }
}

/// Fallback for any platform that is neither Windows nor Unix.
#[cfg(not(any(windows, unix)))]
async fn wait_for_shutdown_signal() -> &'static str {
    let _ = tokio::signal::ctrl_c().await;
    "Ctrl+C"
}
