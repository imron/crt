# Stage 5c: Embedded Server Mode

## Goal

When no persistent server is running, `crt <base>` starts a real server
in-process that binds to the socket, then connects to it. The user never
needs to manually run `crt server` for the basic workflow.

## Why

The persistent server is useful for long-running sessions and multi-client
scenarios. But the common case is a single user running `crt <base>` —
they shouldn't need to start a separate process. Embedded mode makes the
tool zero-config for the simple case.

## Design Decisions

### Embedded Server Uses the Real Socket

The embedded server binds to `~/.crt/server.sock`, just like persistent.
This means:
- Other `crt` sessions or MCP adapters can connect to it.
- Only one server exists at a time (socket enforces this).
- When the embedded client exits, the server shuts down and cleans up.

### `--standalone` for True Isolation

`--standalone` forces in-process channels instead of a socket. The
embedded server never touches the filesystem. No other clients can
connect. Useful for testing or when you don't want to interfere with
a running server.

### Race Handling

If the server dies and N clients detect lost connections simultaneously:
1. Wait a random jitter (50-200ms).
2. Try to connect to the socket (maybe another client recovered).
3. If connected → resume as client.
4. If connection refused → try to bind as server.
5. If bind succeeds → new server.
6. If bind fails (`EADDRINUSE`) → wait jitter, retry from step 2.
The OS `bind()` is atomic, so exactly one client wins.

## Requirements

1. **Embedded server startup**: when `crt <base>` fails to connect to
   `~/.crt/server.sock`, start the server in-process (background tokio
   task), bind to the socket, then connect to it as a client.

2. **Socket lifecycle**: the embedded server binds the socket on start and
   removes it on shutdown. Shutdown happens when the main client
   disconnects (or the process exits).

3. **Other clients welcome**: while the embedded server is running, other
   clients can connect to the socket normally.

4. **`--standalone` flag**: uses in-process channels instead of the
   socket. The embedded server does not bind to any path. No other
   clients can connect. Completely silent (no logging to stderr).

5. **`crt <base>` flow**:
   - Try to connect to `~/.crt/server.sock`.
   - If connected → use persistent server (pure client).
   - If refused → start embedded server on socket, connect.
   - Call `init`, proceed.

6. **Auto-reconnect with fallback**: if the connection drops during a
   session (persistent server killed), the client:
   - Waits random jitter.
   - Tries to reconnect.
   - If refused → starts embedded server, binds socket, connects.

7. **Other subcommands**: `apply-comments` and `clear-comments` also use
   the embedded fallback.

8. **Embedded server is silent**: no logging to stderr unless there's an
   error. The user shouldn't know it's there.

## Acceptance Criteria

- [ ] `crt <base>` works without a running `crt server`.
- [ ] `crt <base>` prefers a running persistent server when available.
- [ ] The embedded server binds to `~/.crt/server.sock`.
- [ ] A second `crt <base>` session connects to the first's embedded
      server (shared state).
- [ ] `crt server` while embedded is running → clear error.
- [ ] When the first `crt <base>` exits, socket is cleaned up.
- [ ] `--standalone` does not create a socket file.
- [ ] `--standalone` works correctly (init, all operations).
- [ ] Auto-reconnect works when the server is killed.
- [ ] Embedded server produces no stderr output.

## Open Questions

- Should the embedded server have an idle timeout? e.g. if the main
  client disconnects but other clients are still connected, keep running
  for N seconds then shut down?
- When auto-reconnect starts an embedded server, should the TUI show
  any indication?
