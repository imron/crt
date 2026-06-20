# Stage 23g: Remove app_update

## Status: Backlog

## Order

4.7 of 7 (recommended implementation order)

## Depends On

- Stage 23f (`23f-route-input-through-app-model-targets.md`)

## Goal

Remove the transitional `app_update` module and make `App` the boundary for
app-domain input handling and app-owned reducer logic.

## Requirements

1. Move app reducer logic behind `App`:
   - interaction context construction,
   - prompt submit context construction,
   - core effect application,
   - command effects,
   - search/definition navigation,
   - review/navigation/diff state mutations.

2. Keep TUI presentation effects in TUI code:
   - prompt widget state,
   - overlay scroll/selection,
   - terminal status presentation,
   - pane visibility/layout persistence,
   - suspend/quit event loop mechanics.

3. Replace `AppUpdate` with a narrow app-owned output/request type only for
   external work that app logic cannot perform synchronously.

4. TUI must call methods on `self.app`; it must not import an app-internal
   reducer module.

## Deliverables

- `src/app_update.rs` deleted.
- `mod app_update` removed.
- TUI runtime/effects call app methods instead of `crate::app_update`.
- Tests updated for the new `App` boundary.
- Progress note in Stage 23a.

## Acceptance Criteria

- [ ] No `app_update` module remains.
- [ ] TUI code no longer imports app reducer helpers directly.
- [ ] `App` owns app-domain input handling.
- [ ] TUI remains thin: native event capture, prompt editing, backend hit maps,
      rendering, and event-loop mechanics.
- [ ] `cargo test` passes.

## Resolved Decisions

- Removing `app_update` happens after the AppModel and semantic input
  boundary are stable.
- The returned app output type must not be treated as a render model.
- Revisit the top-level `src/model.rs` name after the AppModel migration is
  complete. It remains as shared DTO/domain vocabulary for now to avoid mixing
  a broad rename into the app/UI boundary work.
