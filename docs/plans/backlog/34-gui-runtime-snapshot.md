# Stage 34: GUI Runtime and Stub Snapshot

## Status

Implemented on `wip/gui`.

## Goal

Start the tokio runtime before the SwiftUI window, connect to the crt
server the same way review mode does, and show a small owned snapshot
in the placeholder UI.

This slice does not take a base ref, load `App`, or project `AppModel`.

## Why

`NSApplication.run()` owns the process main thread. The embedded
server and client I/O must keep running on tokio worker threads
while the GUI is up. The next unknown is that split, plus passing
a few C strings into Swift after connect.

## Requirements

1. `crt gui` creates a multi-thread tokio runtime, connects with
   `Client::connect_or_start` on the default socket, then calls
   `crt_gui_run` on the main thread.
2. Resolve git context from the current directory (repo name,
   worktree, HEAD). Do not `init` a review session yet.
3. Pass version, repo name, worktree, HEAD, and connection status as
   one JSON envelope (`{"data": ...}`) with pointer + length. Swift
   copies the bytes immediately and decodes them.
4. After the window closes, shut down the client so an embedded
   server started by this process is cancelled.
5. `crt gui` still rejects a base ref, `--root`, `--reset`, and
   `--standalone`.

## Out of scope

- `crt gui <base>` / `--root`.
- File list from a review session.
- Semantic actions, keymaps, `.app` packaging.

## Acceptance Criteria

- [x] `cargo run -- gui` in a git worktree shows repo name, HEAD,
      and a connected status in the window.
- [x] Closing the window shuts down the client and returns to the
      shell.
- [x] `cargo test` still does not open a window.
