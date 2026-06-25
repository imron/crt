//! crt — Code Review Tool
//!
//! Terminal-based code review with LLM agent support. Built around a
//! client-server architecture where all state flows through a JSON-RPC
//! server.

pub mod app;
pub mod client;
pub mod config;
pub mod core;
pub mod db;
pub mod git;
pub mod markers;
pub mod mcp;
pub mod protocol;
pub mod review_types;
pub mod search;
pub mod server;
pub mod tui;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "crt",
    version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("CRT_BUILD_HASH"), ")"),
    about = "Code Review Tool"
)]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Base ref to diff against (e.g. "main", "v1.0", a commit hash)
    #[arg(value_name = "BASE")]
    base: Option<String>,

    /// Clear all review state for the current (base, branch) pair and exit
    #[arg(long)]
    reset: bool,

    /// Run with a private embedded server socket
    #[arg(long)]
    standalone: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Start persistent server (~/.crt/server.sock)
    Server,

    /// Start MCP adapter (stdio transport, connects to existing server)
    McpServer,

    /// Write review comment markers into worktree source files
    ApplyComments {
        /// Base ref for the review scope
        base: String,
    },

    /// Remove review comment markers from worktree source files
    ClearComments {
        /// Base ref for the review scope
        base: String,
    },
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run the application with command-line arguments.
pub fn run() -> Result<()> {
    let cli = Cli::parse();
    run_with_cli(cli)
}

/// Run with a pre-parsed CLI struct (useful for testing).
pub fn run_with_cli(cli: Cli) -> Result<()> {
    match cli.command {
        Some(Command::Server) => cmd_server(),
        Some(Command::McpServer) => {
            if cli.base.is_some() || cli.reset || cli.standalone {
                anyhow::bail!(
                    "`crt mcp-server` does not accept a base ref, --reset, or --standalone"
                );
            }
            cmd_mcp_server()
        }
        Some(Command::ApplyComments { .. }) => {
            println!("crt apply-comments: not yet implemented (stage 15)");
            Ok(())
        }
        Some(Command::ClearComments { .. }) => {
            println!("crt clear-comments: not yet implemented (stage 15)");
            Ok(())
        }
        None => cmd_review(cli.base, cli.reset, cli.standalone),
    }
}

// ---------------------------------------------------------------------------
// Subcommands
// ---------------------------------------------------------------------------

/// `crt server` — start persistent server.
fn cmd_server() -> Result<()> {
    let socket_path = app::default_socket_path()?;
    let rt = tokio::runtime::Runtime::new().context("Failed to create tokio runtime")?;
    rt.block_on(server::run_persistent(&socket_path))
}

/// Default `crt <base>` — review mode (will be TUI in Stage 6).
fn cmd_review(base: Option<String>, reset: bool, standalone: bool) -> Result<()> {
    let base = base.context("A base ref is required.\n\nUsage: crt <BASE>\n\nExample: crt main")?;

    let rt = tokio::runtime::Runtime::new().context("Failed to create tokio runtime")?;

    rt.block_on(async {
        match app::App::start_review(&base, reset, standalone).await? {
            app::ReviewStartup::Reset(summary) => {
                println!(
                    "Reset review state for (merge_base: {}, head: {}): {} review(s) cleared.",
                    summary.merge_base_short, summary.head_ref, summary.cleared
                );
                Ok(())
            }
            app::ReviewStartup::Review(app) => tui::run(app).await.context("TUI error"),
        }
    })
}

/// `crt mcp-server` — start MCP adapter over stdio.
fn cmd_mcp_server() -> Result<()> {
    let rt = tokio::runtime::Runtime::new().context("Failed to create tokio runtime")?;

    rt.block_on(async {
        let client = client::Client::connect_http_from_env().await?;
        mcp::run(client, None).await.context("MCP server error")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn mcp_server_has_no_base_option() {
        let err = match Cli::try_parse_from(["crt", "mcp-server", "--base", "main"]) {
            Ok(_) => panic!("mcp-server should not accept --base"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn mcp_server_rejects_global_base() {
        let cli = Cli::parse_from(["crt", "main", "mcp-server"]);
        let err = run_with_cli(cli).unwrap_err();

        assert!(
            err.to_string().contains("base ref"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn mcp_server_allows_unscoped_startup() {
        let cli = Cli::parse_from(["crt", "mcp-server"]);

        match cli.command {
            Some(Command::McpServer) => {}
            _ => panic!("expected mcp-server command"),
        }
    }

    #[test]
    fn mcp_server_rejects_standalone_flag() {
        let cli = Cli::parse_from(["crt", "--standalone", "mcp-server"]);
        let err = run_with_cli(cli).unwrap_err();

        assert!(
            err.to_string().contains("--standalone"),
            "unexpected error: {err}"
        );
    }
}
