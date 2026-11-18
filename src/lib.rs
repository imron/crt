//! crt — Code Review Tool
//!
//! Terminal-based code review with LLM agent support. Built around a
//! client-server architecture where all state flows through a JSON-RPC
//! server.

pub mod app;
pub mod client;
pub mod db;
pub mod git;
pub mod keys;
pub mod markers;
pub mod mcp;
pub mod model;
pub mod search;
pub mod server;
pub mod ui;

use std::path::PathBuf;

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
fn cmd_review(base: Option<String>, reset: bool, _standalone: bool) -> Result<()> {
    let base = base.context("A base ref is required.\n\nUsage: crt <BASE>\n\nExample: crt main")?;

    let socket_path = default_socket_path()?;
    let rt = tokio::runtime::Runtime::new().context("Failed to create tokio runtime")?;

    rt.block_on(async {
        let client = client::Client::connect(&socket_path).await?;
        let cwd = std::env::current_dir().context("Failed to determine current directory")?;
        let init = client.init(&cwd.to_string_lossy(), &base).await?;

        if reset {
            println!(
                "Reset review state for (merge_base: {}, head: {})",
                &init.merge_base[..12],
                init.head_ref
            );
            println!("Not yet implemented (stage 9).");
            return Ok(());
        }

        println!("crt — Code Review Tool\n");
        println!("  repo root:   {}", init.repo_root);
        println!("  worktree:    {}", init.worktree);
        println!("  base ref:    {}", init.base_ref);
        println!("  merge base:  {}", &init.merge_base[..12]);
        println!("  head ref:    {}", init.head_ref);

        // list_changed_files is a stub for now — will error
        match client.list_changed_files().await {
            Ok(files) => println!("\n  Changed files: {files}"),
            Err(_) => println!("\n  (file listing not yet implemented via server)"),
        }

        println!("\nTUI not yet implemented (stage 6).");
        Ok(())
    })
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
