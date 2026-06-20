//! crt — Code Review Tool
//!
//! Terminal-based code review with LLM agent support. Built around a
//! client-server architecture where all state flows through a JSON-RPC
//! server.

pub mod app;
pub mod app_model;
mod app_update;
pub mod client;
pub mod config;
pub mod core;
pub mod db;
pub mod git;
pub mod markers;
pub mod mcp;
pub mod model;
pub mod protocol;
pub mod search;
pub mod server;
pub mod tui;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

/// How long to wait for an embedded server to become ready after startup.
const EMBEDDED_SERVER_STARTUP_DELAY: Duration = Duration::from_millis(50);

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
    #[arg(value_name = "BASE", global = true)]
    base: Option<String>,

    /// Clear all review state for the current (base, branch) pair and exit
    #[arg(long, global = true)]
    reset: bool,

    /// Run in standalone mode (in-process server, no socket)
    #[arg(long, global = true)]
    standalone: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Start persistent server (~/.crt/server.sock)
    Server,

    /// Start MCP adapter (stdio transport, connects to server)
    McpServer {
        /// Base ref for the review scope
        #[arg(long)]
        base: Option<String>,
    },

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
        Some(Command::McpServer { .. }) => {
            println!("crt mcp-server: not yet implemented (stage 14)");
            Ok(())
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
    let socket_path = default_socket_path()?;
    let rt = tokio::runtime::Runtime::new().context("Failed to create tokio runtime")?;
    rt.block_on(server::run_persistent(&socket_path))
}

/// Default `crt <base>` — review mode (will be TUI in Stage 6).
fn cmd_review(base: Option<String>, reset: bool, standalone: bool) -> Result<()> {
    let base = base.context("A base ref is required.\n\nUsage: crt <BASE>\n\nExample: crt main")?;

    let socket_path = default_socket_path()?;
    let rt = tokio::runtime::Runtime::new().context("Failed to create tokio runtime")?;

    rt.block_on(async {
        let (cancel_guard, client) = connect_or_start(&socket_path, standalone).await?;
        let cwd = std::env::current_dir().context("Failed to determine current directory")?;
        let init = client.init(&cwd.to_string_lossy(), &base).await?;

        if reset {
            let result = client.reset_reviews().await?;
            println!(
                "Reset review state for (merge_base: {}, head: {}): {} review(s) cleared.",
                git::short_hash(&init.merge_base),
                init.head_ref,
                result.cleared
            );
            drop(client);
            shutdown_embedded(cancel_guard).await;
            return Ok(());
        }

        // Launch the TUI.
        tui::run(client, init).await.context("TUI error")?;

        // Clean shutdown of embedded server if we started one.
        shutdown_embedded(cancel_guard).await;
        Ok(())
    })
}

/// Connect to a running server, or start an embedded one.
///
/// Returns a cancellation token (Some if we started an embedded server,
/// None if we connected to an existing one) and the client. The caller
/// must hold the cancel guard — dropping it shuts down the embedded server.
async fn connect_or_start(
    socket_path: &std::path::Path,
    standalone: bool,
) -> Result<(Option<tokio_util::sync::CancellationToken>, client::Client)> {
    if standalone {
        // Standalone mode: start embedded server on a temp socket
        let dir = std::env::temp_dir().join(format!("crt-{}", std::process::id()));
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create temp dir {}", dir.display()))?;
        let standalone_socket = dir.join("server.sock");
        let cancel = server::start_embedded(&standalone_socket).await?;
        // Brief pause for the listener to be ready
        tokio::time::sleep(EMBEDDED_SERVER_STARTUP_DELAY).await;
        let client = client::Client::connect(&standalone_socket).await?;
        return Ok((Some(cancel), client));
    }

    // Try connecting to existing server
    match client::Client::connect(socket_path).await {
        Ok(c) => Ok((None, c)),
        Err(_) => {
            // No server running — start embedded on the real socket
            let cancel = server::start_embedded(socket_path).await?;
            // Brief pause for the listener to be ready
            tokio::time::sleep(EMBEDDED_SERVER_STARTUP_DELAY).await;
            let client = client::Client::connect(socket_path)
                .await
                .context("Failed to connect to embedded server")?;
            Ok((Some(cancel), client))
        }
    }
}

/// Cancel an embedded server and wait briefly for cleanup.
async fn shutdown_embedded(cancel: Option<tokio_util::sync::CancellationToken>) {
    if let Some(token) = cancel {
        token.cancel();
        // Give the background task a moment to remove the socket
        tokio::time::sleep(EMBEDDED_SERVER_STARTUP_DELAY).await;
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Get the default socket path (~/.crt/server.sock).
pub fn default_socket_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    let crt_dir = PathBuf::from(home).join(".crt");
    if !crt_dir.exists() {
        std::fs::create_dir_all(&crt_dir)
            .with_context(|| format!("Failed to create {}", crt_dir.display()))?;
    }
    Ok(crt_dir.join("server.sock"))
}
