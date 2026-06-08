//! ordo-logic-mcp — binary entry point.
//!
//! Tonight: scaffold + version banner. Real MCP server (stdio JSON-RPC
//! per the protocol Ordo uses for the `mcp.servers.install` flow) is
//! the next session.
//!
//! Run-time contract once filled in:
//!   - reads MCP requests on stdin, writes responses on stdout
//!   - advertises capabilities: logic.satisfiability, logic.entailment,
//!     logic.equivalence, logic.normalize
//!   - install via the Studio MCP tab pointing at the path of this
//!     binary (after `cargo build --release -p ordo-logic-mcp`)

fn main() {
    let version = env!("CARGO_PKG_VERSION");
    eprintln!("ordo-logic-mcp v{version}");
    eprintln!("scaffold — full SAT handlers land in the next session.");
    eprintln!("install seam is wired; run this binary via the runtime's MCP install path.");
}
