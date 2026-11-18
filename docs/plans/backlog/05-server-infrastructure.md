# Stage 5: Server Infrastructure

## Goal

Build the core server: Unix domain socket listener, JSON-RPC protocol
handling, connection management with `init` handshake, and the embedded
server mode. Build the client library that the TUI and other clients use
to communicate with the server.

## Why

The server is the backbone of the architecture. All state management, git
operations, and review logic flow through it. The TUI, MCP adapter, and
CLI subcommands are all clients. Getting the server infrastructure right
early means subsequent stages can focus on implementing API methods and
UI without worrying about transport, protocol, or connection lifecycle.

The embedded server mode is equally important — it ensures `crt <base>`
works out of the box without requiring the user to manually start a server.

## Requirements

### Server

1. **Unix domain socket**: the server listens on `~/.crt/server.sock`.
   If the socket file already exists (stale from a previous crash),
   detect this and clean it up.

2. **JSON-RPC 2.0**: the server speaks JSON-RPC 2.0 over the socket.
   Each connection is a newline-delimited JSON stream. The server must
   handle:
   - Method calls (request with `id` → response with `id`).
   - Notifications (request without `id` → no response).
   - Batch requests (array of requests).
   - Error responses for unknown methods, invalid params, and internal
     errors.

3. **Connection lifecycle**: each client connection goes through:
   - Connect to socket.
   - Send `init` request with `worktree_path` and `base_ref`.
   - Server resolves: repo root (via git `commondir`), head_ref (branch
     at HEAD of worktree), DB path. Opens (or reuses) the repo's DB.
   - Server responds with the resolved context.
   - All subsequent requests on this connection are scoped to the
     resolved `(repo, worktree, base_ref, head_ref)`.
   - Disconnect cleans up connection state.

4. **Multi-connection**: the server handles multiple simultaneous
   connections. Each connection has its own scope (repo, worktree,
   base_ref, head_ref). Connections to the same repo share the same
   DB handle.

5. **Notifications (server → client)**: the server can push notifications
   to connected clients. When state changes on one connection (e.g. a
   comment is resolved), all other connections to the same
   `(repo, base_ref, head_ref)` scope receive a notification. This
   enables live updates in the TUI when an agent acts via MCP.

6. **Graceful shutdown**: `SIGTERM` or `SIGINT` causes the server to
   stop accepting new connections, finish in-flight requests, close all
   connections, and clean up the socket file.

7. **`crt server` subcommand**: starts the persistent server in the
   foreground. Logs to stderr. Exits on signal.

### Client Library

8. **Client module** (`client.rs`): provides a synchronous (or async)
   API for connecting to the server, sending requests, and receiving
   responses. The TUI, MCP adapter, and CLI subcommands all use this.

9. **Connection**: connect to `~/.crt/server.sock`. If connection fails
   (no server running), return an error that the caller can handle
   (e.g. by starting an embedded server).

10. **Request/response**: send a JSON-RPC request and wait for the
    matching response (by `id`). Handle notifications received between
    request and response (buffer them for the caller to process).

11. **Notification handling**: the client must be able to receive
    server-pushed notifications asynchronously (or poll for them).

### Embedded Server

12. **Embedded mode**: when `crt <base>` detects no running server, it
    starts the server in-process (same binary, spawned as a background
    task or thread). The TUI then connects to it via the same client
    library (possibly using an in-process channel instead of a socket
    for efficiency, but the API should be identical).

13. **Lifecycle**: the embedded server starts before the TUI event loop
    and shuts down after the TUI exits. It does not listen on the Unix
    socket (to avoid conflicting with a persistent server started later).

### API Stubs

14. **Method dispatch**: the server should have a dispatch table mapping
    method names to handler functions. For this stage, only `init` needs
    a real implementation. All other methods (listed in the OVERVIEW)
    should be registered as stubs that return "not implemented" errors.
    Subsequent stages fill in the implementations.

## Acceptance Criteria

- [ ] `crt server` starts and listens on `~/.crt/server.sock`.
- [ ] A client can connect, send an `init` request, and receive the
      resolved context (repo root, head_ref).
- [ ] Multiple clients can connect simultaneously.
- [ ] Sending an unknown method returns a JSON-RPC error response.
- [ ] `SIGINT` causes the server to shut down cleanly and remove the
      socket file.
- [ ] A stale socket file from a previous crash is cleaned up on startup.
- [ ] `crt <base>` starts an embedded server when no persistent server
      is running, and the TUI can communicate with it.
- [ ] `crt <base>` connects to a running persistent server when one is
      available.
- [ ] Server-to-client notifications can be sent and received.
- [ ] The client library provides a clean API that hides JSON-RPC details
      from callers.

## Open Questions

- Should the embedded server use an in-process channel (e.g. `tokio::sync`)
  instead of a real socket? This avoids filesystem overhead but means the
  client API needs to abstract over both transports.
- Should the server support TCP in addition to Unix sockets, for remote
  access or cross-platform support?
- How should the server handle a client that disconnects without sending
  a proper close? Timeout? Immediate cleanup?
- Should the server persist any state of its own (e.g. tracked repos list),
  or derive everything from client connections?
- What should happen if `crt <base>` starts an embedded server and then
  the user separately starts `crt server`? Should the persistent server
  detect that the socket is in use and fail, or should the embedded server
  yield?
