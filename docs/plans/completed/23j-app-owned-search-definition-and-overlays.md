# Stage 23j: App-Owned Search, Definition, and Overlay Workflows

## Status: Completed

## Order

4.10 of 7 (recommended implementation order)

## Depends On

- Stage 23i (`23i-app-owned-review-and-snapshot-workflows.md`)

## Goal

Move asynchronous search/definition workflows and conceptual overlay state out
of the TUI runtime and into the app model/session.

## Why

Search and go-to-definition are app features, not terminal features. The TUI
currently calls the client, interprets results, creates search/definition
overlays, decides no-result/error status messages, and performs navigation.
A GUI needs the same workflow and should render the same conceptual overlay
model.

The TUI should only render overlay state from `AppModel` and report overlay
selection input back to the app.

## Scope

- In scope: codebase search command execution.
- In scope: diff-only search command execution.
- In scope: definition lookup command execution.
- In scope: search/definition overlay result state.
- In scope: selected overlay row and overlay navigation semantics.
- Out of scope: prompt text editing mechanics, which remain UI-specific.

## Requirements

1. Move `SearchAll`, `SearchDiff`, and `FindDefinition` async execution out of
   `src/tui/runtime.rs`.

2. Store conceptual search/definition results in app-owned state and expose
   them through `AppModel`.

3. Move overlay selection and accept/close semantics behind `App` methods:
   - search result next/previous/first/last/accept,
   - definition result next/previous/accept,
   - overlay close.

4. Remove search/definition overlay result storage from `TuiState`.

5. Keep renderer-specific overlay layout, scroll windowing, and visual style
   in TUI code.

6. Keep prompt widget input buffers in TUI code, but route submit/cancel to app
   prompt semantics.

7. Ensure external definition targets are represented as app output/status or a
   future read-only file-view command, not handled only in TUI.

## Deliverables

- App-owned search/definition workflow methods.
- `AppModel` fields for search/definition overlay state.
- TUI overlay rendering from `AppModel`.
- TUI input routes overlay commands to app-owned handlers.
- Tests for no-result, multi-result, accept, close, and external-target cases.

## Acceptance Criteria

- [x] `src/tui/runtime.rs` no longer calls `search_codebase()` directly.
- [x] `src/tui/runtime.rs` no longer calls `find_definition()` directly.
- [x] `TuiState` no longer stores `SearchResults` or `DefinitionResults`.
- [x] Search and definition result overlays are represented in `AppModel`.
- [x] Existing search and definition workflows remain compatible.
- [x] `cargo test` passes.

## Completed Notes

- Search and definition command execution is now queued as app work from parsed
  app commands.
- `AppState` owns conceptual search and definition overlay state.
- `AppModel` projects overlay data for TUI/GUI renderers.
- TUI keeps only renderer-specific search overlay scroll state.
- The event loop keeps one app background/progress step per turn.

## Notes

- App-owned overlay state should remain UI-agnostic. TUI-specific scroll math
  for how many overlay rows fit on screen should remain in TUI.
- Preserve a clean single-pass TUI event loop while moving search/definition
  into App. The target shape is: render from `app.model()`, collect normalized
  input, pass that input to App, apply TUI-only effects, and let App make one
  background/progress step before the next render. Avoid reintroducing separate
  TUI calls for review, refresh, notifications, search, or definition
  workflows.
- Prefer input-oriented App API names. The UI adapter produces normalized
  `InputEvent`s, so `handle_input`, `process_input`, or `process_inputs` are a
  better fit than `process_ui_event`; "UI event" is too broad and sounds like
  terminal/widget mechanics rather than app input.
