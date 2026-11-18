# Stage 35: GUI Review File List

## Status

Implemented on `wip/gui`.

## Goal

`crt gui` starts a review the same way the TUI does, and the Swift
window shows the file list from that session.

## Why

Stage 34 only connects. The next unknown is putting server-owned
review state on the JSON snapshot: `init`, `list_file_statuses`, a
nested `files` array, and Swift decoding it. Actions, diffs, and
keymaps stay out.

## Requirements

1. `crt gui <BASE>` and `crt gui --root` accept the same review
   args as `crt <BASE>` / `crt --root`, including `--standalone`
   and `--reset`. Bare `crt gui` errors like bare `crt` (base or
   `--root` required).
2. Rust calls `App::start_review`. On `ReviewStartup::Reset`, print
   the TUI reset summary and do not open a window. On
   `ReviewStartup::Review`, keep `App` alive for the window
   lifetime and shut it down when the window closes.
3. Extend the snapshot envelope (still `{"data": ...}`) with
   merge-base and a `files` array. Each file is path, change kind,
   and review status. Do not include hunks or file content.
4. Swift copies the blob as today, decodes `files`, and shows a
   simple list (path plus status). Connection fields from stage 34
   stay.
5. Tests parse `crt gui main` / `crt gui --root` and still do not
   open a window.

## Out of scope

- Selecting a file, loading a diff, or sending actions.
- Keymaps, comments panel, `AppModel` as the snapshot.
- `.app` packaging / Finder launch.

## Acceptance Criteria

- [x] `cargo run -- gui main` in this repo opens the window with
      the changed-file list for that review.
- [x] `crt gui` without a base or `--root` errors.
- [x] `--reset` prints the reset summary and exits 0 without a
      window.
- [x] `cargo test` does not open a window.
