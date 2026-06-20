# Stage 23f: Route Input through AppModel Targets

## Status: Backlog

## Order

4.6 of 7 (recommended implementation order)

## Depends On

- Stage 23e (`23e-render-tui-from-app-model.md`)

## Goal

Route TUI input through backend-owned hit maps into semantic app targets, then
submit app-domain input to `App`.

## Requirements

1. Define semantic app input targets for current interactions:
   - file row activation,
   - diff text anchor,
   - pane focus,
   - pane visibility,
   - overlay item selection,
   - prompt submit/cancel,
   - app commands and key actions.

2. TUI maps terminal cells to semantic app targets using TUI-owned hit maps.

3. App handles semantic input without knowing terminal coordinates, ratatui
   layout, or GUI pixel geometry.

4. Keep prompt text editing local to TUI widgets; submit/cancel remains
   semantic app input.

## Deliverables

- App-domain input/target types where existing core input is too low-level.
- TUI hit-map lookup from terminal cells to semantic targets.
- App input handling methods for semantic targets.
- Progress note in Stage 23a.

## Acceptance Criteria

- [ ] TUI mouse input no longer asks app state to interpret terminal
      coordinates.
- [ ] App input handlers receive semantic targets, not terminal cells.
- [ ] Keybindings and command mode behavior remain compatible.
- [ ] Pointer interactions continue to support file selection, diff cursor
      movement, selection, and scrolling.
- [ ] `cargo test` passes.
