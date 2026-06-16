# Stage 23: TUI Input Adapter Refactor

## Status: In Progress

## Order

4 of 7 (recommended implementation order)

## Depends On

- Stage 20 (`20-thin-client-api-contract.md`)
- Stage 22 (`22-extract-domain-usecases-from-tui.md`)

## Goal

Refactor TUI input handling so `src/keys.rs` acts as a thin adapter from
terminal events to core `InputEvent` messages, without domain decision logic.

## Why

Current key handling mixes key mapping with behavior rules, which prevents
clean reuse in GUI and increases coupling to TUI-only state.

## Scope

- In scope: key/mouse/resize/focus translation into `InputEvent`, local prompt
  widget handling, and dispatch wiring.
- In scope: moving command parsing semantics behind core input interpretation.
- In scope: mapping native pointer coordinates through `InteractionMap`
  semantics from Stage 20.
- Out of scope: reconnect/failover transport concerns (Stage 24).

## Requirements

1. Implement translation from crossterm events into UI-neutral `InputEvent`
   values used by the core interaction entrypoint.

2. Implement pointer translation contract for TUI:
   - native terminal row/column -> pane-local coordinates,
   - pane-local coordinates -> semantic hit via `InteractionMap`,
   - semantic hit emitted as `InputEvent`.

3. `keys` layer responsibilities become:
   - adapter-level event normalization,
   - local text input editing/render for prompt widgets,
   - prompt submit/cancel emission back to core.

4. Domain effects (review toggles, navigation rules, diff mode cycling,
   search/definition actions) are executed by core input interpretation and
   services outside `keys`.

5. Keep current keybinding behavior and command syntax compatible.

6. Keep overlay routing behavior (help/search/definition/modal contexts),
   but domain action execution still occurs in core.

## Implementation Notes

- Start with one vertical slice (`r`, `:`, `/`) before migrating all keys.
- Preserve current transient status text semantics where practical.
- Keep TUI-specific presentation state (prompt cursor, overlay selection) in
  client layer.
- Keep prompt editing local to TUI widgets; only submit/cancel and non-text
  input semantics are sent to core.

## Deliverables

- Input adapter translation and dispatch plumbing.
- Refactored `src/keys.rs` with no business-rule helpers.
- Updated app loop forwarding `InputEvent` values to core.

## Progress

- Added crossterm key-to-core `InputEvent` translation for TUI key events.
- Routed `:` and `/` prompt-opening keys through `CoreInteractionEngine`
  while preserving local TUI prompt text editing and existing key behavior.
- Added focused tests for core prompt requests and TUI key normalization.
- Routed command and diff-search prompt submit/cancel through core
  `PromptSubmit`/`PromptCancel` events, with typed effects applied by the TUI.
- Routed the `r` review-toggle key through core input as a typed review effect.
- Routed `Ctrl-n`/`Ctrl-p` file navigation through core input as typed
  navigation effects.
- Routed hunk jumps, go-to-definition, and jump-stack pop through core input
  as typed navigation effects.
- Fixed go-to-definition symbol extraction to use the current diff column
  cursor instead of the first identifier on the line.
- Routed pane focus, pane visibility, diff rendering, view mode, diff
  algorithm, and diff-base toggles through core input as typed effects.
- Routed active diff-search next, previous, and clear actions through core
  input as typed search effects.
- Routed quit, suspend, help open, and help dismiss keys through core input as
  typed app/session effects.
- Routed search and definition results overlay close, selection movement, and
  accept-selected keys through core input as typed overlay effects.
- Routed diff cursor line, page, viewport, horizontal, and word movement keys
  through core input as typed cursor effects.
- Routed focused-pane `Enter` activation through core input as typed pane
  effects.
- Routed Ctrl-C quit confirmation through core input using adapter-supplied
  confirmation context.
- Moved active diff-search navigation/clear gating into core input context.
- Removed normal-mode and overlay key fallback semantics from the TUI adapter
  after routing those paths through core input.
- Added TUI mouse-to-core input translation with pane-local positions and
  semantic pane/text hits while preserving existing mouse behavior.
- Routed diff-pane mouse-wheel scrolling through core input as semantic cursor
  effects.
- Routed file-list and diff-pane left-click activation through core input as
  semantic pane/cursor effects.
- Migrated drag selection, double-click copy, and selection highlighting to
  semantic text anchors instead of raw terminal content coordinates.
- Moved core-effect application and app-state update helpers out of `keys.rs`
  into the App update step so key handling remains an input/prompt adapter.
- Moved terminal input and rendering modules under the `tui` module boundary.
- Moved the terminal runtime shell into `tui::Tui`, leaving the mixed
  AppState/TUI state split for a later Stage 23 slice.
- Moved the diff render cache into TUI-owned state so `AppState` no longer
  depends on `tui::render` internals.
- Moved prompt widget mode, buffers, cursors, and active prompt id into
  TUI-owned state, with prompt core effects applied at the TUI boundary.
- Moved active mouse selection, drag anchors, double-click tracking, and border
  drag state into TUI-owned state.
- Moved help overlay visibility into TUI-owned state while keeping core input
  routing aware of whether help is visible.
- Moved transient status bar messages into TUI-owned state, with app updates
  returning status presentation changes instead of storing timestamps in
  AppState.
- Moved event-loop runtime intents for review toggles, pending commands,
  suspend, quit, and focus refresh into TUI-owned state.
- Moved file-list layout width and config path into TUI-owned state, with app
  updates requesting layout persistence through the TUI boundary.
- Moved search and definition overlay result state into TUI-owned state while
  keeping accepted-result navigation in app update helpers.
- Moved pane visibility into TUI-owned state, with pane focus adjustments
  handled at the TUI effect boundary.

## Acceptance Criteria

- [ ] `src/keys.rs` does not call domain logic directly.
- [ ] `src/keys.rs` does not interpret semantic commands directly.
- [ ] Pointer interactions in TUI are emitted as semantic hits/events, not raw
      terminal coordinates.
- [ ] All existing keybindings still function.
- [ ] Command mode behavior remains compatible.
- [ ] Input adapter path is reusable by non-terminal UI clients.

## Resolved Decisions

- TUI prompt editing stays local to the UI widget layer.
- Core owns prompt semantics and receives prompt submit/cancel events.
