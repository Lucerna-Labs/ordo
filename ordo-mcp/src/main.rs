//! `ordo-mcp` binary entrypoint.
//!
//! Usage:
//!   $ ordo-mcp            # stdio transport (default)
//!   $ ordo-mcp --probe    # probe runtime, print status, exit
//!
//! Config resolution: see `config::Config::load`.

use std::process::ExitCode;

use ordo_mcp::{Config, RuntimeClient, Server};

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();

    let mut args = std::env::args().skip(1);
    let mut probe_only = false;
    let mut http_bind: Option<String> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--probe" => probe_only = true,
            "--http" => {
                http_bind = args.next();
                if http_bind.is_none() {
                    eprintln!("--http requires a <host:port> argument");
                    return ExitCode::from(2);
                }
            }
            "-h" | "--help" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("ordo-mcp {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument: {other}");
                print_help();
                return ExitCode::from(2);
            }
        }
    }

    let config = Config::load();
    tracing::info!(
        target: "ordo_mcp",
        runtime = %config.runtime_url,
        workspace = %config.workspace_id,
        "starting"
    );

    let client = match RuntimeClient::new(config.clone()) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("failed to build HTTP client: {err}");
            return ExitCode::FAILURE;
        }
    };

    // Fail-fast probe â€” it's far better to exit loudly here than to
    // hand Claude Desktop an MCP server whose every tool call returns
    // "connection refused."
    match client.probe().await {
        Ok(_) => tracing::info!(target: "ordo_mcp", "runtime probe ok"),
        Err(err) => {
            eprintln!(
                "Ordo runtime unreachable at {}: {}\n\
                 Start the runtime (cargo run -p ordo-cli) or set ORDO_URL.",
                config.runtime_url, err
            );
            if probe_only {
                return ExitCode::FAILURE;
            }
            // Don't exit â€” MCP clients reconnect; we'd rather keep
            // running and let the next probe succeed once the runtime
            // boots.
        }
    }

    if probe_only {
        println!("ok â€” runtime at {} is reachable", config.runtime_url);
        return ExitCode::SUCCESS;
    }

    let server = Server::new(client);
    if let Some(bind) = http_bind {
        if let Err(err) = ordo_mcp::transport::run_http(server, &bind).await {
            eprintln!("http transport failed: {err}");
            return ExitCode::FAILURE;
        }
    } else {
        ordo_mcp::transport::run_stdio(server).await;
    }
    ExitCode::SUCCESS
}

fn print_help() {
    println!(
        "ordo-mcp {}\n\n\
         A Model Context Protocol bridge that exposes a running Ordo\n\
         instance to MCP-speaking clients (Claude Desktop, Cursor, Cline, ...).\n\n\
         USAGE:\n\
            ordo-mcp [--probe] [--http <host:port>]\n\n\
         FLAGS:\n\
            --probe             Check that the runtime is reachable and exit.\n\
            --http <host:port>  Serve MCP over HTTP POST instead of stdio.\n\
            -V, --version       Print version and exit.\n\
            -h, --help          Print this help and exit.\n\n\
         ENV:\n\
            ORDO_URL         Runtime base URL (default http://127.0.0.1:4141)\n\
            ORDO_API_TOKEN   Bearer token sent on every runtime request.\n\
            ORDO_WORKSPACE   Default workspace_id (default 'local').\n\
            ORDO_MCP_CONFIG  Path to a JSON config file.\n",
        env!("CARGO_PKG_VERSION")
    );
}

fn init_tracing() {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .expect("env filter");
    // All logs go to stderr so stdout stays clean for the JSON-RPC
    // protocol stream. Claude Desktop surfaces stderr in its logs;
    // users and tests see structured output there.
    let layer = fmt::layer().with_writer(std::io::stderr).with_target(true);
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .try_init();
}
