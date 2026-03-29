# Stage 1: Project Setup

## Goal

Establish the project foundation: directory structure, dependencies, CLI
argument parsing, and a minimal runnable binary that validates the base ref
argument against the current git repository.

## Why

Everything else builds on this. We need the module structure in place so
subsequent stages have clear homes for their code. We need CLI parsing so
we can accept `crt <base>`, `crt <base> --reset`, `crt server`,
`crt mcp-server`, `crt apply-comments <base>`, and `crt clear-comments
<base>`. We need to validate that the current directory is a git repository
and that the base ref resolves to a real commit, because every subsequent
feature depends on this.

## Requirements

1. **Cargo.toml** lists all planned dependencies (ratatui, crossterm, git2,
   rusqlite with bundled feature, clap with derive feature, sha2, anyhow,
   chrono, ignore, regex, serde, serde_json, tokio). Versions should be
   pinned to latest stable.

2. **CLI parsing** via clap with subcommands:
   - Default (positional `<base>`): launch TUI. Flag: `--reset`.
   - `server`: start persistent server.
   - `mcp-server`: start MCP adapter.
   - `apply-comments <base>`: write markers to files.
   - `clear-comments <base>`: remove markers from files.
   - Standard `--help` and `--version` derived automatically.

3. **Module structure** created with placeholder files:
   ```
   src/
     main.rs
     app.rs
     client.rs
     server/
       mod.rs
       api.rs
       notify.rs
     ui/
       mod.rs
       file_list.rs
       diff_view.rs
       comments.rs
     git.rs
     db.rs
     model.rs
     keys.rs
     search.rs
     mcp.rs
     markers.rs
   ```

4. **Repository validation**: on startup (for commands that need it), open
   the git repository from the current directory (or nearest parent) using
   git2. If not a git repository, exit with a clear error message.

5. **Worktree resolution**: resolve the current directory to both the
   worktree path and the main repository root (via `git commondir`). Store
   both for later use.

6. **Base ref validation**: resolve the provided base ref to a commit OID
   using git2. If the ref does not resolve, exit with a clear error message
   that includes the ref string the user provided.

7. **Head ref resolution**: resolve HEAD of the current worktree to a
   branch name. If HEAD is detached, produce a warning and use a fallback
   (worktree directory name or short commit hash).

8. **`.crt/` directory**: ensure the `.crt/` directory exists in the main
   repository root. Create it if it doesn't exist.

9. **`.gitignore` awareness**: if `.crt/` is not already in `.gitignore`,
   print a warning to stderr suggesting the user add it (do not modify
   `.gitignore` automatically).

## Acceptance Criteria

- [ ] `cargo build` succeeds with no errors.
- [ ] `cargo run -- main` (in a git repo with a `main` branch) prints
      something confirming the ref resolved, then exits cleanly.
- [ ] `cargo run -- nonexistent-ref` exits with a non-zero status and a
      human-readable error message.
- [ ] `cargo run` (no arguments) prints usage help and exits with non-zero
      status.
- [ ] `cargo run -- main --reset` runs without error (no-op for now since
      there's no DB yet, but the flag is parsed).
- [ ] `cargo run -- server`, `cargo run -- mcp-server`,
      `cargo run -- apply-comments main`, and
      `cargo run -- clear-comments main` are all recognized subcommands.
- [ ] Running outside a git repository exits with a clear error.
- [ ] The `.crt/` directory is created in the main repo root if absent.
- [ ] Worktree path and repo root are both correctly resolved (verified by
      running from a worktree if available).
- [ ] All source files in the module structure exist (may be mostly empty).

## Open Questions

- Should `crt` with no arguments show help, or should it try to auto-detect
  the base ref (e.g. the upstream tracking branch)?
- Should we support a config file (`.crt/config.toml`) for defaults like
  preferred base ref, diff mode, etc.? If so, this stage would be the place
  to define the config structure.
- What is the fallback for detached HEAD? Worktree directory name, short
  commit hash, or prompt the user?
