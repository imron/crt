# Stage 20: Thin Client API Contract

## Status: Completed (Contract Scaffolding)

## Order

1 of 7 (recommended implementation order)

## Depends On

- Stage 19 decisions (`19-shared-server-failover-lifecycle.md`)

## Goal

Define a stable core interaction contract so TUI and future GUI are thin
input/render adapters over shared logic.

## Why

`src/app.rs` and the former key/input layer mixed UI concerns with domain rules.
Without a clear contract, extraction work becomes ad hoc and GUI support will
duplicate behavior.

This stage adopts a stricter model:

- UI sends low-level input events only.
- Core interprets inputs, keymaps, modes, and commands.
- Core decides whether work is server-authoritative (mutations) or local
  query/render preparation.
- UI captures prompt text UX, but core owns prompt semantics.

## Scope

- In scope: low-level input protocol, core interaction entrypoint, prompt
  handshake, and render-model contract.
- In scope: ownership rules for sync/async work and error surfaces.
- Out of scope: full implementation of all services (handled in later stages).

## Requirements

1. Define a UI-neutral `InputEvent` protocol for core ingress, including:
   - key/chord events,
   - text input editing events when needed,
   - mouse/resize/focus events,
   - prompt submit/cancel events.

2. Define one core interaction entrypoint for UI adapters:
   - `handle_input(event, context) -> CoreEffects` (name may vary),
   - core interprets keymap/mode and performs semantic actions,
   - UI does not call domain methods directly.

3. Define prompt handshake contract:
   - core requests prompt via effect (`RequestPrompt { id, kind, ... }`),
   - UI owns prompt widget/text capture UX,
   - UI returns `PromptSubmit { id, value }` or `PromptCancel { id }`,
   - core validates/parses/executes semantics.

4. Define initial processed render scaffolding. This was later superseded by
   Stage 23a's `AppModel` direction:
   - `AppModel`: shared conceptual review UI state,
   - backend-owned hit maps: terminal cells or GUI pixels to semantic targets.

5. Define pane world models in core (e.g. file list, diff, overlays) that are
   the canonical semantic coordinate space for interaction behavior.

6. Define pointer/mouse mapping contract:
   - UI adapters map native coordinates (terminal rows/cols or GUI pixels)
     into pane-local coordinates,
   - UI adapters resolve pane-local hits using backend-owned hit maps,
   - UI sends semantic input events/hits to core (not raw screen units).

7. Define ownership split:
   - core/app-owned state: session/domain state, mode machine, keymap,
     command interpretation, business rules, and `AppModel`,
   - UI-owned state: terminal/window mechanics and pure presentation details
     (pane geometry, widget-local cursor drawing, etc.).

8. Define error classes visible to clients:
   - transport/transient,
   - invalid user input,
   - domain/repo/data errors.

9. Document threading/async contract:
   - which calls may block,
   - which are async,
   - how clients update UI while calls are in flight.

10. Document event/notification contract for state refresh:
   - what can be pushed,
   - what requires pull-based reload.

11. Keep a feature-oriented internal service boundary (`core::services`) for:
   - session/init,
   - file list/review operations,
   - diff/content/blame loading,
   - search/definition,
   - navigation helpers.

## Implementation Notes

- Keep names concrete and feature-oriented (`toggle_review`, `reload_files`,
  `search_codebase`, etc.) to avoid over-abstracting.
- Avoid introducing transport-specific details (JSON-RPC) into the interaction
  contract.
- Use this stage to decide where current `AppState` fields belong.
- Preserve process-coupled embedded lifecycle assumptions from Stage 19.
- Keep coarse and fine-grained interactions unified through app model semantics:
  pane-level actions (focus/resize/scroll) and text-precise actions
  (cursor/selection/anchors) both map through semantic targets.

## Design Rationale

- Low-level input ingress keeps UI adapters thin and consistent across TUI/GUI.
- Core-owned interpretation prevents behavior drift between interfaces.
- Prompt handshake keeps UX flexible (UI) while preserving semantic ownership
  (core).
- Stage 20's initial `RenderModel + InteractionMap` scaffold was later
  superseded by `AppModel` plus backend-owned hit maps and removed from active
  code.
- Snapshot-plus-delta model balances correctness and performance:
  - snapshots are safest for reconnect/resync,
  - deltas avoid heavy redraw/state copy costs during normal interaction.

## Deliverables

- A new design doc section or module-level docs describing the contract.
- Skeleton Rust types for `InputEvent`, prompt handshake, effects, and error
  categories.
- Internal service traits/APIs for feature use-cases.
- A mapping table from current TUI hotspots to target core entrypoint and
  service methods.

## Acceptance Criteria

- [x] Core interaction API is documented and committed in code as interfaces/types.
- [x] A complete `InputEvent` list exists for current TUI actions.
- [x] Prompt handshake contract is documented and represented in code types.
- [x] Initial `RenderModel + InteractionMap` scaffolding was documented.
      Stage 23a superseded this as the final shared UI boundary, and the
      scaffold types were later removed from active code.
- [x] Pane world model concerns were folded into `AppModel` and backend-owned
      hit maps.
- [x] Pointer mapping rules (native -> pane-local -> semantic hit) are
      documented for both TUI and GUI adapters.
- [x] Snapshot-vs-delta policy is documented with reconnect/resync rules.
- [x] Ownership split (core vs client state) is explicitly documented.
- [x] Mapping from app/input hotspots to target core APIs is complete.
- [x] No unresolved ambiguity remains about where extracted logic should live.

## Resolved Decisions

- UI sends low-level input events; core interprets semantics.
- UI prompt widgets capture text; core requests prompts and interprets results.
- Core interaction uses a single input entrypoint for UI adapters.
- Processed rendering was initially scaffolded as `RenderModel +
  InteractionMap`; Stage 23a superseded that with `AppModel` as shared
  conceptual app state and backend-owned hit maps, and the scaffold types were
  removed from active code.
- Snapshot/resync policy remains relevant for reconnect and state reloads.
