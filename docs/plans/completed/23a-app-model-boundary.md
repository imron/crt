# Stage 23a: AppModel Boundary

## Status: Complete

## Order

4.1 of 7 (recommended implementation order)

## Depends On

- Stage 20 (`20-thin-client-api-contract.md`)
- Stage 22 (`22-extract-domain-usecases-from-tui.md`)
- Stage 23 (`23-tui-intent-only-input.md`)

## Goal

Move the application toward a video-game-style architecture where `App` owns a
single UI-agnostic `AppModel`, input updates that model, and UI adapters render
the model using backend-specific layout and hit mapping.

## Target Architecture

```text
native UI event -> UI adapter -> AppInput
AppInput -> App mutates AppModel
TUI/GUI render AppModel with backend-specific layout
backend hit map -> semantic AppInput
```

`AppModel` is the shared conceptual model of the review UI. It describes what
exists in the application, but not how it maps to terminal cells, pixels, or
toolkit widgets.

## AppModel Owns

- File list panel concepts: sections, files, paths, review states, change
  kinds, selected file, and section counts.
- Diff panel concepts: current file, hunks, diff lines, line states, word
  highlights, cursor anchors, selected text anchors, and search highlights.
- App-level concepts: focus, prompts, overlays, status messages, configuration
  that affects the application experience, and semantic selections.

## UI Adapters Own

- Native event capture and toolkit details.
- Terminal cell or pixel layout.
- Backend-specific hit maps from native coordinates to semantic app targets.
- Prompt widget editing details such as cursor placement.
- Renderer-specific style conversion from shared app style configuration.

## Implementation Plans

1. Stage 23b: AppModel Contract Correction
2. Stage 23c: AppModel Scaffold
3. Stage 23d: Remove Render State from AppState
4. Stage 23e: Render TUI from AppModel
5. Stage 23f: Route Input through AppModel Targets
6. Stage 23g: Remove app_update

## Acceptance Criteria

- [x] The AppModel architecture is documented as the current target.
- [x] Existing Stage 20 `RenderModel` wording is explicitly superseded.
- [x] Each implementation slice has a dedicated plan with acceptance criteria.
- [x] Stage 24 depends on completion of the AppModel boundary work.

## Progress

- Corrected architecture docs so `AppModel` is the current shared UI model,
  while Stage 20 `RenderModel`/`InteractionMap` types were documented as
  scaffolding superseded by the AppModel migration and later removed from
  active code.
- Added the initial `AppModel` scaffold as a pure app-owned projection without
  changing TUI rendering or input behavior.
- Removed terminal render state from `AppState`: pane geometry, rendered text,
  hunk row maps, gutter measurements, and file row hit maps now live in
  `TuiState`; the temporary app reducer dependency was removed in the final
  AppModel boundary slice.
- Converted TUI rendering to consume `AppModel` for app content. The renderer
  now updates only TUI-owned render caches; viewport-dependent app scroll
  clamping happens in the TUI runtime after rendering.
- Routed TUI pointer hits through `AppModel` and semantic app targets. File
  selection now reaches app code as a file index rather than a rendered TUI
  row, while terminal coordinates remain inside the TUI adapter.
- Moved app-domain input handling and reducer logic behind `App`. The TUI now
  handles native events, prompt editing, TUI presentation effects, and render
  state while calling `App` methods for core interaction context, effect
  application, and search/definition navigation.

## Resolved Decisions

- The shared model is named `AppModel`, not `AppWorld`.
- There are not separate shared TUI and GUI render models.
- TUI and GUI produce backend-specific layout and hit maps from the same
  `AppModel`.
