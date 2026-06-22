# Stage 23i: App-Owned Review and Snapshot Workflows

## Status: Backlog

## Order

4.9 of 7 (recommended implementation order)

## Depends On

- Stage 23h (`23h-app-session-owns-client-and-config.md`)

## Goal

Move app keybinding interpretation, review toggling, file-list reload,
selection restoration, and notification-driven snapshot policy out of the TUI
runtime and into the app session.

## Why

The TUI currently participates in app workflow decisions by carrying
review-toggle intent, deciding how to mark/unmark reviews, applying review
results, classifying notifications, and preserving selection/cursor state
across a fresh file snapshot. Those are app rules. A GUI would need the same
behavior, so they should be shared.

The TUI should not decide that a keystroke means "toggle selected review", or
that any other app keybinding means a specific app action. It should normalize
native terminal input into app/core input events and pass them through. The app
should own keybinding interpretation, decide whether a given input is active in
the current mode/context, choose service calls, and mutate the app model.

## Scope

- In scope: auditing all existing keybindings and classifying app-owned versus
  TUI-owned responsibilities.
- In scope: moving app keybinding interpretation out of TUI-owned code where
  any remains.
- In scope: app-owned keybinding interpretation for review workflow.
- In scope: review toggle RPC workflow.
- In scope: applying `ReviewActionResult`.
- In scope: app-owned file snapshot reload.
- In scope: notification classification and reload policy.
- In scope: preserving app state across reloads.
- Out of scope: transport-loss recovery and bind-race failover.

## Requirements

1. Audit the full keybinding surface and classify ownership:
   - app/domain bindings such as review, navigation, search, definition,
     diff mode, blame, whitespace, hunk movement, and commands belong to App,
   - terminal/widget mechanics such as prompt text editing, terminal suspend
     mechanics, mouse drag tracking, and clipboard emission remain TUI-owned,
   - ambiguous bindings must be explicitly documented before implementation.

2. Move app keybinding interpretation out of TUI:
   - TUI forwards normalized key/native input to the app,
   - app decides whether `r` or any other app binding means a specific app
     action,
   - app decides based on current app mode/context whether each binding is
     active,
   - TUI does not emit pre-interpreted app commands such as "review toggle".

3. Move review toggle workflow out of `src/tui/runtime.rs`:
   - determine selected file,
   - choose mark vs unmark,
   - call the review service,
   - apply result,
   - produce an app status/error update.

4. Move review-result application behind `App` or app session methods.

5. Move file-list snapshot reload behind `App`:
   - preserve selected path,
   - replace file list,
   - restore selection,
   - refresh selected diff/content/blame,
   - preserve cursor/content mode/render variant when appropriate.

6. Move notification policy behind `App`:
   - review changed,
   - reviews cleared,
   - reviews migrated,
   - future app-relevant notification kinds.

7. Replace TUI-owned `pending_review_toggle` and `pending_refresh` with
   app-owned input handling, commands/effects, or direct app-session calls.

8. Keep TUI-specific focus events as native input, but pass focus gained
   through so the app can decide whether to refresh.

## Deliverables

- App session methods for key-driven review workflow, snapshot reload, and
  notification handling.
- TUI runtime forwards normalized input and delegates review/refresh workflows
  to the app session.
- `TuiState` no longer stores review/snapshot workflow flags.
- Service-level tests for review toggle and reload state restoration.

## Acceptance Criteria

- [ ] `src/tui/runtime.rs` no longer calls `mark_reviewed()` or
      `unmark_reviewed()` directly.
- [ ] The implementation audits all existing keybindings and records which are
      app-owned versus TUI-owned.
- [ ] TUI code does not decide that `r` or any other app binding maps to a
      specific app action.
- [ ] App-owned input/keybinding code decides whether review toggle is active
      for the current app mode/context.
- [ ] `src/tui/runtime.rs` no longer mutates `app.state.files` directly after
      a reload.
- [ ] Notification-to-reload policy is not implemented in TUI code.
- [ ] Review result application is covered by app/service tests.
- [ ] Existing review workflows remain functionally equivalent.
- [ ] `cargo test` passes.

## Notes

- This stage should make snapshot resync behavior easier for Stage 24 to call
  after reconnect.
