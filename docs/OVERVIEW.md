# crt — Code Review Tool

## What Is This?

`crt` is a terminal-based code review tool built around a client-server
architecture. It targets developers who work in a rebase-driven workflow
and want a fast, keyboard-driven way to review changes on a branch — with
first-class support for providing feedback to LLM coding agents.

The primary use case: you are on a feature branch and want to review all
changes relative to a base ref (e.g. `main`). You run:

```
crt <base>
```

This opens a TUI showing every file changed in `base..HEAD`, with a diff
view for the selected file. You work through the files, marking each as
reviewed and leaving comments where needed. The next time you run the same
command — even after rebasing, amending commits, or adding new work — only
files whose diffs have actually changed are flagged for re-review.

Comments left during review are available to LLM agents via an MCP server
or as inline markers written directly into source files.

## Architecture Overview

### Client-Server Model

All state management, git operations, and review logic live in a **server**.
The TUI, MCP adapter, and CLI commands are all **clients** that communicate
with the server over JSON-RPC. Normal clients use the Unix domain socket;
sandboxed MCP clients use localhost HTTP.

```
┌───────────────────────────────────────────────────────┐
│                      crt server                       │
│                                                       │
│  ┌──────────┐  ┌───────────┐  ┌────────────────────┐ │
│  │ Git Ops  │  │  SQLite   │  │  Review Logic      │ │
│  │ (git2)   │  │ (per-repo)│  │  (state, anchoring │ │
│  │          │  │           │  │   diff hashing)    │ │
│  └──────────┘  └───────────┘  └────────────────────┘ │
│                                                       │
│  ┌───────────────────────────────────────────────┐   │
│  │      JSON-RPC API                             │   │
│  │      ~/.crt/server.sock + 127.0.0.1:25175     │   │
│  └───────────────────────────────────────────────┘   │
└──────────────────────┬────────────────────────────────┘
                       │
         ┌─────────────┼──────────────┐
         │             │              │
  ┌──────▼───────┐ ┌──▼─────────┐ ┌──▼─────────────┐
  │     TUI      │ │   MCP      │ │   Future       │
  │  (ratatui)   │ │  Adapter   │ │   Clients      │
  │  crt <base>  │ │  (stdio)   │ │   (web, CI..)  │
  └──────────────┘ └────────────┘ └────────────────┘
```

**Connection modes:**

- `crt <base>` — TUI tries to connect to a running server. If none is
  running, starts an embedded server in-process (transparent to the user).
  When the TUI exits, the embedded server shuts down.
- `crt server` — starts a persistent server that listens on
  `~/.crt/server.sock` and localhost HTTP JSON-RPC. Stays running until
  stopped. Supports multiple simultaneous clients.
- `crt mcp-server` — MCP adapter. Connects to a running server over localhost
  HTTP and bridges MCP stdio protocol to the server's JSON-RPC API.
- `--standalone` — starts a process-private embedded server socket under the
  system temp directory. It uses the same client/server transport without
  joining the shared `~/.crt/server.sock` session.

Embedded servers are process-coupled. If the hosting TUI exits, the embedded
server is cancelled and its socket is removed on a best-effort basis. Other
clients recover through the reconnect/failover supervisor by connecting to, or
racing to start, a replacement embedded server. `crt server` is the only
explicit persistent server mode. `crt mcp-server` does not start an embedded
server; it only connects to an existing shared server over localhost HTTP.

**Why client-server:**

- Single source of truth for all state. No concurrent DB access issues.
- Live updates: when an agent resolves a comment via MCP, the server pushes
  a notification to connected TUI clients. The TUI re-renders immediately.
- Clean separation of concerns: server owns state + logic, clients own
  presentation.
- Multi-repo: a persistent server can manage state for multiple
  repositories simultaneously.
- Future extensibility: web UI, CI integrations, etc. are just new clients.

### Worktree Awareness

The server distinguishes two paths for each client connection:

- **Data path**: the main repository root (resolved via `git commondir`).
  The SQLite database lives at `<repo_root>/.crt/reviews.db`. This is the
  same regardless of which worktree the client is in.
