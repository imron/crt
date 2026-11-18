# Stage 27: Server-Owned Git Access Assessment

## Status: Backlog

## Goal

Assess whether git and worktree access should move fully behind the server API,
or whether the app should keep some local repo reads for presentation-only
workflows.

## Why

The current architecture keeps shared mutable review state on the server, while
the app still reads local git/worktree data for presentation caches such as
full-file content, blame, and refreshed diffs. That split is pragmatic for a
local TUI, but it may become a boundary problem for MCP, HTTP, GUI, remote, or
sandboxed clients.

Before moving code, we need a clear viability assessment. Moving all git access
server-side improves consistency and keeps clients thinner, but it also adds API
surface and can make interactive presentation changes more expensive.

## Scope

- In scope: inventory app-side git/worktree access.
- In scope: identify which reads are canonical review data versus local
  presentation caches.
- In scope: evaluate server API additions for file content, blame, and
  configurable diff reloads.
- In scope: assess performance, round-trip, caching, reconnect, and multi-client
  implications.
- Out of scope: implementing a full migration of app-side git reads.
- Out of scope: changing review/comment mutation ownership, which already
  belongs on the server.

## Current Boundary Notes

- Server already owns review status, diff hashes, review migration, comment
  mutation, comment scope checks, and comment re-anchoring.
- App owns UI state, navigation, selection, status messages, undo intent, and
  render-model projection.
- App still performs repo reads for:
  - current file content,
  - base file content,
  - blame data,
  - current diff refresh with local diff options.
- The protocol has `get_file_content` types and method routing, but the server
  currently returns not implemented for that method.
- The app composes "current comments plus previous unresolved comments" with two
  client calls; this may be acceptable, or it may be server query policy.

## Key Questions

- Which git reads must be server-authoritative for all clients?
- Which git reads are presentation-only and safe to keep app-local?
- Should diff algorithm, whitespace mode, and diff-base display remain app
  preferences, or become server query parameters?
- Should blame be treated as presentation data or server-owned repo data?
- Can server APIs batch content, diff, blame, and comments to avoid extra
  round trips during common navigation?
- How should server-side caching be invalidated when the worktree changes?
- Does keeping app-side git access block remote, sandboxed, or non-TUI clients?
- What minimum API changes would let the app stop reading the repo directly?

## Assessment Tasks

1. Inventory direct git/worktree calls outside `src/server/` and classify each
   as canonical state, query data, or presentation cache.

2. Map each app-side git read to an existing or proposed server API:
   - `get_file_content`,
   - configurable `get_file_diff`,
   - blame retrieval,
   - combined file snapshot query.

3. Evaluate a hybrid model:
   - server owns canonical review/comment/repo facts,
   - app may keep local presentation-only reads when running colocated with the
     repo,
   - remote/sandboxed clients use server APIs only.

4. Evaluate a strict model:
   - all git and filesystem reads happen server-side,
   - app only consumes server snapshots and query results.

5. Identify compatibility requirements for MCP and HTTP clients:
   - no direct Unix socket assumptions,
   - no direct filesystem access assumptions,
   - consistent results across multiple clients.

6. Prototype or sketch the minimum server contracts needed for a strict model.

7. Decide whether to migrate immediately, defer, or keep a documented hybrid
   split.

## Deliverables

- A written inventory of current app-side git accesses.
- A recommendation: strict server-owned git access, hybrid model, or no change.
- Proposed API changes if migration is recommended.
- Risk notes for performance, caching, multi-client consistency, and remote
  client support.
- Follow-up implementation plan if the recommendation is to migrate.

## Acceptance Criteria

- [ ] Every direct non-server git/worktree access is identified.
- [ ] Each access is classified as canonical, query, or presentation-only.
- [ ] The assessment states whether app-side git access is acceptable long term.
- [ ] Required server API additions are listed with rough request/response
      shapes.
- [ ] Tradeoffs are documented for local TUI, MCP, HTTP, GUI, and remote
      clients.
- [ ] A follow-up implementation plan exists if migration is recommended.
