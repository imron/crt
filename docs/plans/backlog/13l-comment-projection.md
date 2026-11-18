# Comment Projection

## Status

Completed.

## Goal

Introduce a single app-owned `CommentProjection` abstraction for all comment
lookup, marker, and navigation coordinate mapping in the active view.

## Why

Comments have one logical identity, but they can appear in different view
coordinate systems:

- Full-file views use one source-line axis.
- Side-by-side diffs use separate base and head side/line cells.
- Inline diffs use rendered diff rows that can contain base-only, head-only,
  or shared context lines.

When each feature reconstructs that mapping independently, base and head line
numbers can be mixed incorrectly. That causes bugs such as inline markers
appearing on the wrong same-numbered head line, the comments panel missing a
comment on a deletion row, or `{`/`}` navigation selecting the same comment
again instead of advancing.

## Requirements

1. Add an app-owned `CommentProjection` built from:
   - comments for the active file or scope,
   - the active content mode,
   - the active render variant,
   - the app-owned diff/full-file row model,
   - the current cursor row,
   - the selected comment id.

2. Keep the projection UI-agnostic. It must not depend on ratatui, terminal
   cell geometry, styles, or renderer state.

3. Expose query methods for the app and TUI model, such as:
   - `logical_range(comment_id)`,
   - `current_comment_id()`,
   - `comment_at_cursor()`,
   - `marker_for_inline_row(row)`,
   - `marker_for_side_line(side, line)`,
   - `display_rows_for_comment(comment_id)`,
   - `next_comment_from_cursor(direction)`.

4. Use logical comment identity for comments panel state and current-comment
   commands. Most app code should not care whether the cursor is on the base
   or head segment of a compound comment.

5. Use view-specific coordinates only at the projection boundary:
   - inline diff marker rendering uses rendered row indices,
   - side-by-side marker rendering uses side and source line,
   - full-file rendering uses source line,
   - navigation asks the projection for the next logical comment target.

6. Remove duplicate ad hoc helpers that compute comment line ranges or
   current-comment precedence in separate app modules.

7. Keep the TUI thin. It should render the projected markers and comment
   panel model it receives, without reconstructing comment semantics from
   base/head line numbers.

## Acceptance Criteria

- [x] Comment lookup for commands and the comments panel goes through
      `CommentProjection`.
- [x] Inline diff markers are keyed by rendered row, not by a merged
      base/head line number range.
- [x] Side-by-side diff markers remain side-aware.
- [x] `{` and `}` navigation uses projection targets and works from
      deletion rows, addition rows, context rows, and full-file rows.
- [x] Multiple comments with overlapping or identical starts preserve the
      current selected-comment tie breaker.
- [x] Tests cover inline, side-by-side, full-file, compound base/head
      comments, deletion-row cursor lookup, and same-start navigation.
- [x] The TUI renderer contains no comment range precedence logic.

## Implementation Notes

- This is a structural cleanup and hardening step, not a visual redesign.
- The first implementation can wrap the existing marker candidate logic, but
  callers should see one projection API.
- Prefer moving duplicated helpers into the projection instead of adding new
  per-module helper functions.
