mod apps_cmd;
mod chat_cmd;
mod cloud_cmd;
mod ext_cmd;
mod lifecycle;
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
    // Determine workspace root (where Cargo.toml lives)
    let workspace_root = std::env::current_dir()?;

    // Initialize boot state for the progress page
    let boot_state = lifecycle::new_boot_state();
    ordo_control::set_boot_state(boot_state.clone());

    // ── Phase 1: Ensure Studio UI is built ──────────────────────
    if !lifecycle::studio_is_built(&workspace_root) {
        if let Err(e) = lifecycle::build_studio(&workspace_root, boot_state.clone()).await {
            eprintln!("[ordo] {e}");
            eprintln!("[ordo] Studio build failed. Install Node.js 20+ and try again.");
            std::process::exit(1);
        }
    } else {
        let mut bs = boot_state.lock().await;
        bs.steps.insert("build_studio".into(), "done".into());
    }

    // ── Phase 2: Ensure Servo shell is built ────────────────────
    if let Err(e) = lifecycle::ensure_servo_shell(&workspace_root, boot_state.clone()).await {
        eprintln!("[ordo] {e}");
        eprintln!("[ordo] Servo shell build failed. Make sure Rust is installed.");
        std::process::exit(1);
    }

    // Ensure ANGLE DLLs on Windows
    if let Err(e) = lifecycle::ensure_angle_dlls(&workspace_root) {
        eprintln!("[ordo] Warning: {e}");
    }

    // ── Phase 3: Boot the runtime ───────────────────────────────
    {
        let mut bs = boot_state.lock().await;
        bs.steps.insert("start_runtime".into(), "active".into());
        bs.status_text = "Starting Ordo runtime…".into();
    }

    println!("--- Ordo: serving runtime (Ctrl+C or close window to stop) ---");
    let runtime = PlanningOrdoRuntime::boot(RuntimeConfig::local_default()).await?;

    {
        let mut bs = boot_state.lock().await;
        bs.steps.insert("start_runtime".into(), "done".into());
        bs.steps.insert("build_runtime".into(), "done".into());
    }

    println!(
        "[runtime] profile={} components={:?}",
        runtime.config().profile.as_str(),
        runtime.component_names()
    );
    if let Some(bind_addr) = &runtime.config().control_api_bind {
        println!("[runtime] control API:  http://{bind_addr}");
    }

    // ── Phase 4: Spawn Servo shell as child ─────────────────────
    let control_url = runtime
        .config()
        .control_api_bind
        .as_ref()
        .map(|addr| format!("http://{addr}"))
        .unwrap_or_else(|| "http://127.0.0.1:4141".to_string());

    let mut servo_child = match lifecycle::ServoChild::spawn(
        &workspace_root,
        &control_url,
        1560,
        980,
        boot_state.clone(),
    )
    .await
    {
        Ok(child) => child,
        Err(e) => {
            eprintln!("[ordo] Could not open Ordo window: {e}");
            eprintln!("[ordo] Runtime is running at {control_url}. Open it in a browser.");
            // Fall back to waiting for Ctrl+C
            let signal = wait_for_shutdown_signal().await;
            println!("\n[runtime] received {signal}; shutting down...");
            let database_path = runtime.config().database_path.clone();
            runtime.shutdown();
            let _ = checkpoint_wal_with_retry(&database_path).await;
            println!("[runtime] shutdown complete");
            return Ok(());
        }
    };

    // ── Phase 5: Wait for Servo to close, then clean up ─────────
    // When the Servo window closes, we kill the runtime — no orphaned processes.
    println!("[ordo] Ordo window is open. Close it to stop the runtime.");

    // Race: Servo exit vs shutdown signal
    let servo_exit = tokio::select! {
        result = servo_child.wait() => {
            println!("[ordo] Ordo window closed (exit code {}).", result.unwrap_or(-1));
            true
        }
        _ = wait_for_shutdown_signal() => {
            println!("\n[ordo] Shutdown signal received.");
            servo_child.kill();
            false
        }
    };

    let _ = servo_exit; // suppress unused warning

    println!("[runtime] shutting down components...");
    let database_path = runtime.config().database_path.clone();
    runtime.shutdown();

    match checkpoint_wal_with_retry(&database_path).await {
        Ok(stats) if stats.busy == 0 => println!("[runtime] WAL checkpoint complete ({stats})"),
        Ok(stats) => println!("[runtime] WAL still busy, will fold on next open ({stats})"),
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
