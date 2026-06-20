# Stage 23h: App Session Owns Client and Config

## Status: Backlog

## Order

4.8 of 7 (recommended implementation order)

## Depends On

- Stage 23g (`23g-remove-app-update.md`)

## Goal

Move session construction, config loading, initial data loading, and client
ownership out of the TUI runtime and behind an app-owned boundary.

## Why

The TUI currently constructs the application by loading config, calling the
server client for the initial file snapshot, building `App`, and then forcing
the first selected file to load its diff. A future GUI needs the same behavior.
Keeping this in `src/tui/runtime.rs` makes the TUI a controller for app
lifecycle rather than a thin terminal adapter.

The app should own everything required for the application to run:

```rust
pub struct App {
    pub state: AppState,
    pub config: Config,
    // app-owned service/client boundary
}
```

The TUI should receive or create an app session and render/input against it; it
should not know how config and initial review data are loaded.

## Scope

- In scope: moving `Client` ownership out of `Tui`.
- In scope: moving config load/path ownership out of `TuiState`.
- In scope: app-owned initial snapshot loading.
- In scope: preserving existing startup and reset behavior.
- Out of scope: reconnect/failover supervision; that remains Stage 24.

## Requirements

1. Introduce an app-owned session/runtime boundary that contains:
   - `App`,
   - config path/persistence context,
   - the review/search client or service handle needed for app workflows.

2. Move `crate::config::load()` and `crate::config::config_path()` out of
   `src/tui/runtime.rs`.

3. Move the initial `list_changed_files()` call out of `Tui::new()`.

4. Move first-file diff/content/blame initialization behind an `App` or app
   session constructor.

5. Change `tui::run(...)` so the TUI is given an app-owned session instead of
   a raw `Client` plus `ConnectionContext`, or make `tui::run(...)` call a
   narrow app constructor and never touch the raw client directly.

6. Keep startup connection/bootstrap in CLI-level code until Stage 24 supplies
   a reusable reconnecting supervisor.

## Deliverables

- App-owned session/runtime type or equivalent `App` constructor.
- `src/tui/runtime.rs` no longer stores `Client`.
- `TuiState` no longer stores `config_path`.
- Initial file snapshot loading lives outside TUI.
- Focused tests for app session construction where practical.

## Acceptance Criteria

- [ ] `src/tui/runtime.rs` has no `Client` field.
- [ ] `src/tui/runtime.rs` does not call `crate::config::load()`.
- [ ] `src/tui/runtime.rs` does not call `list_changed_files()` during
      construction.
- [ ] Config remains app-owned and available to renderers through `App`.
- [ ] Startup behavior remains unchanged for `crt <base>` and `--reset`.
- [ ] `cargo test` passes.

## Notes

- This stage should not implement reconnect logic. It should only make the
  later Stage 24 reconnect supervisor reusable by both TUI and GUI.