- **Working path**: the worktree where the client is running. File reads,
  diff computations, and marker writes operate against this path.

```
/path/to/repo/                ← main working tree
  .git/                       ← git directory
  .crt/reviews.db             ← review state (shared)

/path/to/agent-worktree/      ← agent's worktree
  .git                        ← file pointing to main .git
  src/...                     ← agent works here, markers written here
```

This means a reviewer in the main working tree and an agent in a worktree
can share review state while operating on different copies of the files.

### Review Scoping: `(merge_base, head_ref)`

All review state — file reviews, comments — is scoped by the pair
`(merge_base, head_ref)` where:

- `merge_base` is the **commit hash** of the common ancestor between the
  base ref and HEAD, computed via `git merge-base`. This is resolved
  automatically by the server during connection init.
- `head_ref` is the branch name at HEAD of the worktree, also resolved
  automatically.

The merge-base is the stable "fork point" — the commit where the feature
branch diverged from the base. Using it as the scope key solves several
problems:

- **Stable when base advances.** If `main` moves forward (new commits,
  merges from other branches), the merge-base of `main` and `feature-a`
  doesn't change. Review state is preserved without any re-review needed.
- **Different ref strings converge.** `crt main` and `crt abc123` produce
  the same scope if they have the same common ancestor with HEAD. This
  means a reviewer using `crt main` and an agent using `crt <hash>` see
  each other's comments.
- **Rebase creates a new scope.** When `feature-a` is rebased onto new
  main, the merge-base changes (correctly). The diff-hash mechanism
  preserves review state for files whose diffs haven't changed.
- **Relative refs resolve stably.** `crt HEAD~3` resolves to a specific
  merge-base hash. Adding a new commit changes the hash, which is correct
  — the review window has shifted.

This ensures that two feature branches (`feature-a`, `feature-b`) both
branching from `main` have completely separate review state, even though
they share the same repo and the same database. A reviewer and an agent
working on the same branch from different worktrees naturally share
state — they resolve to the same merge-base and head_ref.

### Connection Handshake

When a client connects, it sends an `init` request:

```
client → server:  init { worktree: "/path/to/agent-worktree", base_ref: "main" }
server resolves:  repo_root   → /path/to/repo  (via git commondir)
                  head_ref    → "feature-a"    (branch at HEAD of worktree)
                  merge_base  → abc123...      (merge-base main feature-a)
                  db_path     → /path/to/repo/.crt/reviews.db
server → client:  ok { repo_root, head_ref, merge_base, base_ref }
```

All subsequent requests on that connection are scoped to
`(merge_base, head_ref)` and operate against the specified worktree.
No need to repeat paths on every call. The original `base_ref` string
is returned for display purposes but is not used as a key.

## Design Decisions

### Storage: SQLite (Per-Repo)

Review state is stored in a SQLite database at `.crt/reviews.db` in the
main repository root (not in worktrees). This path should be added to
`.gitignore`.

Each repository has its own database. The server opens databases on demand
when clients connect. There is no central database.

**Why SQLite over git notes or flat files:**

- Review state is inherently relational (base ref + head ref + file path +
  review metadata). SQL is a natural fit.
- Queries like "which files changed since last review?" are trivial in SQL
  and awkward with flat files or git notes.
- Git notes are keyed per-commit, not per-file. Encoding per-file state
  inside a note body is clunky and doesn't merge well across reviewers.
- SQLite is fast, reliable, zero-config, and well-supported in Rust via
  `rusqlite`.
- Review state is personal (per-developer), so it does not need to travel
  with the repository. Local-only storage is the correct choice.
- WAL mode provides good concurrent read/write performance for the
  client-server model.

### Change Detection: Diff Hashing

When a file is marked as reviewed, we store a SHA-256 hash of its diff
content (`base..HEAD` for that file). On subsequent runs, we recompute the
diff hash and compare.

**Why this approach:**

- Rebase-proof. If you rebase but the file's diff is identical, the hash
  matches and the file stays reviewed.
- Handles base ref advancement. If `main` moves forward and the diff
  changes as a result, the hash will differ and the file is flagged for
  re-review. This is correct — you should verify your changes still look
  right against the new base.
