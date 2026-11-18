# Stage 20 Thin Client Contract

This document records the implemented Stage 20 contracts for the thin-client
architecture.

## Core Interaction API

Core exposes a single UI-ingress entrypoint:

- `CoreInteractionEngine::handle_input(event, context) -> CoreEffects`

Defined in:

- `src/core/interaction.rs`

`CoreEffects` is a list of core-to-UI effects including prompt requests,
render updates, connection-state updates, and transient errors.

## InputEvent Protocol (Complete for Current TUI)

UI adapters send low-level events only.

Defined in:

- `src/core/input.rs`

Event set:

- `InputEvent::Key(KeyEvent)`
- `InputEvent::Mouse(MouseEvent)`
- `InputEvent::Resize { width, height }`
- `InputEvent::FocusGained`
- `InputEvent::FocusLost`
- `InputEvent::PromptSubmit { id, value }`
- `InputEvent::PromptCancel { id }`

Keyboard coverage:

- `Key::Char(char)` and all non-char keys used by current TUI navigation.
- `InputModifiers` supports `ctrl`, `alt`, and `shift` for existing and future
  keybindings.

Mouse/pointer coverage:

- `MouseEventKind::{Down, Up, Drag, Move, ScrollUp, ScrollDown}`
- Semantic hit support via `PointerSemanticHit`.

## Prompt Handshake Contract

Defined in:

- `src/core/prompt.rs`
- `src/core/interaction.rs`

Flow:

1. Core emits `CoreEffect::RequestPrompt(PromptRequest)`.
2. UI displays prompt and performs local text editing UX.
3. UI returns `PromptSubmit` or `PromptCancel` with `PromptId`.
4. Core validates/interprets input and emits follow-up effects.

This keeps prompt UX local while prompt semantics remain core-owned.

## Current AppModel Direction

Stage 20 introduced render/input scaffolding before the later App boundary
work clarified the target architecture. The current direction is:

- `App` owns a UI-agnostic `AppModel`.
- `AppModel` describes the conceptual review UI: file-list sections, files,
  review states, current diff, hunks, semantic highlights, focus, prompts,
  overlays, status, and semantic selections.
- TUI/GUI adapters render the same `AppModel` with backend-specific layout.
- TUI/GUI adapters own native hit maps from terminal cells or GUI pixels back
  to semantic app targets.

This supersedes the earlier `RenderModel` idea as the final shared UI
boundary.

## Retired RenderModel Scaffold

Stage 20 originally added paired processed render scaffolding:

- `RenderModel`: what to draw,
- `InteractionMap`: what rendered regions mean for interaction,
- pane world model structs for semantic coordinates.

Those scaffolding types have been removed from active code. New work should use
`AppModel` plus backend-owned hit maps.

## Pointer Mapping Rules

### TUI Adapter

1. Convert terminal row/column to pane-local coordinates.
2. Resolve hit using a TUI-owned terminal-cell hit map.
3. Emit `MouseEvent` with `semantic_hit` (and optional `local_pos`).
4. For text-precise actions, set `text_anchor` (`line`, `column`).

### GUI Adapter

1. Convert pixel coordinates to pane-local coordinates.
2. Resolve hit using a GUI-owned pixel/widget hit map.
3. Emit the same semantic `MouseEvent` shape as TUI.
4. For text-precise actions, set `text_anchor` (`line`, `column`).

Contract rule: core consumes semantic hits, not raw screen-space units.

## Snapshot vs Delta Policy

Snapshots are required for:

- initial load,
- reconnect + re-init resync,
- any explicit full invalidation.

Deltas are used for:

- steady-state interaction updates,
- localized pane/status changes.

Reconnect rule: after reconnect/re-init, process a snapshot before accepting
steady-state deltas.

## Ownership Split

Core-owned:

- input semantics and mode interpretation,
- keymap/command behavior,
- `AppModel` and app-domain semantic targets,
- business/domain rules.

UI-owned:

- terminal/window toolkit specifics,
- native event capture,
- prompt widget editing UX,
- rendering `AppModel`,
- coordinate conversion to semantic hits.

## Hotspot Mapping Table

| Current hotspot | Target core API/module |
| --- | --- |
| `src/tui/input.rs` key normalization | App/Core input handling |
| `src/tui/input.rs` prompt submit/cancel | Prompt handshake |
| `src/app/update.rs` reducers | Private implementation behind `App` |
| `src/tui/runtime.rs` async commands | App-requested work run by runtime |
| `src/tui/runtime.rs` review toggle RPC | Review service + app-owned result |
| `src/tui/runtime.rs` file-list reload | Review/diff services + snapshots |
| `src/app.rs` diff/content/blame helpers | Diff service + model projection |
| `src/tui/input.rs` mouse normalization | Adapter hit map -> app input |
| `src/tui/render/*` data prep coupling | Render `AppModel` + TUI hit maps |

## Stage 20 Artifacts

Implemented files:

- `src/core/mod.rs`
- `src/core/input.rs`
- `src/core/prompt.rs`
- `src/core/render.rs`
- `src/core/interaction.rs`
- `src/core/services.rs`

This completes Stage 20 contract scaffolding and documentation. Stage 23a and
its follow-up plans supersede the render boundary with the `AppModel`
architecture.
