mod app;
mod client;
mod db;
mod git;
mod keys;
mod markers;
mod mcp;
mod model;
mod search;
mod server;
mod ui;

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "crt", version, about = "Code Review Tool")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Base ref to diff against (e.g. "main", "v1.0", a commit hash)
    #[arg(value_name = "BASE", global = true)]
    base: Option<String>,

    /// Clear all review state for the current (base, branch) pair and exit
    #[arg(long, global = true)]
    reset: bool,
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

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Server) => {
            println!("crt server: not yet implemented (stage 5)");
            Ok(())
        }
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
        None => {
            // Default: TUI mode — requires a base ref
            let base = cli
                .base
                .context("A base ref is required.\n\nUsage: crt <BASE>\n\nExample: crt main")?;

            let repo_info = resolve_repo_context(&base)?;

            if cli.reset {
                println!(
                    "Reset review state for ({}, {}) — not yet implemented (stage 9)",
                    base, repo_info.head_ref
                );
                return Ok(());
            }

            println!("crt — Code Review Tool\n");
            println!("  repo root:  {}", repo_info.repo_root.display());
            println!("  worktree:   {}", repo_info.worktree.display());
            println!("  base ref:   {}", base);
            println!("  head ref:   {}", repo_info.head_ref);
            println!("  base commit: {}", &repo_info.base_commit_id[..12]);
            println!("  db path:    {}", repo_info.db_path.display());
            println!("\nTUI not yet implemented (stage 6).");

            Ok(())
        }
    }
}

struct RepoContext {
    repo_root: PathBuf,
    worktree: PathBuf,
    head_ref: String,
    base_commit_id: String,
    db_path: PathBuf,
}

fn resolve_repo_context(base: &str) -> Result<RepoContext> {
    let cwd = std::env::current_dir().context("Failed to determine current directory")?;

    // Open the repository from current directory (searches parents)
    let repo = git2::Repository::discover(&cwd)
        .context("Not a git repository (or any parent up to mount point)")?;

    // Resolve repo root (main repo, not worktree)
    // commondir() gives us the shared .git directory; its parent is the repo root
    let repo_root = if repo.is_worktree() {
        // For worktrees, commondir points to the main repo's .git dir
        let common = repo.commondir().to_path_buf();
        common.parent().map(|p| p.to_path_buf()).unwrap_or(common)
    } else {
        repo.workdir()
            .context("Bare repositories are not supported")?
            .to_path_buf()
    };

    // Worktree path is where we're actually running
    let worktree = repo
        .workdir()
        .context("Bare repositories are not supported")?
        .to_path_buf();

    // Resolve HEAD to a branch name
    let head_ref = match repo.head() {
        Ok(head) => {
            if head.is_branch() {
                head.shorthand().unwrap_or("HEAD").to_string()
            } else {
                // Detached HEAD — use short commit hash as fallback
                let oid = head.target().context("HEAD has no target")?;
                let short = &oid.to_string()[..8];
                eprintln!(
                    "Warning: HEAD is detached at {}. Review state will be scoped to this commit hash.",
                    short
                );
                short.to_string()
            }
        }
        Err(e) => {
            if e.code() == git2::ErrorCode::UnbornBranch {
                bail!("Repository has no commits yet. Make an initial commit before running crt.");
            }
            bail!("Failed to read HEAD: {}", e);
        }
    };

    // Resolve the base ref to a commit
    let base_obj = repo.revparse_single(base).with_context(|| {
        format!(
            "Could not resolve ref '{}'. Is it a valid branch, tag, or commit?",
            base
        )
    })?;
    let base_commit = base_obj
        .peel_to_commit()
        .with_context(|| format!("Ref '{}' does not point to a commit", base))?;
    let base_commit_id = base_commit.id().to_string();

    // Ensure .crt/ directory exists in repo root
    let crt_dir = repo_root.join(".crt");
    if !crt_dir.exists() {
        std::fs::create_dir_all(&crt_dir)
            .with_context(|| format!("Failed to create {}", crt_dir.display()))?;
    }

    let db_path = crt_dir.join("reviews.db");

    // Check if .crt/ is in .gitignore
    check_gitignore(&repo_root);

    Ok(RepoContext {
        repo_root,
        worktree,
        head_ref,
        base_commit_id,
        db_path,
    })
}

fn check_gitignore(repo_root: &PathBuf) {
    let gitignore_path = repo_root.join(".gitignore");
    if gitignore_path.exists() {
        if let Ok(contents) = std::fs::read_to_string(&gitignore_path) {
            let has_crt = contents.lines().any(|line| {
                let trimmed = line.trim();
                trimmed == ".crt" || trimmed == ".crt/" || trimmed == "/.crt" || trimmed == "/.crt/"
            });
            if !has_crt {
                eprintln!(
                    "Warning: .crt/ is not in .gitignore. Consider adding it to avoid committing review state."
                );
            }
        }
    } else {
        eprintln!(
            "Warning: No .gitignore found. Consider creating one and adding .crt/ to avoid committing review state."
        );
    }
}