- Simple to implement. We compute diffs anyway for display; hashing is
  negligible overhead.

### Git Interaction: git2

We use the `git2` crate (libgit2 Rust bindings) for all git operations:
listing changed files, computing diffs, reading blob OIDs, and resolving
worktree paths. This avoids shelling out to git for standard operations
and gives us structured data directly. For any edge case that git2 cannot
handle, we can fall back to shelling out to the git CLI.

### TUI Framework: Ratatui + Crossterm

Ratatui is the de-facto standard Rust TUI framework. Crossterm provides
the terminal backend. This combination is mature, well-documented, and
actively maintained.

### Symbol Navigation: Trait-Based with Ripgrep Backend

Go-to-definition (`Ctrl-]`) and codebase search (`:gr`) are backed by a
trait abstraction (`DefinitionFinder`). The initial implementation uses
the `ignore` and `regex` crates (the same libraries that power ripgrep)
to search the codebase for definition patterns.

**Why a trait:**

- Allows swapping in more sophisticated backends later (tree-sitter for
  AST-aware definitions, LSP for full language intelligence) without
  changing the rest of the application.
- The ripgrep-based backend is fast, has zero external dependencies,
  respects `.gitignore`, and provides ~70% accuracy which is sufficient
  for code review.

### No Formal Review Sessions

There is no concept of a "review session" that can be started or
completed. Review state is simply a mapping of
`(merge_base, head_ref, file_path) -> (content_id, reviewed_at)`.
`content_id` is an algorithm-independent pair of git blob OIDs
(`v1:{base_blob}:{workdir_blob}`). Running `crt <base>` always shows the
current state.

**Why:**

- A feature branch is a living thing until it's merged. There's no
  meaningful "complete" state while work is ongoing.
- Content identity naturally handles the lifecycle: as the branch
  evolves, files whose base/workdir blobs change are automatically
  flagged for re-review, independent of display diff algorithm.
- If you want a fresh start, `crt <base> --reset` clears all stored
  state for the current `(merge_base, head_ref)` scope.
- After the branch is merged, you simply never run `crt <base>` for it
  again.

### Comments: Feedback for LLM Agents

The primary consumers of review comments are LLM coding agents. When
reviewing code an agent has written, the reviewer attaches comments to
specific lines or ranges in the diff. The agent then reads the comments,
fixes the code, and the reviewer re-reviews only the changed files.

Comments are stored in SQLite alongside review state, with content-based
anchoring for rebase resilience. Each comment stores the anchor text (the
exact code being commented on) plus surrounding context lines. After a
rebase, the tool re-anchors comments by searching for the anchor text in
the new diff — if the code moved, the comment follows it.

**Two channels for agent consumption:**

1. **MCP server** (`crt mcp-server`) — a structured, queryable interface.
   The agent calls tools like `list_review_comments` and
   `resolve_comment` via the MCP adapter, which forwards to the server.
   This is the preferred channel for MCP-capable agents.
2. **File markers** (`crt apply-comments`) — writes `<<<<<<< REVIEW` /
   `=======` / `>>>>>>> REVIEW` blocks directly into source files in the
   worktree using the file's native comment syntax. Any agent that can
   read code sees the feedback. This is the universal fallback.

**Why two channels:**

- MCP provides rich metadata (resolved status, timestamps, structured
  queries) but requires MCP-capable tooling.
- File markers work with any agent that reads source code, but lack
  structure and modify the working tree.
- Having both means the tool works regardless of agent capabilities.

### Comment Anchoring: Rebase Resilience

Comments survive rebases via an append-only history model:

- `comments` table: stable logical record (id, scope, file_path, body,
  resolved, timestamps).
- `anchor_versions` table: append-only snapshots with `file_blob_sha` (exact
  file content), line/char ranges, `anchor_text`,
  `context_before`/`context_after`, and `AnchorStatus`.
- `v_current_anchors` view (GROUP BY `comment_id` + MAX(`created_at`),
  covering index) exposes the single latest version per comment.

On creation the first `anchor_versions` row records the exact
`file_blob_sha` the selection was made against plus its context.

