# Stage 23k: Thin TUI Runtime Cleanup

## Status: Completed

## Order

4.11 of 7 (recommended implementation order)

## Depends On

- Stage 23j (`23j-app-owned-search-definition-and-overlays.md`)

## Goal

Reduce the TUI runtime to terminal mechanics: native event capture, prompt
editing, backend hit maps, rendering, terminal lifecycle, clipboard, and
terminal-only presentation state.

## Why

After app-owned session, review, snapshot, search, and definition workflows
move behind the app boundary, the TUI should no longer contain business/app
workflow code. This final cleanup verifies the boundary and removes remaining
TUI state fields that exist only to drive app workflows.

## Target TUI Responsibilities

- Terminal setup/restore, raw mode, alternate screen, focus events.
- Crossterm event normalization into app/core input, without deciding app
  keybinding semantics.
- Prompt text editing mechanics and prompt cursor placement.
- Ratatui rendering of `AppModel`.
- TUI-owned render caches and hit maps from terminal cells to semantic targets.
- Pointer selection mechanics and OSC52 clipboard.
- Suspend/resume terminal mechanics.
- Pane border drag detection and reporting resulting layout changes to App.

## Target App Responsibilities

- Config values and config persistence.
- Client/service access and app workflows.
- Keybinding interpretation and app-mode-specific input semantics.
- Review/reload/search/definition/notification semantics.
- App status messages and conceptual overlays.
- App model projection.
- Layout settings that are part of app configuration.

## Requirements

1. Remove app workflow flags from `TuiState`:
   - review toggle pending state,
   - pending command execution,
   - pending refresh,
   - app-owned overlay result state.

2. Replace direct app state mutation in TUI effects/runtime with app methods
   where the mutation is not terminal-specific.

3. Route pane visibility and layout width changes as app/domain configuration
   updates, while keeping terminal measurement and drag tracking in TUI.

4. Decide whether transient status messages are app-owned, TUI-owned, or split:
   - app status for app workflows,
   - TUI-only status for terminal mechanics such as clipboard and Ctrl-C
     confirmation if needed.

5. Update Stage 20/23 docs if the final boundary differs from the documented
   target.

6. Add a focused "TUI boundary" check in tests or docs so future changes do not
   reintroduce client/config/workflow ownership into `src/tui/runtime.rs`.

## Deliverables

- Slimmed `src/tui/runtime.rs`.
- Slimmed `TuiState`.
- Updated docs describing the final app/TUI split.
- Focused tests for the remaining TUI adapter behavior.

## Acceptance Criteria

- [x] `src/tui/runtime.rs` has no direct review/search/definition RPC calls.
- [x] `src/tui/runtime.rs` has no app snapshot reload implementation.
- [x] `src/tui/runtime.rs` does not load or persist app config directly.
- [x] `TuiState` contains only terminal/render/prompt/clipboard/pointer state.
- [x] TUI renders from `AppModel` and sends normalized native input plus
      semantic pointer hits to App.
- [x] App-owned code, not TUI code, decides what app keybindings such as `r`,
      navigation keys, search keys, definition keys, diff mode keys, and
      command keys mean.
- [x] The App can plausibly be reused by a GUI without copying TUI workflow
      code.
- [x] `cargo test` passes.

## Boundary Audit

App owns:

- client/service access and review/search/definition/snapshot/notification
  workflows,
- keybinding interpretation and parsed command execution,
- conceptual pane visibility and file-list width,
- layout config persistence,
- conceptual search/definition overlay state,
- `AppModel` projection.

TUI owns:

- crossterm event capture and normalization,
- prompt text editing buffers and cursor positions,
- terminal setup/restore/suspend mechanics,
- ratatui rendering, terminal pane rectangles, hit maps, render caches, and
  TUI-only search overlay scroll windowing,
- pointer drag tracking and clipboard emission,
- TUI-only transient status for terminal mechanics such as copy messages and
  Ctrl-C confirmation.

Remaining intentional boundary:

- `src/tui/runtime.rs` clamps app cursor/scroll after rendering because the
  limits depend on terminal-rendered row counts. A later viewport/layout
  snapshot plan should revisit whether this becomes an app input/effect using
  a UI-provided viewport model.

## Completed Notes

- Pane visibility and file-list width moved from `TuiState` into app-owned
  state/config and are exposed through `AppModel.layout`.
- File-list width drag now reports the new width to `App`, which persists
  layout config.
- TUI effects no longer mutate app pane focus/visibility or persist config.
- Double-click path copy reads from `AppModel`, not directly from `AppState`.
- Added app/model tests for app-owned layout and TUI state tests for the
  remaining terminal presentation state.

## Notes

- Do this after the preceding app-owned workflow slices. Trying to make the TUI
  thin in one step risks producing a large, hard-to-review rewrite.
