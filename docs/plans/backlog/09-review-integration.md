# Stage 9: Review State Integration

## Goal

Wire together the review workflow: marking files as reviewed, persisting
that state via the server, detecting changes on re-launch, and
auto-advancing to the next unreviewed file.

## Why

This is the core value proposition of the tool. Without this stage, the TUI
is just a diff viewer. This stage turns it into a review tool — one that
remembers what you've already reviewed, detects when things have changed,
and helps you work through the remaining files efficiently.

All state mutations go through the server API, ensuring consistency across
clients.

## Requirements

1. **Mark reviewed (`r`)**: pressing `r` on the currently selected file:
   - Sends `mark_reviewed(base_ref, file_path)` to the server.
   - The server computes the current diff hash and stores the review.
   - The server responds with the updated file entry.
   - The TUI updates the file's `ReviewStatus` to `Reviewed`.
   - The file moves from the unreviewed section to the reviewed section.
   - The cursor auto-advances to the next unreviewed file.

2. **Auto-advance behavior**: after marking a file reviewed:
   - If unreviewed files remain below the current position, go to the
     next one.
   - If the current file was the last unreviewed, go to the first
     unreviewed (if any remain).
   - If no unreviewed files remain, stay at the current position in the
     reviewed section.

3. **Startup reconciliation**: when the TUI connects to the server and
   requests `list_changed_files`, the server:
   - Loads stored reviews for the current `(base_ref, head_ref)`.
   - Computes current diff hashes for all changed files.
   - Returns each file with the correct status:
     - No stored review → `Unreviewed`.
     - Stored review, hash matches → `Reviewed`.
     - Stored review, hash differs → `Changed`.
   - Files previously reviewed but no longer in the diff are left in the
     database (harmless, may be relevant after future rebases).

4. **Un-reviewing**: pressing `r` on a `Reviewed` file should toggle it
   back to `Unreviewed` by sending `unmark_reviewed` to the server.

5. **`--reset` implementation**: `crt <base> --reset` connects to the
   server, sends `reset_reviews(base_ref)`, prints a confirmation, and
   exits.

6. **Review progress indicator**: visible somewhere in the UI (e.g. in
   the file list header or status bar). Shows `3/10 files reviewed`.

7. **Live updates**: if another client (e.g. the MCP adapter) changes
   review state, the server pushes a notification. The TUI updates its
   state and re-renders.

## Acceptance Criteria

- [ ] Pressing `r` marks the file as reviewed and the change persists
      across application restarts.
- [ ] After pressing `r`, the cursor auto-advances to the next unreviewed
      file.
- [ ] Pressing `r` on a reviewed file un-reviews it.
- [ ] On restart, previously reviewed files with unchanged diffs appear
      in the reviewed section.
- [ ] On restart, previously reviewed files with changed diffs appear in
      the unreviewed section with the `~` marker.
- [ ] `--reset` clears all review state for the `(base_ref, head_ref)`
      pair and confirms.
- [ ] Review progress is visible in the UI.
- [ ] Changes from another client (via server notification) are reflected
      in the TUI without restarting.
- [ ] The workflow feels smooth: launch → review → quit → rebase →
      relaunch → only changed files need re-review.

## Open Questions

- Should `r` on a `Changed` file mark it as reviewed (with the new hash),
  or should the user be required to view the diff first? Marking without
  viewing is faster but risks missing changes.
- Should there be a "mark all as reviewed" shortcut for quick bulk
  operations?
- Should the TUI auto-refresh if it detects that HEAD has changed (e.g.
  after a rebase in another terminal)?