Only **unresolved** comments are re-anchored. The server runs the four-step
match against current file content and `INSERT`s a new version row with
fresh context and the current `file_blob_sha`. The view automatically
reflects the latest attachment. Resolved comments are never re-anchored;
their last recorded version is the historical record.

For unresolved comments, re-anchoring proceeds as:

1. Exact match at the stored line → Anchored.
2. Exact match elsewhere in the file → Shifted, reattach with fresh context.
3. Context-assisted match → Approximate.
4. No match → Orphaned. The comment is still returned (visible in the panel)
   with its last-known context from the most recent anchor version so it is
   not silently lost.

## Layout

The TUI has a two-pane layout: file list on the left, diff/file view on
the right. Either pane can be hidden to give the other full width.

```
┌─ Unreviewed (3) ──────────────┐┌──────────────────────────────────────┐
│ ✗ src/main.rs                  ││ @@ -1,3 +1,15 @@                    │
│ ~ src/app.rs       (changed)   ││  fn main() {                        │
│ ✗ src/git.rs                   ││-    println!("Hello, world!");      │
│                                ││+    let args = Cli::parse();        │
│                                ││+    let repo = Repository::open(..);│
├─ Reviewed (3) ─────────────────┤│+    let app = App::new(args, repo); │
│ ✓ src/model.rs                 ││+    app.run()?;                     │
│ ✓ src/tui/input.rs             ││  }                                  │
│ ✓ Cargo.toml                   ││                                     │
└────────────────────────────────┘└──────────────────────────────────────┘
```

### File List Pane

The file list is split into two sections:

- **Unreviewed** (top): files that have never been reviewed, or whose diff
  has changed since the last review. Sorted alphabetically within the
  section.
- **Reviewed** (bottom): files whose diff matches what was reviewed. Sorted
  alphabetically within the section.

Status markers:
- `✗` — never reviewed
- `~` — previously reviewed, but diff has changed since (needs re-review)
- `✓` — reviewed, diff unchanged

A `●` to the left of the review marker means the file has unresolved
comments. To the right, `+`, `-`, and `R` mark added, deleted, and renamed
files; renames also show `new ← old`.

### Diff / File View Pane

The right pane has two independent display axes.

**View mode** cycles with `s`:

```
diff  →  full file (HEAD)  →  full file (base)  →  diff
```

**Render variant** toggles with `i`, and only applies in diff mode:
**inline** (unified diff) or **side-by-side**.

The diff base toggles with `m`, for files that have been reviewed before.
By default the diff runs from the commit the file was last reviewed at, so
only changes since that review are shown; `m` switches to the full diff
from the merge base.

When an **unreviewed** file is selected, the diff pane shows its diff.

When a **reviewed** file is selected, the diff pane shows a summary line:

```
src/model.rs — reviewed at 2026-03-29T14:30:00+10:00
```

Pressing `Enter` expands the full diff for that file.

### Comments Panel

A togglable panel for viewing all comments (current file or all files).
Unresolved comments are shown fully. Resolved comments are **collapsed**
by default (showing only a header line with file, line range, and body
preview). Pressing `Enter` on a collapsed resolved comment expands it.
Comments can be unresolved from the panel.

## Keybindings

Press `?` in the TUI for the same list. Note that `j`/`k` and the other
cursor motions always drive the diff cursor; the file list is navigated
with `Ctrl-n` / `Ctrl-p` rather than with `j`/`k`.

### Review

| Key              | Action                                              |
| ---------------- | --------------------------------------------------- |
| `a` / `A`        | Toggle the current file between approved and unapproved |
| `u`              | Undo the last review action                         |
| `Ctrl-n`         | Next file (cycles unreviewed first, then reviewed)  |
| `Ctrl-p`         | Previous file (same cycle order)                    |
| `Ctrl-Shift-n`   | Next file-list section                              |
| `Ctrl-Shift-p`   | Previous file-list section                          |

### Comments

| Key              | Action                                              |
| ---------------- | --------------------------------------------------- |
| `c` / `Space`    | Comment on the current line or selection            |
| `V`              | Line selection mode (select lines for commenting)   |
| `v`              | Character selection mode (select within lines)      |
| `Escape`         | Cancel the active selection                         |
| `Shift-C`        | Toggle the comments panel                           |
| `e`              | Edit the current comment                            |
| `r`              | Resolve or unresolve the current comment            |
| `d`              | Delete the current comment                          |
| `}` / `{`        | Next / previous unresolved comment                  |

