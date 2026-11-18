# Stage 33: GUI Scaffold

## Status

Implemented on `wip/gui`.

## Goal

Prove that a SwiftUI window can live inside the `crt` rust binary and that
`cargo build` is the only step that embeds it.

`crt gui` opens a placeholder window and exits cleanly when the window
closes. This slice does not start a review session, talk to the server, or
render `AppModel`.

## Why

The GUI will be a thin Swift adapter linked into the rust process. The
unknown is the build and process wiring: SwiftPM static library, cargo
`build.rs` link, main-thread `NSApplication`, and a `crt gui` entry that
does not `block_on` the window.

Later slices can start the tokio runtime in the background and hand the
GUI a snapshot. They should not also be debugging the linker.

## Requirements

1. `gui/` is a SwiftPM static library with a `@_cdecl("crt_gui_run")`
   entry point and a placeholder SwiftUI window titled `crt`.
2. On macOS, `build.rs` compiles `gui/` into `OUT_DIR`, links it into
   `crt`, and emits `cargo:rerun-if-changed` for the Swift sources. No
   prebuilt Swift artifacts are checked in.
3. `crt gui` calls `crt_gui_run` on the process main thread and returns
   when the window closes (`cmd+q` or last window closed).
4. Rust passes the crate version and `CRT_BUILD_HASH` across the C ABI;
   the window displays that string.
5. On non-macOS targets, `cargo build` skips Swift and `crt gui` errors
   with a clear message.
6. `crt gui` rejects `--root`, `--reset`, `--standalone`, and a base ref
   for now (review args come in a later slice).
7. Tests parse the `gui` subcommand but do not launch the window.

## Out of scope

- Server connection, `App`, and `AppModel` snapshots.
- `crt gui <base>` / `--root`.
- Keymaps, menus beyond Quit, `.app` packaging, Finder launch.

## Acceptance Criteria

- [x] `cargo run -- gui` on macOS opens a window showing the version
      string.
- [x] Closing the window or `cmd+q` returns to the shell with exit 0.
- [x] Changing a Swift source and running `cargo build` rebuilds the GUI.
- [x] `cargo test` passes without opening a window.
