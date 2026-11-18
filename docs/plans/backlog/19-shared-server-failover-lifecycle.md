# Stage 19: Shared Server Failover and Lifecycle

## Status: Backlog

## Goal

Ensure all UIs (TUI now, GUI later) share one server instance per user,
with automatic takeover when the active server dies.

Embedded mode remains process-coupled to the hosting UI process. Long-lived
server behavior is provided explicitly by `crt server`.

## Why

The architecture is intentionally server-based: UIs are thin clients and
the server is the single source of truth.

Current behavior is close but incomplete:

- Startup supports connect-or-start embedded server.
- Embedded server ownership is tied to the client that started it.
- Full runtime failover/re-election is documented but not fully enforced.

This stage makes runtime behavior match the intended model.

## Target Behavior

1. On startup, UI clients try to connect to `~/.crt/server.sock`.
2. If no server is reachable, one client becomes embedded server and binds
   the shared socket.
3. All other UI clients connect to that same server.
4. If the active server dies, connected clients detect loss and race to
   recover; exactly one becomes server, others reconnect.
5. If the hosting UI process exits/crashes, embedded server exits with it and
   the socket is cleaned up.
6. Persistent `crt server` mode remains supported and takes precedence when
   running.

## Scope

- In scope: UI-launched embedded server lifecycle and failover.
- In scope: TUI and future GUI using identical recovery semantics.
- Out of scope: dedicated daemon/background service mode beyond existing
  `crt server` command UX.

## Requirements

### 1) Shared Socket Ownership

- Keep one canonical socket path (`~/.crt/server.sock`).
- Server ownership is defined by successful `bind()`.
- Bind conflict (`EADDRINUSE`) is treated as "another server won" and
  triggers reconnect attempts.

### 2) Client Connection Supervisor

- Add a connection supervisor layer around JSON-RPC client calls.
- On broken pipe / EOF / connection reset:
  1. wait randomized jitter (50-200ms),
  2. try reconnect,
  3. if refused, attempt embedded startup,
  4. if bind fails, repeat from reconnect.
- After reconnect, re-run `init` with original `(worktree, base_ref)` and
  restore normal operation.

### 3) Process-Coupled Embedded Lifecycle

- Embedded server runs in the same process as the hosting UI client.
- No server-side active-client counting is required in embedded mode.
- If the hosting UI exits while other clients are connected, those clients
  detect loss and recover via reconnect + bind-race election.
- Long-lived server behavior is an explicit user choice via `crt server`.

### 4) Multi-Client Notification Continuity

- Notification broadcasting continues to work after failover.
- Reconnected clients re-subscribe transparently.
- Clients must tolerate duplicate/reordered notifications around failover
  windows by reloading canonical server state when needed.

### 5) Mode Semantics

- `crt <base>`: connect-or-start + runtime failover enabled.
- `crt mcp-server`: same connect-or-start + runtime failover behavior.
- `crt server`: explicit persistent mode; no auto-shutdown on client count.

### 6) Recovery UX and Retry Policy

- During disconnect/failover, interactive UIs show a transient status message
  (e.g. `Connection lost. Reconnecting...`).
- On successful recovery, show a short confirmation (e.g. `Reconnected`).
- Reconnect attempts are unbounded while the UI process is alive; no hard
  terminal failure is raised solely due to temporary server loss.
- Requests made while disconnected should fail fast with a transient,
  user-readable error and succeed again after reconnect.

## Implementation Notes

### Client Side

- Introduce a reconnecting RPC wrapper used by TUI/GUI/MCP clients.
- Preserve pending user actions safely across reconnect boundaries where
  possible; otherwise return clear transient errors and recover on next action.
- Centralize error classification for transport vs method errors.

### Server Side

- Ensure socket cleanup on all exit paths.

### CLI Orchestration

- Keep startup `connect_or_start` flow.
- Keep embedded lifecycle process-coupled (host exits -> embedded exits).

## Acceptance Criteria

- [ ] `crt <base>` starts without pre-running `crt server`.
- [ ] Two `crt <base>` sessions share one server and see live updates.
- [ ] Killing the active server causes clients to recover automatically.
- [ ] During failover, exactly one client wins `bind()` and others reconnect.
- [ ] Closing/killing the hosting UI stops embedded server.
- [ ] If peers are connected when host exits, one peer takes over and others
      reconnect.
- [ ] `crt server` remains stable and unaffected by embedded auto-shutdown.
- [ ] Multi-repo behavior remains intact under one shared server process.

## Test Plan

1. Start UI A (`crt <base>`) with no server running; verify embedded starts.
2. Start UI B (`crt <base>`); verify B connects to same socket/server.
3. Kill server process; verify A/B recover and continue.
4. Force simultaneous recovery from A and B; verify single bind winner.
5. Close/kill host A while B is connected; verify takeover by B (or another
   peer) and continued operation.
6. Close final UI; verify no embedded server remains.
7. Repeat with two repositories connected concurrently.

## Resolved Decisions

- Embedded lifecycle stays process-coupled; no client-count grace-period
  shutdown logic is needed in embedded mode.
- Failover surfaces transient UI status (`Reconnecting...` then
  `Reconnected`).
- Recovery retries are unbounded while the UI process remains active.