`r` and `d` act on a comment only while one is current; otherwise `d`
cycles the diff algorithm.

### View

| Key              | Action                                              |
| ---------------- | --------------------------------------------------- |
| `s`              | Cycle: diff → full file (HEAD) → full file (base)    |
| `i`              | Toggle inline / side-by-side (diff mode only)        |
| `m`              | Toggle diff base: merge base / since last review     |
| `d`              | Cycle diff algorithm                                |
| `Ctrl-w`         | Toggle whitespace-insensitive diffing               |
| `Ctrl-b`         | Toggle blame annotations                            |
| `Tab`            | Switch focus between panes                          |
| `1`              | Toggle file list pane visibility                    |
| `2`              | Toggle diff pane visibility                         |

### Movement

| Key              | Action                                              |
| ---------------- | --------------------------------------------------- |
| `j` / `k`        | Move the cursor down / up one line                  |
| `h` / `l`        | Move the cursor left / right one column             |
| `Ctrl-d` / `Ctrl-u` | Half-page down / up                              |
| `Ctrl-e` / `Ctrl-y` | Scroll down / up one line                        |
| `g` / `G`        | Jump to top / bottom                                |
| `H` / `M` / `L`  | Cursor to top / middle / bottom of the viewport     |
| `0` / `$`        | Start / end of line                                 |
| `w` / `b`        | Word forward / back                                 |
| `W` / `B`        | Big-word forward / back                             |
| `]` / `[`        | Next / previous diff hunk                           |
| `Enter`          | Activate the selection in the focused pane          |

### Search and navigation

| Key              | Action                                              |
| ---------------- | --------------------------------------------------- |
| `/`              | Search the rendered diff                            |
| `n` / `N`        | Next / previous search match                        |
| `Escape`         | Clear the active diff search                        |
| `Ctrl-]`         | Go to definition of the symbol under the cursor     |
| `Ctrl-t`         | Jump back (pop the jump stack)                      |
| `:`              | Enter command mode                                  |

### Session

| Key              | Action                                              |
| ---------------- | --------------------------------------------------- |
| `?`              | Toggle the help overlay                             |
| `q`              | Quit                                                |
| `Ctrl-c`         | Press twice to quit                                 |
| `Ctrl-z`         | Suspend                                             |

### Mouse

| Action           | Effect                                              |
| ---------------- | --------------------------------------------------- |
| Click            | Select a file / set pane focus                      |
| Double-click     | Select the word under the pointer (auto-copy)       |
| Drag             | Select text (auto-copy)                             |
| Drag pane border | Resize the file list pane                           |
| Scroll           | Scroll the diff pane                                |

### Command Mode

| Command                   | Action                                     |
| ------------------------- | ------------------------------------------ |
| `:gr <regex>`             | Search across the whole codebase           |
| `:grd <regex>`            | Search across files in the diff only       |
| `:gd [symbol]`            | Go to definition; defaults to word at cursor |
| `:set blame` / `noblame`  | Show or hide blame annotations             |
| `:set comments` / `nocomments` | Show or hide inline comments          |
| `:set whitespace` / `nowhitespace` | Honour or ignore whitespace       |
| `:set mergebase[=<ref>]`  | Re-scope to a new merge base (alias `mb`)  |
| `:q` / `:quit`            | Quit                                       |

`:apply-comments` and `:clear-comments` are planned, alongside the file
marker support described below.

## Server API

