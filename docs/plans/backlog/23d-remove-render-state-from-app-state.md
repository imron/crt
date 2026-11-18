# Stage 23d: Remove Render State from AppState

## Status: Backlog

## Order

4.4 of 7 (recommended implementation order)

## Depends On

- Stage 23c (`23c-app-model-scaffold.md`)

## Goal

Move render-derived and backend-specific state out of `AppState`, keeping only
semantic application state in the app model.

## Requirements

1. Move TUI layout and hit data out of `AppState`, including terminal pane
   areas, row-to-file mappings, rendered text caches, and backend gutter
   measurements.

2. Keep conceptual state in `AppModel` or app state:
   - selected file,
   - file sections,
   - diff hunks and semantic lines,
   - cursor anchors,
   - search matches,
   - semantic selections.

3. Store TUI-specific render artifacts in TUI-owned state or a TUI render
   cache.

4. Preserve existing pointer and selection behavior while changing ownership.

## Deliverables

- TUI-owned render/hit cache for terminal-specific data.
- `AppState` no longer stores ratatui `Rect`s or terminal-only rendered text.
- Tests updated to assert semantic hit mapping through TUI-owned data.
- Progress note in Stage 23a.

## Acceptance Criteria

- [ ] `AppState` no longer depends on ratatui layout types.
- [ ] Terminal pane geometry is stored in TUI-owned state.
- [ ] Terminal row/cell hit maps are stored in TUI-owned state.
- [ ] App-owned state remains sufficient to build `AppModel`.
- [ ] Existing mouse click, drag selection, double-click copy, and wheel scroll
      behavior still works.
- [ ] `cargo test` passes.
