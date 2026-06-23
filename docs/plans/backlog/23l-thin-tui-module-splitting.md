# Stage 23l: Thin TUI Module Splitting

## Status: Backlog

## Order

4.12 of 7 (recommended implementation order)

## Depends On

- Stage 23k (`23k-thin-tui-runtime-cleanup.md`)

## Goal

Split the largest app/TUI modules into smaller ownership-focused modules while
preserving the thin-TUI boundary.

## Why

The Stage 23 boundary is now mostly in place, but some files remain large
enough to slow review and make ownership drift harder to spot:

- `src/app/update.rs` mixes command application, cursor movement, overlays,
  diff search, and navigation helpers.
- `src/tui/render/diff_view.rs` mixes diff rendering, cache construction,
  line conversion, word highlighting, blame formatting, and tests.
- `src/tui/runtime.rs` still contains event-loop, mouse, clipboard, suspend,
  and selection mechanics in one file.

The goal is not to remove every repeated line. A little duplication is better
than introducing shared dependencies that blur App/TUI ownership.

## Scope

- In scope: splitting large modules by existing ownership boundaries.
- In scope: moving tests with the behavior they cover.
- In scope: keeping public APIs narrow and local to `app` or `tui`.
- Out of scope: changing input semantics, app workflows, rendering behavior, or
  reconnect/lifecycle behavior.

## Candidate Splits

1. Split `src/app/update.rs` into focused app-owned update modules:
   - command application,
   - diff cursor/search movement,
   - overlay result navigation,
   - pane/file navigation,
   - viewport helpers.

2. Split `src/tui/render/diff_view.rs` into TUI-owned rendering modules:
   - diff cache construction,
   - diff line/span conversion,
   - blame/full-file rendering,
   - reviewed-file summary rendering,
   - word/search highlighting.

3. Split `src/tui/runtime.rs` only if it improves boundary clarity:
   - event loop,
   - mouse/pointer handling,
   - clipboard/selection,
   - terminal lifecycle/suspend.

## Constraints

- Keep app logic in `app`/`core`; do not move workflows into `tui`.
- Keep backend geometry and render caches in `tui`; do not move terminal cells
  or ratatui details into `app`.
- Prefer simple sibling modules over reusable abstractions unless there is a
  clear ownership benefit.
- Avoid broad visibility changes just to make code compile; use narrow module
  structure and local helpers.

## Acceptance Criteria

- [ ] `src/app/update.rs` is split into smaller app-owned modules without
      behavior changes.
- [ ] `src/tui/render/diff_view.rs` is split into smaller TUI-owned render
      modules without behavior changes.
- [ ] Any `src/tui/runtime.rs` split preserves TUI-only responsibilities.
- [ ] No new dependency from `app` to `tui` is introduced.
- [ ] No review/search/definition/client/config workflow logic moves back into
      `tui`.
- [ ] `cargo test` passes.
