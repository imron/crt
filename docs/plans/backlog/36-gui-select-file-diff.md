# Stage 36: GUI Select File and Diff

## Status

Implemented on `wip/gui`.

## Goal

Clicking a file in the GUI loads that file's diff from `App` and
shows unified diff text. The JSON envelope becomes a live loop:
Swift submits an action, rust updates `App`, rust pushes a new
snapshot.

## Why

Stage 35 is a one-shot snapshot. The next unknown is FFI after
`NSApplication.run()` has taken the main thread: an action from
Swift and a snapshot callback into SwiftUI.

`AppState::select_file` / `on_file_changed` already load hunks
synchronously from the worktree, so this slice does not add a new
git path.

## Requirements

1. Keep `App` alive in a session mutex while the window is open.
   Shut it down when the window closes, as today.
2. Swift submits `{ "data": { "type": "select_file", "path": ... } }`
   via `crt_gui_submit_action(ptr, len)`.
3. Rust applies `select_file` on `App` and calls
   `crt_gui_apply_snapshot(ptr, len)` with a new envelope. Do not
   hold the session lock while calling into Swift.
4. Snapshot grows `selected_path` and optional `diff` (path, binary
   flag, flattened lines with kind and content). No comments.
5. Swift copies bytes immediately, updates an observable store on
   the main queue, and shows a sidebar list plus a read-only diff.
6. Initial snapshot includes the file `start_review` already
   selected (first file) and its diff when present.

## Out of scope

- Approve, comments, keymaps, search.
- Full `AppModel` as the snapshot.
- `.app` packaging.

## Acceptance Criteria

- [x] `crt gui <BASE>` shows the file list; clicking a file shows
      that file's unified diff.
- [x] Closing the window still shuts down the client.
- [x] `cargo test` does not open a window.
