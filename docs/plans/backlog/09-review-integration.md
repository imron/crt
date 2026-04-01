# Stage 9: Review State Integration

## Status: Complete

## Goal

Wire together the review workflow: marking files as reviewed, persisting
that state via the server, detecting changes on re-launch, and
auto-advancing to the next unreviewed file. Also: resizable file list,
section-scoped navigation, server push notifications, and config
persistence.

## What Was Implemented

### Review Workflow

1. **Mark reviewed (`r`)**: pressing `r` on the currently selected file
   sends `mark_reviewed(file_path)` to the server. The server computes
   the current diff hash, stores the review in the database, broadcasts
   a notification, and responds with the updated status. The TUI updates
   the file entry, re-sorts the list, and auto-advances.

2. **Un-review (`r` on reviewed file)**: pressing `r` on a reviewed file
   sends `unmark_reviewed(file_path)`. The file moves back to the
   unreviewed section. Cursor stays on the same file.

3. **Auto-advance behavior**: after marking reviewed, the cursor moves to
   the next unreviewed file *after* the current position. Only if there
   are no more unreviewed files below does it wrap to the first
   unreviewed file. If all files are reviewed, cursor stays on the file
   just marked.

4. **Startup reconciliation**: `list_changed_files` compares stored
   review diff hashes against current diffs. Files are classified as
   Unreviewed, Reviewed (hash matches), or Changed (hash differs).

5. **`--reset` implementation**: `crt <base> --reset` calls
   `reset_reviews()` on the server, prints the count of cleared reviews,
   and exits.

6. **Review progress**: status bar shows `X/Y reviewed` alongside the
   ref chain.

### Server Handlers

7. **`mark_reviewed`**: computes diff hash, stores review in DB with
   timestamp, broadcasts notification, returns updated status.

8. **`unmark_reviewed`**: removes review from DB, broadcasts
   notification, returns unreviewed status.

9. **`reset_reviews`**: clears all reviews for the current
   `(merge_base, head_ref)` scope, broadcasts notification, returns
   count of cleared reviews.

### Server Push Notifications

10. **Broadcast channel**: the server maintains a `tokio::sync::broadcast`
    channel. Review mutations send notifications on it.

11. **Connection forwarder**: each client connection spawns a task that
    subscribes to the broadcast channel and writes JSON-RPC notifications
    (method `"notification"`, no `id`) to the client.

12. **Client-side handling**: when the client reads a response and
    encounters a notification (has `method` but no `id`), it routes it to
    an internal channel. `drain_notifications()` returns all pending
    notifications.

13. **TUI notification processing**: each event loop iteration checks for
    notifications. Review-related notifications trigger a full file list
    reload from the server, preserving selection by path.

### Async Architecture

14. **Fully async event loop**: the event loop uses
    `tokio::task::spawn_blocking` to poll crossterm events on a dedicated
    thread, sending them via `tokio::sync::mpsc::unbounded_channel`. The
    main loop `await`s the channel receiver. All server calls (`await`
    client methods) happen naturally in async context — no
    `block_in_place` or runtime bridging needed.

15. **Event draining**: after processing the first event, all additional
    queued events are drained via `try_recv()` before re-rendering. This
    makes mouse drag and rapid key repeat feel instant.

16. **Review toggle**: the `r` key handler sets
    `state.pending_review_toggle = true`. The async event loop picks this
    up and `await`s the client call directly.

### Section-Scoped Navigation

17. **File list focus**: `Ctrl-n` / `Ctrl-p` cycles within the current
    section (unreviewed or reviewed) based on where the cursor is.

18. **Diff pane focus**: `Ctrl-n` / `Ctrl-p` cycles only through
    unreviewed files (falls back to all files if none unreviewed).

### Resizable File List Pane

19. **Configurable width**: `file_list_width` in `[layout]` config
    section (default 40). Replaces the hardcoded `FILE_LIST_MAX_WIDTH`
    constant.

20. **Drag-to-resize**: clicking on the border between file list and diff
    pane starts a drag. Dragging updates the width live (clamped min 10,
    max terminal-20). On mouse-up, the new width is persisted to
    `~/.crt/config.toml`.

### File List Improvements

21. **Mouse click to select**: clicking a file in the file list selects
    it and loads its diff. Clicking section headers is ignored.

22. **Long file name truncation**: paths that exceed the pane width are
    truncated from the left with `…` prefix, keeping the filename
    visible. E.g. `…local/0003_tasks.up.sql`. No slash after the
    ellipsis (avoids confusion with `../`).

23. **Row-to-file mapping**: built during render for mouse click
    resolution. Maps each display row to `Some(file_index)` or `None`
    for headers.

### Configuration System

24. **TOML config file**: `~/.crt/config.toml` with `[style]` and
    `[layout]` sections. Fully optional — all fields have defaults.

25. **`config::Color` newtype**: wraps `ratatui::style::Color` with
    native `Deserialize`. Supports `"#RRGGBB"`, named colors, 256-color
    indices.

26. **`StyleConfig`** with sections: `diff`, `files`, `panel`, `status`,
    `help`, `selection` — covering every color in the UI.

27. **`LayoutConfig`**: `file_list_width` and `diff_algorithm`. Persisted
    on drag-resize and algorithm change.

28. **`save_layout()`**: surgically updates the `[layout]` section of the
    config file without disturbing other sections.

## Acceptance Criteria

- [x] Pressing `r` marks the file as reviewed and persists across
      restarts.
- [x] After pressing `r`, cursor auto-advances to next unreviewed file
      after current position.
- [x] Pressing `r` on a reviewed file un-reviews it.
- [x] On restart, reviewed files with unchanged diffs appear reviewed.
- [x] On restart, reviewed files with changed diffs appear as `Changed`.
- [x] `--reset` clears all review state and confirms with count.
- [x] Review progress visible in status bar.
- [x] Server push notifications update TUI state without restart.
- [x] `Ctrl-n`/`Ctrl-p` scoped to current section in file list.
- [x] `Ctrl-n`/`Ctrl-p` cycles only unreviewed from diff pane.
- [x] File list pane width configurable and drag-resizable.
- [x] Long file names truncated with `…` prefix.
- [x] Mouse click selects files in file list.
- [x] All colors configurable via `~/.crt/config.toml`.
- [x] Event loop is fully async with no sync/async bridging hacks.
- [x] Diff algorithm configurable (`d` key, git config fallback).
- [x] Ignore-whitespace toggle (`w` key).
- [x] Blame annotations toggle (`b` key).
- [x] Double-click word selection with auto-clipboard.

## Open Questions (Resolved)

- **`r` on `Changed` file**: currently marks it as reviewed with the new
  hash. User is expected to have viewed the diff first.
- **"Mark all as reviewed"**: not implemented. Could be added as a
  shortcut in a future stage.
- **Auto-refresh on HEAD change**: not implemented. Would need
  filesystem watching or polling.

## Open Questions (New)

- Hunk-level review approval: individual hunks within a file could be
  approved independently. If approved hunks haven't changed since
  approval, they could be hidden from review. This was discussed but
  deferred — needs its own design document.
