# Stage 31: Server Version Compatibility and Role Handoff

## Status

Backlog.

## Goal

Make shared server ownership explicit when `crt` API versions or client roles
are present.

Clients that connect to an incompatible server API version should fail with a
clear error so the user can choose whether to upgrade, downgrade, stop the
existing server, or restart their sessions.

MCP should be able to start a low-priority fallback server when no other
server exists, but must relinquish server ownership when a same-version
interactive `crt` instance starts.

## Why

The current lifecycle model assumes one shared server elected by socket bind
ownership. That works for failover, but it does not encode two important
cases:

1. A newly started binary may speak an incompatible server API from the
   currently running server.
2. MCP can be useful even before a TUI exists, but an MCP-owned fallback server
   should not become the long-lived authority once a real interactive `crt`
   session starts.

We need explicit server metadata and handoff semantics so compatibility errors
and ownership decisions are deterministic, testable, and not hidden in TUI or
MCP adapters.

## Scope

- In scope: server API version metadata.
- In scope: server role metadata and priority decisions.
- In scope: explicit same-version handoff/shutdown RPCs.
- In scope: client reconnect behavior after handoff.
- In scope: MCP low-priority fallback startup.
- In scope: tests for version compatibility and role takeover behavior.
- Out of scope: durable daemon/service management.
- Out of scope: preserving old pre-feature servers that do not implement the
  metadata RPC. They can receive a clear incompatible-server error.

## Design Principles

- Server remains the source of truth for shared mutable state.
- UI and MCP layers stay thin:
  - they declare their client kind,
  - they declare whether they are allowed to start a fallback server,
  - they react to reconnect/status events.
- The shared client/supervisor owns reconnect and handoff orchestration.
- Protocol behavior should use typed enums in Rust. Database columns, if ever
  needed, may remain strings mapped through enum conversions.
- Socket bind still decides final ownership after a handoff window. Metadata
  decides whether a same-version role handoff should be requested.

## Terminology

### API Version

Add a single integer server API version:

```rust
pub const SERVER_API_VERSION: u32 = 1;
```

This is an incompatible protocol version, not a package version. Compatible
additions do not need to increment it. Breaking wire/protocol changes do. A
client may only operate against a server with the exact same API version.

### Client Kind

Add a protocol enum:

```rust
pub enum ClientKind {
    Interactive,
    Mcp,
    Cli,
}
```

The TUI and future GUI use `Interactive`. MCP uses `Mcp`. Non-interactive CLI
commands use `Cli`.

### Server Role

Add a protocol enum:

```rust
pub enum ServerRole {
    Primary,
    LowPriorityMcp,
    Persistent,
}
```

Role meaning:

- `Primary`: normal embedded server started by an interactive `crt` client.
- `LowPriorityMcp`: fallback server started by MCP only because no other
  server was reachable.
- `Persistent`: explicit `crt server` process.

Role priority for the same API version:

1. `Persistent`
2. `Primary`
3. `LowPriorityMcp`

API version compatibility is checked before role priority. If the server API
version differs from the client API version, startup fails with a clear error
and no handoff is attempted.

Role priority only applies when versions match. Same-version interactive
clients do not replace an existing `Primary` or `Persistent` server.

## Protocol Additions

### `server_info`

Add a JSON-RPC method:

```rust
server_info() -> ServerInfo
```

Where:

```rust
pub struct ServerInfo {
    pub api_version: u32,
    pub role: ServerRole,
    pub instance_id: String,
    pub active_interactive_clients: usize,
    pub active_mcp_clients: usize,
}
```

`instance_id` is generated at server startup. It is used only for diagnostics
and tests; socket ownership remains authoritative.

### `request_server_handoff`

Add a JSON-RPC method:

```rust
request_server_handoff(params: ServerHandoffRequest)
    -> ServerHandoffResult
```

```rust
pub struct ServerHandoffRequest {
    pub requester_api_version: u32,
    pub requester_client_kind: ClientKind,
    pub requester_will_start_role: ServerRole,
    pub reason: ServerHandoffReason,
}

pub enum ServerHandoffReason {
    InteractiveReplacingLowPriorityMcp,
}

pub enum ServerHandoffResult {
    Accepted { shutdown_after_ms: u64 },
    Rejected { reason: ServerHandoffRejectReason },
}
```

The server validates the request against its current role, API version, and
active client roles. It rejects requests from incompatible API versions and
never trusts the caller to decide final authority.

### Shutdown Notification

Before shutting down for handoff, the server broadcasts:

```rust
ServerNotification::ServerHandoff {
    reason: ServerHandoffReason,
}
```

Clients treat this like a controlled reconnect event. They should stop issuing
new requests, allow in-flight requests to fail transiently, and reconnect via
the existing supervisor loop.

## Startup Behavior

### Interactive `crt`

1. Try to connect to the default socket.
2. If no server is reachable, start an embedded `Primary` server.
3. If a server is reachable, call `server_info`.
4. If the server API version differs from this binary:
   - print a clear incompatible-server error,
   - do not request handoff,
   - do not attempt socket takeover,
   - let the user decide whether to upgrade, downgrade, stop the existing
     server, or restart their sessions.
