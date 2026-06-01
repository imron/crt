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

## Processed Render Contract

Defined in:

- `src/core/render.rs`

Core emits paired processed outputs:

- `RenderModel`: what to draw.
- `InteractionMap`: what rendered regions mean for interaction.

Update policy:

- `RenderUpdate::Snapshot` for init/reconnect/resync.
- `RenderUpdate::Delta` for steady-state incremental updates.

## Pane World Models

Defined in:

- `src/core/world.rs`

Pane semantic models:

- `FileListPaneWorld`
- `DiffPaneWorld`
- `OverlayPaneWorld`
- `PaneWorldModel` enum wrapper

These are canonical interaction spaces used by both rendering and input
mapping.

## Pointer Mapping Rules

### TUI Adapter

1. Convert terminal row/column to pane-local coordinates.
2. Resolve hit using `InteractionMap` region bounds/ids.
3. Emit `MouseEvent` with `semantic_hit` (and optional `local_pos`).
4. For text-precise actions, set `text_anchor` (`line`, `column`).

### GUI Adapter

1. Convert pixel coordinates to pane-local coordinates.
2. Resolve hit using `InteractionMap` region bounds/ids.
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
- pane worlds,
- render model and interaction map,
- business/domain rules.

UI-owned:

- terminal/window toolkit specifics,
- native event capture,
- prompt widget editing UX,
- rendering the core-provided model,
- coordinate conversion to semantic hits.

## Hotspot Mapping Table

| Current hotspot | Target core API/module |
| --- | --- |
| `src/keys.rs:handle_key_event` | `CoreInteractionEngine::handle_input` |
| `src/keys.rs:handle_command_input` | Prompt handshake (`prompt.rs` + `interaction.rs`) |
| `src/keys.rs:handle_diff_search_input` | Prompt handshake + search semantics in core interaction/services |
| `src/keys.rs:execute_command` | Core command interpretation (interaction + services) |
| `src/keys.rs:navigate_to_search_match` | Navigation service + pane worlds |
| `src/keys.rs:navigate_to_definition` | Navigation service + pane worlds |
| `src/keys.rs:request_go_to_definition` | Core input semantics + search service |
| `src/keys.rs:cycle_view_mode` | Diff/navigation services |
| `src/keys.rs:jump_to_next_hunk` | Navigation service |
| `src/keys.rs:jump_to_prev_hunk` | Navigation service |
| `src/keys.rs:navigate_file` | Review/navigation services |
| `src/app.rs:process_pending_command` | Core interaction + search/review services |
| `src/app.rs:process_review_toggle` | Review service |
| `src/app.rs:apply_review_result` | Review service/state reducer |
| `src/app.rs:reload_file_list` | Review/diff services + snapshot/delta policy |
| `src/app.rs:load_head_content` | Diff service |
| `src/app.rs:load_base_content` | Diff service |
| `src/app.rs:load_blame` | Diff service |
| `src/app.rs:refresh_current_file_diff` | Diff service |
| `src/app.rs:reload_current_diff` | Diff service |
| `src/app.rs:handle_mouse_event` | UI adapter mapping -> `InputEvent::Mouse` |
| `src/ui/*` render data prep coupling | `RenderModel` + `InteractionMap` producers |

## Stage 20 Artifacts

Implemented files:

- `src/core/mod.rs`
- `src/core/input.rs`
- `src/core/prompt.rs`
- `src/core/render.rs`
- `src/core/world.rs`
- `src/core/interaction.rs`
- `src/core/services.rs`

This completes Stage 20 contract scaffolding and documentation. Subsequent
stages migrate runtime behavior into these contracts.
