# Stage 5a: Server Core + Persistent Mode

## Goal

Build the JSON-RPC server that listens on a Unix domain socket, handles
the `init` handshake, manages per-connection state, and dispatches
requests to method handlers. `crt server` starts the persistent server.

## Why

The server is the single source of truth for all state. Getting the
protocol, dispatch, and connection lifecycle right first means every
subsequent feature is just adding a new method handler.

## Design Decisions

### Threads vs Tasks

The server uses **tokio tasks** (async). Git2 and rusqlite are synchronous
libraries, so handler code that touches git or the database runs inside
`spawn_blocking` to avoid blocking the async runtime.

### Socket Ownership

Whoever holds `~/.crt/server.sock` is the server. This applies to both
persistent (`crt server`) and embedded servers. Only one server can exist
at a time. The OS enforces this via `bind()` semantics.

### init Validation

`init` fails immediately if the `base_ref` doesn't resolve to a valid
commit. This catches typos early.

## Requirements

### Protocol

1. **JSON-RPC 2.0 over newline-delimited JSON**: each message is a single
   line of JSON terminated by `\n`. The server reads lines, parses as
   JSON-RPC, dispatches, writes response line.

2. **Request handling**:
   - Method calls (have `id`) → dispatch, return response.
   - Notifications (no `id`) → dispatch, no response.
   - Unknown methods → `-32601 Method not found`.
   - Parse errors → `-32700 Parse error`.
   - Batch requests not required for v1.

### Server

3. **Unix domain socket**: listen on `~/.crt/server.sock`.

4. **Stale socket cleanup**: on startup, if socket exists, try to connect.
   If connection fails (no process listening), remove stale file. If
   connection succeeds (another server running), exit with error.

5. **Multi-connection**: accept multiple simultaneous connections. Each
   handled by its own tokio task. Each has its own state (set during
   `init`).

6. **Graceful shutdown**: `SIGINT`/`SIGTERM` → stop accepting, close
   connections, remove socket file, exit.

7. **Logging**: connection events and errors to stderr.

### `init` Handler

8. **Parameters**: `worktree` (string), `base_ref` (string).

9. **Server resolves** (using git module via `spawn_blocking`):
   - Repo root via `commondir`.
   - Head ref (branch at HEAD of worktree).
   - Validates base_ref resolves to a commit.
   - Opens/reuses database.

10. **Response**: `{ repo_root, worktree, head_ref, base_ref }`.

11. **Pre-init requests**: any request other than `init` before
    initialization returns `-32000 Not initialized`.

### API Stubs

12. **Method dispatch**: all planned methods registered as stubs
    returning `-32001 Not implemented`:
    - `list_changed_files`, `get_file_diff`, `get_file_content`
    - `mark_reviewed`, `unmark_reviewed`, `reset_reviews`
    - `create_comment`, `list_comments`, `get_comment`
    - `update_comment`, `resolve_comment`, `unresolve_comment`,
      `delete_comment`
    - `apply_comments`, `clear_comments`
    - `search_codebase`, `find_definition`
    - `track_repo`, `list_repos`

### `crt server` Subcommand

13. **`crt server`**: starts persistent server in foreground. Prints
    socket path to stderr. Runs until signalled.

### Project Structure

14. **`lib.rs`**: `main.rs` becomes a thin wrapper calling `crt::run()`.
    All logic lives in `lib.rs` and submodules. This enables integration
    testing.

## Acceptance Criteria

- [ ] `crt server` starts and listens on `~/.crt/server.sock`.
- [ ] A valid `init` request returns the resolved context.
- [ ] `init` with a bad `base_ref` returns an error.
- [ ] Sending a request before `init` returns `-32000`.
- [ ] Unknown method returns `-32601`.
- [ ] Known stub method returns `-32001`.
- [ ] Multiple clients can connect simultaneously.
- [ ] `SIGINT` shuts down cleanly, removes socket file.
- [ ] Stale socket is cleaned up on startup.
- [ ] Error if another server is already running.
- [ ] Events logged to stderr.
- [ ] `main.rs` is a thin wrapper; logic is in `lib.rs`.

## Open Questions

- Should the server log to a file in addition to stderr?
