# Stage 23b: AppModel Contract Correction

## Status: Complete

## Order

4.2 of 7 (recommended implementation order)

## Depends On

- Stage 23a (`23a-app-model-boundary.md`)

## Goal

Correct the architecture docs so `AppModel` is the shared renderable app state,
and the old Stage 20 `RenderModel`/`InteractionMap` wording is treated as
contract scaffolding that has been superseded.

## Requirements

1. Update Stage 20 contract documentation to describe the current target:
   - `App` owns a UI-agnostic `AppModel`.
   - UI adapters render `AppModel` with backend-specific layout.
   - UI adapters own hit maps from native coordinates to semantic targets.

2. Preserve the useful Stage 20 decisions:
   - low-level input ingress,
   - core/app-owned interpretation,
   - prompt handshake,
   - semantic pointer targets.

3. Mark `core::render::RenderModel` as scaffolding, not the active target
   interface for renderer-agnostic state.

4. Update stale docs that still describe `src/keys.rs`, `src/ui`, or
   `src/app.rs` as the TUI event/render owner.

## Deliverables

- Updated Stage 20 contract docs.
- Updated overview/module ownership docs.
- A short note in Stage 23a confirming the contract correction is complete.

## Acceptance Criteria

- [x] Docs consistently use `AppModel` for shared renderable app state.
- [x] Docs state that terminal cells and GUI pixels are UI-adapter concerns.
- [x] Docs state that backend hit maps are owned by TUI/GUI adapters.
- [x] No current architecture doc presents `RenderModel` as the final shared
      UI boundary.
- [x] No current architecture doc points new work at removed `src/keys.rs` or
      `src/ui` modules.
