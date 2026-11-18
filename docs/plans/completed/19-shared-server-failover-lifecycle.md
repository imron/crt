# Stage 19: Shared Server Failover and Lifecycle

## Status: Completed / Superseded By Stages 24-26

## Goal

Ensure all UIs (TUI now, GUI later) share one server instance per user,
with automatic takeover when the active server dies.

Embedded mode remains process-coupled to the hosting UI process. Long-lived
server behavior is provided explicitly by `crt server`.

## Why

The architecture is intentionally server-based: UIs are thin clients and
the server is the single source of truth.

This stage captured the architecture target for shared server ownership,
runtime failover, and process-coupled embedded lifecycle. Implementation was
completed through the more specific follow-on plans:

- Stage 24: client reconnect and bind-race failover supervisor.
- Stage 25: embedded lifecycle alignment and MCP startup alignment.
- Stage 26: remaining hardening and integration-test coverage.

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
- In scope: TUI, MCP, and future GUI using identical recovery semantics.
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
- `crt mcp-server`: connect to the existing shared server and select an
  active review session.
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

### App Startup Orchestration

- Keep the app-owned startup `connect_or_start` flow.
- Keep embedded lifecycle process-coupled (host exits -> embedded exits).

## Acceptance Criteria

- [x] `crt <base>` starts without pre-running `crt server`.
- [x] Runtime reconnect/failover is implemented in the shared client.
- [x] During failover, one client wins `bind()` and others reconnect.
- [x] Closing/killing the hosting UI stops its embedded server.
- [x] If peers are connected when host exits, peers recover through reconnect
      and bind-race failover.
- [x] `crt server` remains explicit persistent mode and is unaffected by
      embedded auto-shutdown.
- [x] `crt mcp-server` uses the shared server socket and does not own review
      state directly.
- [x] Multi-repo/session discovery is supported through `list_repos` /
      `list_review_sessions`.
- [ ] Full multi-client, multi-repo, peer-takeover integration coverage is
      handled by Stage 26.

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

## Completed Notes

- Stage 24 added shared reconnect supervision in `src/client.rs`, including
  transport-loss classification, reconnect attempts, embedded startup,
  bind-race behavior, re-init, snapshot reload hooks, and connection-state
  events.
- Stage 25 aligned embedded lifecycle so `crt` and future interactive clients
  share connect-or-start behavior, while MCP remains an adapter to the shared
  server.
- Persistent `crt server` remains an explicit user-started server mode.
- Active review sessions are tracked server-side and exposed through
  `list_repos`, allowing MCP to start unscoped and select from connected
  review sessions.
- Remaining work is test depth, not architecture definition. That belongs in
  Stage 26.
