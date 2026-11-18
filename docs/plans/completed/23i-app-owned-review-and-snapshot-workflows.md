# Stage 23i: App-Owned Review and Snapshot Workflows

## Status: Completed

## Order

4.9 of 7 (recommended implementation order)

## Depends On

- Stage 23h (`23h-app-session-owns-client-and-config.md`)

## Goal

Move app keybinding interpretation, review toggling, file-list reload,
selection restoration, and notification-driven snapshot policy out of the TUI
runtime and into `App`.

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

## Keybinding Audit

App-owned interpretation:

- Normal-mode app commands are interpreted by core/app input handling: `q`,
  `Ctrl-C`, `:`, `/`, `?`, `r`, `Ctrl-N`, `Ctrl-P`, `[`, `]`, `Ctrl-]`,
  `Ctrl-T`, `Tab`, `1`, `2`, `i`, `s`, `d`, `m`, diff search `n`/`N`/`Esc`,
  and overlay navigation/accept/cancel keys.
- Diff cursor movement is interpreted by core/app input handling, including
  line/page/half-page movement, scroll wheel movement, top/bottom/view
  positioning, character movement, line start/end, and word movement.
- Semantic pointer clicks are normalized by the TUI but interpreted by
  core/app input handling as app targets such as file selection and diff cursor
  movement.
- Focus gained is forwarded as native input and the app decides to queue a file
  snapshot refresh.

TUI-owned mechanics:

- Prompt buffer editing remains TUI-local: character insertion, cursor motion,
  backspace/delete, submit, and cancel are converted into prompt submit/cancel
  app input events.
- Terminal-only operations remain TUI-local: alternate-screen setup/restore,
  actual process suspension after the app requests it, mouse drag tracking,
  border resizing, clipboard emission, and double-click word/path copying.
- TUI presentation state remains TUI-local for this stage: help/search/
  definition overlay visibility and pane visibility/layout are still rendered
  and tracked by the TUI, although their keybinding interpretation comes from
  core/app input handling.

Ambiguous or deferred:

- Search and definition service execution still live in the TUI runtime after
  this stage and are covered by Stage 23j.
- Remaining TUI presentation cleanup, including pane/layout presentation
  state, is covered by Stage 23k and the later AppModel plans.

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

- `App` methods for key-driven review workflow, snapshot reload, and
  notification handling.
- TUI runtime forwards normalized input and delegates review/refresh workflows
  to `App`.
- `TuiState` no longer stores review/snapshot workflow flags.
- App-level tests for review toggle queuing, review result application, reload
  state restoration, and notification reload policy.

## Acceptance Criteria

- [x] `src/tui/runtime.rs` no longer calls `mark_reviewed()` or
      `unmark_reviewed()` directly.
- [x] The implementation audits all existing keybindings and records which are
      app-owned versus TUI-owned.
- [x] TUI code does not decide that `r` or any other app binding maps to a
      specific app action.
- [x] App-owned input/keybinding code decides whether review toggle is active
      for the current app mode/context.
- [x] `src/tui/runtime.rs` no longer mutates `app.state.files` directly after
      a reload.
- [x] Notification-to-reload policy is not implemented in TUI code.
- [x] Review result application is covered by app/service tests.
- [x] Existing review workflows remain functionally equivalent.
- [x] `cargo test` passes.

## Notes

- This stage should make snapshot resync behavior easier for Stage 24 to call
  after reconnect.