All operations go through the server's JSON-RPC API. The TUI, MCP
adapter, and CLI subcommands are all clients. After `init`, all
operations are implicitly scoped to the connection's `(merge_base,
head_ref)` — no need to pass them on every call.

**Connection:**
- `init(worktree_path, base_ref)` — resolve repo context, compute
  merge-base, open DB, establish connection scope

**Review state** (scoped by connection):
- `list_changed_files()`
- `get_file_diff(file_path)`
- `get_file_content(file_path, version)`
- `mark_reviewed(file_path)`
- `unmark_reviewed(file_path)`
- `reset_reviews()`

**Comments** (scoped by connection):
- `create_comment(file_path, lines, body, ...)`
- `list_comments(file_path?, include_resolved?)`
- `get_comment(id)`
- `update_comment(id, body)`
- `resolve_comment(id)` / `unresolve_comment(id)`
- `delete_comment(id)`

**Markers** (scoped by connection):
- `apply_comments()`
- `clear_comments()`

**Search** (scoped by connection worktree):
- `search_codebase(pattern, scope?)`
- `find_definition(symbol, context_file?)`

**Server-level:**
- `track_repo(path)`
- `list_repos()`

## Module Structure

```
src/
  main.rs           -- thin binary entrypoint
  lib.rs            -- CLI parsing, connection/embedded server startup
  server/
    mod.rs          -- Server core, socket listener, connection handling
    api.rs          -- JSON-RPC method implementations
    notify.rs       -- Push notifications to connected clients
  client.rs         -- Client connection (Unix socket), request/response
  app.rs            -- App-owned state, config, projection, reducers
  app/
    model.rs        -- UI-agnostic AppModel projection
    update.rs       -- Private app-owned reducer implementation
  tui/
    mod.rs          -- Terminal UI adapter boundary
    runtime.rs      -- TUI event loop and terminal/runtime effects
    input.rs        -- Crossterm event normalization to app/core input
    effects.rs      -- TUI-side application of presentation effects
    state.rs        -- TUI-owned presentation and terminal state
    render/         -- Ratatui rendering and TUI-owned render caches
  core/
    interaction.rs  -- UI-neutral input interpretation
    input.rs        -- UI-neutral input event types
    command.rs      -- Command parsing and prompt semantics
    review.rs       -- Review workflow rules
    diff.rs         -- Diff/content/blame services
    navigation.rs   -- Navigation and cursor rules
    search.rs       -- Search/definition result shaping
  git.rs            -- Git operations via git2 (diffs, blobs, worktrees)
  db.rs             -- SQLite operations (reviews, comments, anchoring)
  review_types.rs   -- Shared review/session data types
  search.rs         -- Codebase search (:gr) and go-to-definition (Ctrl-])
  mcp.rs            -- rmcp-based MCP adapter over stdio
  markers.rs        -- Apply/clear review markers in worktree files
```

The current AppModel migration target is for `App` to expose a shared
UI-agnostic `AppModel`. TUI and future GUI adapters render that same model with
backend-specific layout and own their native hit maps.

## Dependencies

| Crate            | Purpose                                             |
| ---------------- | --------------------------------------------------- |
| `ratatui`        | TUI framework                                       |
| `crossterm`      | Terminal backend                                     |
| `git2`           | Git operations (libgit2 bindings)                    |
| `rusqlite`       | SQLite (with `bundled` feature)                      |
| `clap`           | CLI argument parsing (with `derive` feature)         |
| `sha2`           | Hashing diff content for change detection            |
| `anyhow`         | Error handling                                       |
| `chrono`         | Timestamps for review metadata                       |
| `ignore`         | Gitignore-aware file traversal (from ripgrep)        |
| `regex`          | Pattern matching for search and go-to-definition     |
| `serde`          | Serialization for JSON-RPC messages                  |
| `serde_json`     | JSON parsing/generation                              |
| `rmcp`           | Model Context Protocol server/stdio adapter          |
| `tokio`          | Async runtime (server socket handling)               |

## Data Model

```sql
CREATE TABLE file_reviews (
    file_path   TEXT NOT NULL,
    merge_base  TEXT NOT NULL,          -- commit hash of common ancestor
    head_ref    TEXT NOT NULL,
    diff_hash   TEXT NOT NULL,          -- legacy; cleared on new writes
    content_id  TEXT NOT NULL,          -- v1:{base_blob}:{workdir_blob}
    reviewed_at TEXT NOT NULL,
    PRIMARY KEY (merge_base, head_ref, file_path)
);

