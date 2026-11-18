# Stage 13e: Comments Panel and Comment Lifecycle

## Status: Not Started

## Order

5 of 6

## Depends On

- 13d (comments visible in model + basic display)
- 13b (edit re-uses composer)

## Goal

Add a togglable comments panel that lists comments for the current file or
scope, shows resolved comments collapsed, supports navigation to a comment's
location, and provides the remaining lifecycle operations: resolve,
unresolve, edit, delete, and next/previous comment navigation using { / }.

## Why

Users and agents need to see all outstanding (and historical) feedback, move
between comments, and manage their state without relying solely on inline
blocks.

## Requirements

1. `Shift-C` toggles a comments panel at the bottom of the diff view.

2. Gutter markers remain visible whether the panel is open or closed.

3. Unresolved comments are shown fully; resolved comments are shown collapsed
   (header + preview). Enter on a collapsed one expands it.

4. The panel allows filtering by file and basic navigation to the comment's
   location in the diff.

5. When the cursor is on a line with a comment marker, the corresponding
   comment(s) are shown (fully expanded) in the panel.

6. From the panel or when a comment is the current focus, the user can:
   - resolve / unresolve
   - edit (re-open composer with existing body)
   - delete (with confirmation)

7. { moves to previous comment in the current file's diff; } moves to the next.

8. Changes made in the panel or via lifecycle keys are sent to the server and
   reflected (via live update or refresh).

9. The panel and nav work for both unresolved and resolved comments.

10. `Tab` cycles focus between visible panes/panels, including the comments
    panel when it is open. Do not add a `3` binding for panel focus.

11. `e` edits the currently focused/current-cursor comment. If there is no
    comment at the current cursor, `e` does nothing.

12. `c` opens the comment editor for the current comment context when one
    exists; otherwise in the diff view it creates a new comment on the
    current line.

## Acceptance Criteria

- [ ] `Shift-C` toggles the comments panel at the bottom of the diff view.
- [ ] When panel is open, inline blocks are hidden (gutter markers remain).
- [ ] Cursor on a line with a comment marker shows the comment in the panel.
- [ ] Resolved comments appear collapsed; Enter expands them.
- [ ] Navigation from panel jumps the cursor to the comment location.
- [ ] Resolve, unresolve, edit, and delete work and persist.
- [ ] { / } cycle through comments in the current file.
- [ ] `Tab` cycles focus to the comments panel when it is open.
- [ ] `e` edits the current comment and does nothing when no comment exists.
- [ ] `c` opens the comment editor for the current comment context or creates
      a new current-line comment from the diff view.
- [ ] Live updates from other clients appear in the panel.

## Implementation Notes

- Follow existing overlay patterns for the panel UI.
- Use { and } exactly (no composites) for next/prev comment.
- Lifecycle actions can be available both in panel and via direct keys when
  appropriate.

## Depends On / Enables

- Depends on prior display and creation slices.
- 13f will add final live-update polish and full test coverage.

## Progress

- (To be filled)
