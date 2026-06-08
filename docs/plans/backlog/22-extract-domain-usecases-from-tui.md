# Stage 22: Extract Domain Use-Cases from TUI

## Status: Complete

## Order

3 of 7 (recommended implementation order)

## Depends On

- Stage 20 (`20-thin-client-api-contract.md`)
- Stage 21 (`21-protocol-type-decoupling.md`)

## Goal

Move business/domain behavior out of `src/app.rs` and `src/keys.rs` into shared
core services and core interaction modules.

## Why

The TUI currently owns rules that should be reusable by GUI and other clients:
review state transitions, diff base policy, sorting, search workflows, and
navigation semantics.

After Stage 20, core owns input semantics, pane world behavior, and processed
render semantics. This stage performs the concrete extraction needed to match
that contract.

## Scope

- In scope: extraction of non-rendering, non-terminal domain logic.
- In scope: creation of reusable use-case services, interaction helpers, and
  pane-world behavior modules.
- Out of scope: full intent-only key mapping cleanup (Stage 23).

## Requirements

1. Extract review workflow logic into a review service:
   - mark/unmark behavior,
   - list sorting/grouping rules,
   - selection advancement rules after review actions.

2. Extract diff/content/blame loading logic into a diff service:
   - effective diff base policy,
   - diff reload/recompute,
   - file content and blame retrieval orchestration.

3. Extract navigation logic into a navigation service:
    - hunk jump behavior,
    - new/old line mapping,
    - jump stack behavior.

4. Extract command/search/definition semantics into core interaction modules
   and search services, including result shaping for client overlays.

5. Move stateful pane interaction behavior (file-list selection mapping, diff
   cursor semantics, text-precise navigation helpers) into core-owned modules
   where feasible.

6. TUI retains rendering, event loop, and transient UI presentation only.

7. Remove direct `crate::git::Repo::open(...)` usage from `src/app.rs`.

## Implementation Notes

- Migrate in small, behavior-preserving slices (service by service).
- Keep existing keybindings and UI behavior unchanged during extraction.
- Introduce focused unit tests alongside each extracted service.

## Deliverables

- New service modules in core.
- New/updated core interaction modules for command/search/navigation semantics.
- `src/app.rs` updated to call service methods rather than embedding rules.
- `src/keys.rs` reduced usage of domain helpers owned by TUI.

## Progress

- Extracted review-list ordering, unreviewed counting, effective diff-base
  selection, selected-file restoration, and post-review auto-advance rules
  into `core::review` with focused unit tests.
- Extracted AppState git orchestration for diff algorithm resolution, file
  content loading, blame loading, and diff fallback loading into `core::diff`.
- Extracted line-number mapping, hunk jump targeting/scrolling, and
  section-scoped file cycling into `core::navigation` with focused unit tests.
- Extracted search result shaping, definition result routing, and
  path-to-location target resolution into `core::search` with focused unit
  tests.
- Extracted command parsing into `core::command` and replaced stringly typed
  pending commands with typed command requests.
- Added focused `core::diff` tests for diff algorithm selection, content
  loading, blame gating, and diff-base fallback behavior.
- Human review approved functional equivalence for existing workflows.

## Acceptance Criteria

- [x] No direct git orchestration remains in TUI application state methods.
- [x] Review/diff/navigation/search rules are exercised by service-level tests.
- [x] Command/search/definition semantics are no longer implemented in TUI
      business-logic helpers.
- [x] TUI behavior remains functionally equivalent for existing workflows.
- [x] New service code is interface-reusable for GUI.

## Resolved Decisions

- Navigation helpers that mutate or depend on session/view semantics are
  exposed via core interaction/services.
- Pure stateless calculations may remain helper functions, but live in core
  modules (not UI modules).
