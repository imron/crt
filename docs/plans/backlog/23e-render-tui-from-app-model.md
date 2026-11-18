# Stage 23e: Render TUI from AppModel

## Status: Complete

## Order

4.5 of 7 (recommended implementation order)

## Depends On

- Stage 23d (`23d-remove-render-state-from-app-state.md`)

## Goal

Convert TUI rendering to consume `AppModel` rather than raw app state.

## Requirements

1. Render the file list from `AppModel`:
   - sections,
   - file rows,
   - review states,
   - selected row.

2. Render the diff panel from `AppModel`:
   - current file,
   - hunks and lines,
   - line kinds,
   - highlights,
   - cursor and semantic selection anchors.

3. Render prompts, overlays, and status from the shared app concepts plus
   TUI-owned widget details.

4. Keep style conversion at the TUI edge:
   - shared config stays app-owned,
   - ratatui `Style` values are built by TUI rendering code.

5. Keep visual behavior equivalent unless a difference is explicitly called
   out in the implementation notes.

## Deliverables

- TUI render entrypoint takes `AppModel`.
- File list and diff renderers no longer require mutable `AppState`.
- TUI render code updates TUI-owned hit maps during drawing.
- Progress note in Stage 23a.

## Acceptance Criteria

- [x] TUI rendering reads `AppModel` for app content.
- [x] TUI rendering does not mutate app state.
- [x] Renderer-specific style conversion stays in TUI code.
- [x] Existing visual layout and keyboard/mouse workflows remain compatible.
- [x] `cargo test` passes.

## Resolved Decisions

- TUI may keep its own render cache for performance.
- The render cache must not become the shared app model.
