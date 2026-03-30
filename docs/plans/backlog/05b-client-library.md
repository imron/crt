# Stage 5b: Client Library

## Goal

Build the async client library that connects to a running crt server,
sends JSON-RPC requests, and receives responses. Wire up `crt <base>` to
connect to a running server and call `init`.

## Why

The client library is the interface between all client-side code (TUI,
MCP adapter, CLI subcommands) and the server. A clean, typed async API
means callers never deal with raw JSON-RPC.

## Design Decisions

### Async Client

The client is fully async (tokio). For the TUI, it runs on a background
thread with a tokio runtime, communicating with the main (crossterm)
thread via channels. For non-TUI clients (MCP adapter, CLI), the async
client runs directly in the tokio runtime.

### Auto-Reconnect

The client auto-reconnects if the connection drops. On reconnection:
1. Wait a brief random jitter (50-200ms).
2. Try to connect to the socket (another client may have become server).
3. If connected → re-send `init`, resume.
4. If refused → try to bind as server (become embedded), connect.
5. If bind fails (`EADDRINUSE`) → wait jitter, retry from step 2.

This handles the case where a persistent server is killed while clients
are connected.

## Requirements

### Client API

1. **`Client` struct**: async methods for each server API call. Hides
   JSON-RPC details.

2. **`Client::connect(socket_path)`**: connect to a running server.
   Returns error if socket doesn't exist or connection refused.

3. **`client.init(worktree, base_ref)`**: sends `init`, returns resolved
   context.

4. **Request/response**: auto-incrementing `id`, serialize request,
   write line, read response line matching `id`, deserialize, return.

5. **Notification buffering**: server-pushed notifications buffered.
   Callers can poll or subscribe.

6. **Error handling**: JSON-RPC errors converted to typed Rust errors.

7. **Stub method signatures**: typed methods for all planned API calls.
   They return "not implemented" from server stubs for now.

### `crt <base>` Integration

8. **Connect to server**: `crt <base>` attempts to connect. On success,
   calls `init`, prints resolved context.

9. **No server available**: prints message suggesting `crt server` or
   that embedded mode will start (Stage 5c). For now, just an error.

10. **Remove direct git calls from `main.rs`**: the temporary
    scaffolding is replaced with server calls via the client.

## Acceptance Criteria

- [ ] `Client::connect()` connects to a running server.
- [ ] `Client::connect()` returns clear error when no server running.
- [ ] `client.init()` returns resolved context.
- [ ] `crt server` + `crt <base>` in separate terminals: client connects,
      prints context from server.
- [ ] Stub methods return typed "not implemented" errors.
- [ ] Direct git calls removed from `main.rs`.

## Open Questions

- Should notification subscriptions use a channel or a callback?