5. If the server role is `LowPriorityMcp` with the same API version:
   - request handoff with `InteractiveReplacingLowPriorityMcp`,
   - wait for socket release,
   - attempt to start a `Primary` server,
   - reconnect through the normal supervisor path.
6. Otherwise, connect as a client and run `init`.

If another process wins the bind race after handoff, reconnect to the winner
and verify `server_info` again.

### `crt server`

1. Starts as `Persistent`.
2. If another same-version `Persistent` or `Primary` server exists, keep the
   current bind-conflict behavior.
3. If an incompatible-version server exists, print a clear error and exit.
4. If a same-version `LowPriorityMcp` server exists, request handoff and then
   bind.

### MCP

1. Try to connect to the default socket.
2. If a server is reachable, use it and do not attempt to become server.
3. If no server is reachable, MCP may start a `LowPriorityMcp` server.
4. If that server later receives an interactive handoff request, it accepts and
   shuts down.
5. MCP reconnects through the shared client supervisor and re-selects its
   active session using Stage 30 recovery behavior.

MCP must not request handoff from an existing `Primary` or `Persistent` server.

## Server-Side Behavior

The server tracks active client roles per connection:

- role is provided during `init` or a small pre-init registration request;
- disconnect removes that role from active counts;
- uninitialized connections do not count as interactive sessions.

Handoff acceptance rules:

- accept if current role is `LowPriorityMcp` and requester will start
  `Primary` or `Persistent`;
- reject same-version `Primary` replacing `Primary`;
- reject MCP replacing any reachable server;
- reject any request from an incompatible API version.

When handoff is accepted:

1. broadcast `ServerHandoff`;
2. stop accepting new requests except idempotent diagnostics;
3. cancel the listener after `shutdown_after_ms`;
4. close client connections;
5. remove the socket best-effort.

## Client Supervisor Behavior

The shared client should learn a new startup mode:

```rust
pub enum StartupRole {
    PrimaryInteractive,
    LowPriorityMcp,
    Persistent,
}
```

`connect_or_start` should:

- discover `server_info` before `init`;
- reject incompatible API versions with a user-facing error;
- decide whether to request handoff;
- perform handoff wait/reconnect/bind race;
- preserve existing reconnect behavior after transport loss.

The decision logic should be factored into a pure helper so compatibility and
role cases can be unit tested without sockets.

## Compatibility Behavior

If `server_info` is missing because the existing server predates this feature,
the new client cannot prove API compatibility.

Initial behavior should be conservative:

- return a clear error explaining that an incompatible or too-old server is
  running;
- tell the user to stop the server, upgrade/downgrade one side, or restart
  existing `crt` sessions.

Once this feature ships, future same-version role handoffs are handled
automatically. Incompatible version changes still require user choice.

## Interaction With Stage 30

Stage 30 remains useful and should either be implemented first or alongside
this stage. MCP handoff creates exactly the stale-session scenario Stage 30
addresses:

- MCP fallback server shuts down;
- MCP reconnects to the new interactive server;
- MCP re-selects the previous active session if it still exists.

## Tests

### Pure Decision Tests

- Incompatible-version interactive client should return a compatibility error.
- Incompatible-version persistent server should return a compatibility error.
- Same-version interactive client should not replace `Primary`.
- Same-version interactive client should replace `LowPriorityMcp`.
- MCP should not replace `Primary`.
- MCP should not replace `Persistent`.
- Lower-version and higher-version clients should both fail against a
  different server API version.

### Server RPC Tests

- `server_info` returns API version, role, instance id, and client counts.
- `request_server_handoff` accepts interactive-over-MCP requests.
- `request_server_handoff` rejects same-version primary-over-primary.
- `request_server_handoff` rejects incompatible-version requests.
- Accepted handoff broadcasts a notification before shutdown.

### Supervisor Integration Tests

- Interactive startup replaces a same-version MCP fallback server.
- Interactive startup fails clearly against an incompatible server.
- Two same-version interactive clients racing after MCP handoff produce one
  server.
- Clients connected to the old server reconnect to the winner.
- MCP fallback starts only when no server is reachable.
- MCP reconnects after relinquishing fallback server ownership.

### Compatibility Tests

- Missing `server_info` returns an actionable incompatible-server error.
- API-version mismatch returns an actionable incompatible-server error.
- Existing normal same-version startup behavior remains unchanged.

## Acceptance Criteria

- [ ] Server exposes typed API version and role metadata.
- [ ] Startup clients check server metadata before `init`.
- [ ] Incompatible server API versions fail with a clear user-facing error.
- [ ] MCP can start a low-priority fallback server when no server exists.
- [ ] Interactive `crt` replaces a low-priority MCP fallback server.
- [ ] MCP reconnects after relinquishing server status.
- [ ] Same-version `Primary` and `Persistent` ownership stays stable.
- [ ] Handoff behavior is covered by pure decision tests and integration
      tests.
- [ ] Older servers without `server_info` produce a clear error instead of
      undefined socket takeover behavior.

## Open Questions

- Should MCP fallback startup be enabled by default, or gated behind an option
  for MCP hosts that should never start background work?
- Should low-priority MCP fallback have an idle timeout when no MCP tools are
  active?
