# Stage 5c: Embedded Server Mode

## Goal

When no persistent server is running, `crt <base>` starts an in-process
server and connects to it automatically. The user never needs to manually
run `crt server` for the basic workflow.

## Why

The persistent server is useful for long-running sessions and multi-client
scenarios (TUI + MCP agent simultaneously). But the common case is a
single user running `crt <base>` — they shouldn't need to start a
separate process first. Embedded mode makes the tool zero-config for the
simple case while preserving the full server architecture.

## Requirements

1. **In-process server**: when `crt <base>` fails to connect to
   `~/.crt/server.sock` (no persistent server running), it starts the
   server in-process as a background tokio task.

2. **In-process transport**: the embedded server and client communicate
   via an in-process channel (e.g. `tokio::sync::mpsc` or similar)
   rather than a Unix socket. This avoids creating a socket file that
   could conflict with a later `crt server` invocation.

3. **Same API**: the client library must work identically whether
   connected to a persistent server over a socket or an embedded server
   over a channel. The client API should abstract over the transport.

4. **Lifecycle**: the embedded server starts before the main logic
   (TUI, CLI command) and shuts down after it exits. No cleanup needed
   since there's no socket file.

5. **`crt <base>` flow**:
   - Try to connect to `~/.crt/server.sock`.
   - If connected → use the persistent server.
   - If connection refused → start embedded server, connect to it.
   - Call `init`, proceed with the rest of the command.

6. **Other subcommands**: `crt apply-comments <base>` and
   `crt clear-comments <base>` should also use the embedded server
   fallback (same connection logic as `crt <base>`).

## Acceptance Criteria

- [ ] `crt <base>` works without a running `crt server` (starts embedded).
- [ ] `crt <base>` prefers a running persistent server when available.
- [ ] The embedded server does not create a socket file.
- [ ] The client API is identical for both persistent and embedded
      connections (same method signatures, same error types).
- [ ] The embedded server shuts down when the command exits.
- [ ] `crt apply-comments <base>` and `crt clear-comments <base>` also
      work via embedded mode.

## Open Questions

- Should there be a `--no-server` flag to force embedded mode even when
  a persistent server is running?
- Should the embedded server print any indication that it's running
  (e.g. a log line to stderr), or be completely silent?
