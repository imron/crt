# Stage 28: Comment Scope Semantics

## Status: Backlog

## Goal

Make comment visibility and mutation scope explicit and consistent across the
TUI, MCP tools, server API, and client helpers.

## Why

Comment APIs currently mix two related concepts:

- exact current review scope,
- unresolved carry-over comments from previous review bases.

The TUI unresolved-comments section needs both current comments and unresolved
carry-over comments. MCP agents need the same visible unresolved set so they can
discover and resolve outstanding review feedback. Server mutation endpoints
already allow unresolved carry-over comments by id, which means listing and
mutation semantics can diverge unless the scope rules are documented and tested
as first-class behavior.

Recent fixes aligned MCP listing with the TUI, but the policy still lives as a
client composition rule. This plan makes the intended semantics explicit and
decides whether that composition belongs in client helpers, server API
parameters, or both.

## Scope

- In scope: define named comment scopes and their expected visibility.
- In scope: align TUI, MCP, client, and server behavior with those definitions.
- In scope: document mutation behavior for current and carry-over comments.
- In scope: add tests for listing, summary counts, resolve, and unresolve.
- Out of scope: threaded comments, comment priorities, or reply workflows.
- Out of scope: base-side versus head-side anchoring, covered by stage 13k.

## Current Behavior

- Server `list_comments` can return exact current-scope comments.
- Server `list_comments` can return unresolved comments outside the current
  merge-base/head pair via `include_previous_bases`.
- Client code composes current comments plus previous unresolved comments for
  TUI and MCP visibility.
- MCP `list_review_comments` now uses the same visible unresolved set as the
  TUI.
- Server `resolve_comment` and `unresolve_comment` can operate on unresolved
  carry-over comments by id even when they are not exact current-scope rows.

## Proposed Semantics

Define and document these scopes:

1. `CurrentUnresolved`
   - exact current merge-base/head comments only,
   - unresolved comments only.

2. `CurrentWithResolved`
   - exact current merge-base/head comments only,
   - resolved and unresolved comments.

3. `PreviousBasesUnresolved`
   - comments outside the exact current merge-base/head pair,
   - unresolved comments only.

4. `VisibleUnresolved`
   - `CurrentUnresolved` plus `PreviousBasesUnresolved`,
   - de-duplicated by comment id.

5. `VisibleWithCurrentResolved`
   - `CurrentWithResolved` plus `PreviousBasesUnresolved`,
   - de-duplicated by comment id.

## Key Questions

- Should visible scopes become explicit server API options instead of client
  composition?
- Should MCP expose exact-current and visible scopes as separate tool options?
- Should summary counts use visible scopes by default or exact current scope?
- Should mutation endpoints continue accepting any unresolved carry-over
  comment by id?
- Should resolved carry-over comments ever appear outside exact-current detail
  lookups?
- How should selected review sessions communicate that visible comments may
  belong to older merge bases?

## Implementation Tasks

1. Write scope documentation in the protocol or comments documentation.

2. Decide where visible-scope composition belongs:
   - keep it in `Client`,
   - move it into server `list_comments`,
   - or support both exact and visible server scopes.

3. Add explicit request/response tests for every named scope.

4. Add MCP tests for:
   - visible unresolved listing,
   - file-path filtering,
   - summary counts,
   - no duplicate rows when a comment appears in both query legs.

5. Add server tests for mutation behavior:
   - resolving current comments,
   - resolving unresolved carry-over comments,
   - rejecting unrelated resolved comments unless they have a current-scope
     resolution event.

6. Audit user-facing labels so TUI and MCP distinguish exact current comments
   from visible unresolved feedback when needed.

7. Update overview docs and any agent-facing MCP tool descriptions.

## Acceptance Criteria

- [ ] Named comment scopes are documented with exact behavior.
- [ ] TUI unresolved comments and MCP `list_review_comments` use the same
      visible-scope semantics.
- [ ] MCP summary counts match MCP comment listing.
- [ ] Exact-current listing remains available for callers that need it.
- [ ] Resolve and unresolve semantics for carry-over comments are documented.
- [ ] Tests cover current, previous-base, visible, resolved, and duplicate
      cases.
- [ ] Tool descriptions make visible carry-over behavior clear to agents.
