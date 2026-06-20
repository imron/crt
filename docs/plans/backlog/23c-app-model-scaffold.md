# Stage 23c: AppModel Scaffold

## Status: Backlog

## Order

4.3 of 7 (recommended implementation order)

## Depends On

- Stage 23b (`23b-app-model-contract-correction.md`)

## Goal

Introduce `AppModel` as a read-only UI-agnostic projection from current app
state without changing TUI behavior.

## Requirements

1. Add `AppModel` types for the conceptual review UI:
   - file list sections and rows,
   - diff panel content,
   - app focus,
   - semantic selection anchors,
   - prompt, overlay, and status concepts.

2. Add an app-owned projection method:
   - prefer `App::model(&self) -> AppModel` for the first slice unless a
     borrowed model is clearly simpler.
   - keep projection pure and free of TUI layout, ratatui types, terminal
     coordinates, and pixel/cell geometry.

3. Keep existing TUI rendering and input paths unchanged in this slice.

4. Do not remove `app_update` in this slice.

## Deliverables

- `AppModel` and nested semantic model types.
- Projection from current `App`/`AppState` into `AppModel`.
- Focused projection tests.
- Progress note in Stage 23a.

## Acceptance Criteria

- [ ] `AppModel` represents file list sections with file ids, paths, change
      kinds, review states, and selection.
- [ ] `AppModel` represents the current diff with hunks, lines, line kinds,
      word/search highlights where currently available, and cursor anchors.
- [ ] `AppModel` represents app focus, status, prompt, overlay, and semantic
      selection concepts without backend geometry.
- [ ] Existing TUI behavior is unchanged.
- [ ] `cargo test` passes.

## Resolved Decisions

- The first scaffold may clone data for simplicity.
- Borrowed model views can be introduced later if cloning becomes a measured
  performance problem.
