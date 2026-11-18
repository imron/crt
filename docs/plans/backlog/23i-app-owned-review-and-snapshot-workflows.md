# Stage 23i: App-Owned Review and Snapshot Workflows

## Status: Backlog

## Order

4.9 of 7 (recommended implementation order)

## Depends On

- Stage 23h (`23h-app-session-owns-client-and-config.md`)

## Goal

Move review toggling, file-list reload, selection restoration, and
notification-driven snapshot policy out of the TUI runtime and into the app
session.

## Why

The TUI currently decides how to mark/unmark reviews, how to apply review
results, which server notifications require a reload, and how to preserve
selection/cursor state across a fresh file snapshot. Those are app workflow
rules. A GUI would need the same behavior, so they should be shared.

The TUI should report semantic input such as "toggle selected review" or
"focus gained". The app should decide which service calls to make and how to
mutate the app model.

## Scope

- In scope: review toggle RPC workflow.
- In scope: applying `ReviewActionResult`.
- In scope: app-owned file snapshot reload.
- In scope: notification classification and reload policy.
- In scope: preserving app state across reloads.
- Out of scope: transport-loss recovery and bind-race failover.

## Requirements

1. Move review toggle workflow out of `src/tui/runtime.rs`:
   - determine selected file,
   - choose mark vs unmark,
   - call the review service,
   - apply result,
   - produce an app status/error update.

2. Move review-result application behind `App` or app session methods.

3. Move file-list snapshot reload behind `App`:
   - preserve selected path,
   - replace file list,
   - restore selection,
   - refresh selected diff/content/blame,
   - preserve cursor/content mode/render variant when appropriate.

4. Move notification policy behind `App`:
   - review changed,
   - reviews cleared,
   - reviews migrated,
   - future app-relevant notification kinds.

5. Replace TUI-owned `pending_review_toggle` and `pending_refresh` with
   app-owned commands/effects or direct app-session calls from semantic input.

6. Keep TUI-specific focus events as native input, but translate focus gained
   into an app-level refresh request.

## Deliverables

- App session methods for review toggle, snapshot reload, and notification
  handling.
- TUI runtime delegates review and refresh workflows to the app session.
- `TuiState` no longer stores review/snapshot workflow flags.
- Service-level tests for review toggle and reload state restoration.

## Acceptance Criteria

- [ ] `src/tui/runtime.rs` no longer calls `mark_reviewed()` or
      `unmark_reviewed()` directly.
- [ ] `src/tui/runtime.rs` no longer mutates `app.state.files` directly after
      a reload.
- [ ] Notification-to-reload policy is not implemented in TUI code.
- [ ] Review result application is covered by app/service tests.
- [ ] Existing review workflows remain functionally equivalent.
- [ ] `cargo test` passes.

## Notes

- This stage should make snapshot resync behavior easier for Stage 24 to call
  after reconnect.
