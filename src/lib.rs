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

use std::path::{Path, PathBuf};

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

    // Temporary: direct git calls until client library is built (Stage 5b)
    let cwd = std::env::current_dir().context("Failed to determine current directory")?;
    let repo = git::Repo::open(&cwd)?;
    let ctx = repo.context()?;

    if ctx.is_detached {
        eprintln!(
            "Warning: HEAD is detached at {}. Review state will be scoped to this commit hash.",
            ctx.head_ref
        );
    }

    repo.resolve_commit(&base)?; // validate base ref
    let merge_base = repo.merge_base(&base, "HEAD")?;

    // Ensure .crt/ directory exists
    let crt_dir = ctx.repo_root.join(".crt");
    if !crt_dir.exists() {
        std::fs::create_dir_all(&crt_dir)
            .with_context(|| format!("Failed to create {}", crt_dir.display()))?;
    }

    check_gitignore(&ctx.repo_root);

    if reset {
        println!(
            "Reset review state for (merge_base: {}, head: {})",
            &merge_base[..12],
            ctx.head_ref
        );
        println!("Not yet implemented (stage 9).");
        return Ok(());
    }

    println!("crt — Code Review Tool\n");
    println!("  repo root:   {}", ctx.repo_root.display());
    println!("  worktree:    {}", ctx.worktree.display());
    println!("  base ref:    {}", base);
    println!("  merge base:  {}", &merge_base[..12]);
    println!("  head ref:    {}", ctx.head_ref);
    println!("  db path:     {}", crt_dir.join("reviews.db").display());

    let changes = repo.list_changed_files(&merge_base, "HEAD")?;

    if changes.is_empty() {
        println!("\n  No changes between {} and HEAD.", &merge_base[..12]);
    } else {
        println!("\n  Changed files ({}):", changes.len());
        for change in &changes {
            let marker = match change.kind {
                git::ChangeKind::Added => "+",
                git::ChangeKind::Deleted => "-",
                git::ChangeKind::Modified => "~",
                git::ChangeKind::Renamed => "→",
            };
            if let Some(old) = &change.old_path {
                println!("    {} {} → {}", marker, old, change.path);
            } else {
                println!("    {} {}", marker, change.path);
            }
        }
    }

    println!("\nTUI not yet implemented (stage 6).");
    Ok(())
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

fn check_gitignore(repo_root: &Path) {
    let gitignore_path = repo_root.join(".gitignore");
    if gitignore_path.exists() {
        if let Ok(contents) = std::fs::read_to_string(&gitignore_path) {
            let has_crt = contents.lines().any(|line| {
                let trimmed = line.trim();
                trimmed == ".crt" || trimmed == ".crt/" || trimmed == "/.crt" || trimmed == "/.crt/"
            });
            if !has_crt {
                eprintln!(
                    "Warning: .crt/ is not in .gitignore. \
                     Consider adding it to avoid committing review state."
                );
            }
        }
    } else {
        eprintln!(
            "Warning: No .gitignore found. \
             Consider creating one and adding .crt/ to avoid committing review state."
        );
    }
}
