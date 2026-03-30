# Stage 5a: Server Core + Persistent Mode

## Goal

Build the JSON-RPC server that listens on a Unix domain socket, handles
the `init` handshake, manages per-connection state, and dispatches
requests to method handlers. `crt server` starts the persistent server.

## Why

The server is the single source of truth for all state. Getting the
protocol, dispatch, and connection lifecycle right first means every
subsequent feature is just adding a new method handler. This stage is
the foundation — if we get this right, everything else plugs in cleanly.

## Requirements

### Protocol

1. **JSON-RPC 2.0 over newline-delimited JSON**: each message is a single
   line of JSON terminated by `\n`. The server reads lines from the
   socket, parses each as a JSON-RPC request, and writes a JSON-RPC
   response followed by `\n`.

2. **Request handling**:
   - Method calls (have `id`) → dispatch to handler, return response.
   - Notifications (no `id`) → dispatch to handler, no response.
   - Unknown methods → return JSON-RPC error `-32601 Method not found`.
   - Parse errors → return JSON-RPC error `-32700 Parse error`.

3. **Batch requests are not required for v1.** Single requests only.

### Server

4. **Unix domain socket**: listen on `~/.crt/server.sock`.

5. **Stale socket cleanup**: on startup, if the socket file exists, try
   to connect to it. If connection fails (no process listening), remove
   the stale file and proceed. If connection succeeds (another server is
   running), exit with an error.

6. **Multi-connection**: accept multiple simultaneous client connections.
   Each connection is handled independently (own task/thread). Each
   connection has its own state (set during `init`).

7. **Graceful shutdown**: on `SIGINT` or `SIGTERM`, stop accepting new
   connections, close existing connections, remove the socket file, and
   exit cleanly.

8. **Logging**: log to stderr. Connection events (connect, init,
   disconnect), errors, and shutdown.

### `init` Handler

9. **`init` method**: the first request on a new connection must be
   `init`. Parameters:
   - `worktree` (string): path to the client's working directory.
   - `base_ref` (string): the base ref for the review scope.

10. **Server resolves** (using the git module):
    - Repo root (via `commondir`).
    - Head ref (branch at HEAD of the worktree).
    - DB path (`<repo_root>/.crt/reviews.db`).
    - Opens (or reuses) the database.

11. **Response**: returns `{ repo_root, head_ref, worktree }`.

12. **Scope**: after `init`, all subsequent requests on this connection
    are scoped to `(base_ref, head_ref)` and operate against the resolved
    worktree and repo.

13. **Pre-init requests**: any request other than `init` before the
    connection is initialized returns an error.

### API Stubs

14. **Method dispatch table**: a central dispatch that maps method names
    to handler functions. For this stage, only `init` has a real
    implementation. All other planned methods should be registered as
    stubs returning a `-32001 Not implemented` error with the method
    name. The planned methods (from OVERVIEW):
    - `list_changed_files`
    - `get_file_diff`
    - `get_file_content`
    - `mark_reviewed`
    - `unmark_reviewed`
    - `reset_reviews`
    - `create_comment`
    - `list_comments`
    - `get_comment`
    - `update_comment`
    - `resolve_comment`
    - `unresolve_comment`
    - `delete_comment`
    - `apply_comments`
    - `clear_comments`
    - `search_codebase`
    - `find_definition`
    - `track_repo`
    - `list_repos`

### `crt server` Subcommand

15. **`crt server`**: starts the server in the foreground. Prints the
    socket path to stderr on startup. Runs until signalled.

## Acceptance Criteria

- [ ] `crt server` starts and listens on `~/.crt/server.sock`.
- [ ] Connecting to the socket and sending a valid `init` JSON-RPC
      request returns the resolved context (testable with a script or
      manual netcat).
- [ ] Sending a request before `init` returns an error.
- [ ] Sending an unknown method returns `-32601 Method not found`.
- [ ] Sending a known stub method returns `-32001 Not implemented`.
- [ ] Multiple clients can connect simultaneously with different scopes.
- [ ] `SIGINT` shuts down cleanly and removes the socket file.
- [ ] A stale socket file from a previous crash is cleaned up on startup.
- [ ] An error is shown if another server is already running.
- [ ] Connection/disconnection events are logged to stderr.

## Open Questions

- Should the server have a `--socket` flag to override the default path?
- Should `init` fail if the `base_ref` doesn't resolve, or should that
  be deferred to when the ref is actually used?
