# Stage 25: Embedded Lifecycle Alignment

## Status: Completed

## Order

6 of 7 (recommended implementation order)

## Depends On

- Stage 19 (`../completed/19-shared-server-failover-lifecycle.md`)
- Stage 20 (`20-thin-client-api-contract.md`)
- Stage 24 (`24-client-reconnect-failover-supervisor.md`)

## Goal

Align implementation details with the agreed embedded lifecycle model:
embedded server is process-coupled to host UI, while persistent mode is
explicitly provided by `crt server`.

## Why

Some current orchestration logic still reflects starter-client ownership and
manual shutdown assumptions. This stage removes ambiguity and enforces one
clear lifecycle model.

## Scope

- In scope: startup/shutdown orchestration and cleanup behavior.
- In scope: consistent semantics across TUI and MCP startup flows.
- Out of scope: changing persistent server responsibilities.

## Requirements

1. Keep a single shared `connect_or_start` startup behavior as the primary
   flow for review and MCP clients.

2. Ensure embedded lifecycle is process-coupled:
   - host process exits -> embedded server exits,
   - socket cleanup is best-effort and reliable.

3. Remove lifecycle assumptions that conflict with failover model
   (e.g. logic implying starter UI should preserve embedded lifetime after
   process end).

4. Keep persistent `crt server` behavior unchanged:
   - no dependency on client connection count,
   - explicit start/stop by user.

5. Ensure `crt mcp-server` follows same connect-or-start semantics and does
   not introduce alternate lifecycle behavior.

6. Keep lifecycle/status signaling UI-agnostic:
   - lifecycle/reconnect state is surfaced through core interaction
     effects/events,
   - no TUI-specific lifecycle coupling is introduced.

## Implementation Notes

- Keep behavior simple and explicit; no hidden background-helper mode.
- Prefer documenting lifecycle guarantees near startup functions.
- Validate stale socket cleanup paths after abrupt exits.

## Deliverables

- Updated lifecycle orchestration code and comments.
- Consistent startup behavior across entry modes (`crt`, `crt mcp-server`).
- Updated docs reflecting final lifecycle semantics.

## Acceptance Criteria

- [x] Embedded host exit always terminates embedded server.
- [x] Peers recover via Stage 24 failover behavior.
- [x] `crt server` remains explicit and unaffected.
- [x] Socket cleanup behavior is consistent across normal and abrupt exits.
- [x] Lifecycle/reconnect status remains consumable by both TUI and future GUI
      via core effects/events.

## Resolved Decisions

- Lifecycle model remains process-coupled for embedded mode.
- Persistent/background behavior is explicit via `crt server` only.

## Completed Notes

- Kept lifecycle ownership in the shared reconnecting `Client` path introduced
  by Stage 24, and documented that both TUI and MCP startup use that same
  connect-or-start behavior.
- Wired `crt mcp-server` through the same socket path, connect-or-start, and
  shutdown lifecycle as review mode. It can start unscoped, discover active
  review sessions, and select one before running scoped tools.
- Implemented the MCP stdio adapter for currently server-backed tools:
  `list_review_sessions`, `select_review_session`, `list_changed_files`,
  `get_file_diff`, `search_codebase`, `find_definition`, and
  `list_review_summary`.
- Left comment-oriented MCP tools for the existing Stage 14 backlog because
  the corresponding server comment methods still return not implemented.
- Documented embedded lifecycle semantics in `docs/OVERVIEW.md`: embedded
  servers are process-coupled, `crt server` is the only persistent mode, and
  standalone mode uses a private temp socket.
- Added tests for MCP tool schemas and MCP CLI base resolution.
