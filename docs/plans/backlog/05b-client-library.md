# Stage 5b: Client Library

## Goal

Build the client library that connects to a running crt server over the
Unix domain socket, sends JSON-RPC requests, and receives responses. Wire
up `crt <base>` to connect to a running server and call `init`.

## Why

The client library is the interface between all client-side code (TUI, MCP
adapter, CLI subcommands) and the server. A clean, type-safe client API
means callers never deal with raw JSON-RPC. This stage also validates the
server from Stage 5a by connecting to it for real.

## Requirements

### Client API

1. **`Client` struct**: connects to the server at `~/.crt/server.sock`.
   Provides typed methods for each server API call.

2. **Connection**: `Client::connect()` opens a connection to the socket.
   Returns an error if the socket doesn't exist or connection is refused
   (no server running).

3. **`init` method**: `client.init(worktree, base_ref)` sends the `init`
   request and returns the resolved context (repo root, head ref).

4. **Request/response**: internally, the client:
   - Assigns an auto-incrementing `id` to each request.
   - Serializes the request as JSON, writes it as a line to the socket.
   - Reads lines from the socket until it gets a response matching the
     `id`.
   - Deserializes and returns the result (or an error).

5. **Notification buffering**: if the server sends a notification (a
   JSON-RPC message without `id`) between request and response, the
   client buffers it. Callers can poll for buffered notifications.

6. **Error handling**: server errors (JSON-RPC error responses) are
   converted to Rust errors with context (method name, error message,
   error code).

7. **Stub methods**: typed method signatures for all planned API calls
   (e.g. `client.list_changed_files()`). These just call the generic
   request method with the right method name and params. They'll return
   "not implemented" errors from the server stubs for now.

### `crt <base>` Integration

8. **Connect to server**: `crt <base>` attempts to connect to a running
   server. If successful, it calls `init` and prints the resolved context
   (same info as the current output, but now coming from the server).

9. **No server available**: if no server is running (connection refused),
   print a message telling the user to run `crt server` first (or that
   no server is available). Embedded mode comes in Stage 5c.

10. **Remove direct git/db calls from `main.rs`**: the temporary
    scaffolding code that calls `git::Repo` directly from `main.rs`
    should be replaced with server calls via the client library.

## Acceptance Criteria

- [ ] `Client::connect()` connects to a running server.
- [ ] `Client::connect()` returns a clear error when no server is running.
- [ ] `client.init(worktree, base_ref)` sends the request and returns
      the resolved context.
- [ ] With `crt server` running in one terminal, `crt <base>` in another
      terminal connects and prints the resolved context from the server.
- [ ] Sending a stub method (e.g. `list_changed_files`) returns a typed
      error indicating "not implemented".
- [ ] Server errors are converted to Rust errors with context.
- [ ] The direct `git::Repo` calls are removed from `main.rs`.

## Open Questions

- Should the client be sync (blocking I/O) or async? The TUI event loop
  may benefit from async, but sync is simpler for now.
- Should the client auto-reconnect if the connection drops?
