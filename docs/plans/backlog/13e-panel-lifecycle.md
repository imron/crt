# Stage 13e: Comments Panel and Comment Lifecycle

## Status: Not Started

## Order

5 of 6

## Depends On

- 13d (comments visible in model + basic display)
- 13b (edit re-uses composer)

## Goal

Add a togglable comments panel that lists comments for the current file or scope, shows resolved comments collapsed, supports navigation to a comment's location, and provides the remaining lifecycle operations: resolve, unresolve, edit, delete, and next/previous comment navigation using { / }.

## Why

Users and agents need to see all outstanding (and historical) feedback, move between comments, and manage their state without relying solely on inline blocks.

## Requirements

1. A panel (key `3` or command) can be opened that shows comments.

2. Unresolved comments are shown fully; resolved comments are shown collapsed (header + preview). Enter on a collapsed one expands it.

3. The panel allows filtering by file and basic navigation to the comment's location in the diff.

4. From the panel or when a comment is the current focus, the user can:
   - resolve / unresolve
   - edit (re-open composer with existing body)
   - delete (with confirmation)

5. { moves to previous comment in the current file's diff; } moves to the next.

6. Changes made in the panel or via lifecycle keys are sent to the server and reflected (via live update or refresh).

7. The panel and nav work for both unresolved and resolved comments.

## Acceptance Criteria

- [ ] `3` (or equivalent) opens/closes the comments panel.
- [ ] Resolved comments appear collapsed; Enter expands them.
- [ ] Navigation from panel jumps the cursor to the comment location.
- [ ] Resolve, unresolve, edit, and delete work and persist.
- [ ] { / } cycle through comments in the current file.
- [ ] Live updates from other clients appear in the panel.

## Implementation Notes

- Follow existing overlay patterns for the panel UI.
- Use { and } exactly (no composites) for next/prev comment.
- Lifecycle actions can be available both in panel and via direct keys when appropriate.

## Depends On / Enables

- Depends on prior display and creation slices.
- 13f will add final live-update polish and full test coverage.

## Progress

- (To be filled)