CREATE TABLE comments (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    merge_base      TEXT NOT NULL,      -- commit hash of common ancestor
    head_ref        TEXT NOT NULL,
    file_path       TEXT NOT NULL,
    line_start      INTEGER NOT NULL,
    line_end        INTEGER NOT NULL,
    char_start      INTEGER,            -- NULL for full-line (V) selections
    char_end        INTEGER,            -- NULL for full-line (V) selections
    anchor_text     TEXT NOT NULL,
    context_before  TEXT NOT NULL DEFAULT '',
    context_after   TEXT NOT NULL DEFAULT '',
    body            TEXT NOT NULL,
    resolved        INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);
```

Both tables are scoped by `(merge_base, head_ref)`. The `merge_base` is
the commit hash of the common ancestor between the user's base ref and
HEAD, computed via `git merge-base`. The `head_ref` is the branch name
at HEAD of the worktree (e.g. `"feature-a"`), resolved automatically by
the server. This means `crt main` from `feature-a` and `crt main` from
`feature-b` have completely separate review state (different merge-bases).
Two clients using different ref strings that resolve to the same ancestor
share state (same merge-base).

## CLI

```
crt <base>                    TUI — review base..HEAD (connects to a
                              server, or starts an embedded one)
crt <base> --reset            Clear all review state for (base, current branch)
crt server                    Start persistent server (~/.crt/server.sock)
crt mcp-server                Start MCP adapter (stdio, connects to server)
crt apply-comments <base>     Write review markers into worktree files
crt clear-comments <base>     Remove review markers from worktree files
```

## MCP Server

The `crt mcp-server` subcommand starts an MCP adapter that bridges the MCP
stdio protocol to the crt server's JSON-RPC API. It connects to an existing
server over localhost HTTP and does not start an embedded server. Start a TUI
session or explicit `crt server` first.

By default the server listens on `127.0.0.1:25175` for HTTP JSON-RPC. Override
the MCP connection target by passing environment variables to `crt mcp-server`:

```
CRT_SERVER_HOST=127.0.0.1
CRT_SERVER_PORT=25175
```

MCP starts unscoped. Use `list_review_sessions` to discover active review
sessions registered by connected crt clients, then `select_review_session` to
choose the session for scoped tools.

### Tools

| Tool                    | Description                                    |
| ----------------------- | ---------------------------------------------- |
| `list_review_sessions`  | Active review sessions from connected clients. |
| `select_review_session` | Select a session for the scoped tools.         |
| `list_changed_files`    | Changed files, with review and diff metadata.  |
| `list_file_statuses`    | Compact review status, without diff hunks.     |
| `get_file_diff`         | Diff content for one file (base..HEAD).        |
| `search_codebase`       | Regex search across the worktree or the diff.  |
| `find_definition`       | Best-effort symbol definition lookup.          |
| `create_review_comment` | Comment on a line range in the review scope.   |
| `list_review_comments`  | Comments in scope, filtered by file/status.    |
| `get_comment_detail`    | Full context for one review comment.           |
| `resolve_comment`       | Mark a review comment resolved.                |
| `unresolve_comment`     | Mark a review comment unresolved.              |
| `mark_file_reviewed`    | Mark a changed file reviewed.                  |
| `unmark_file_reviewed`  | Clear a changed file's reviewed state.         |
| `list_review_summary`   | Files changed, review progress, comment counts. |

## File Markers

Review comments can be written directly into source files in the worktree
as inline markers. This lets any agent that reads source code see the
feedback, regardless of MCP support.

The marker format uses the file's native comment syntax. Each marker
includes the comment's database ID (`#<id>`) on both the opening and
closing lines, enabling agents to reference specific comments by ID
(e.g. to call `resolve_comment(42)` via MCP) and enabling idempotent
apply/clear operations:

```rust
// <<<<<<< REVIEW #42
fn process(input: &str) -> Result<Output> {
    let parsed = parse(input);
}
// =======
// Does parse() handle empty input? If input is "", this
// will silently produce a default value.
// >>>>>>> REVIEW #42
```

Files remain syntactically valid after markers are applied. Language
detection is based on file extension, supporting `//`, `#`, `--`,
`<!-- -->`, `/* */`, `"`, and `;` comment styles.
