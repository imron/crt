# Stage 13b: Comment Composer and Creation

## Status: Not Started

## Order

2 of 6

## Depends On

- 13a (visual selection + capture of attachment data)

## Goal

Provide a multi-line text input for the body of a review comment, support submission that creates the comment using data captured by 13a, support cancellation, and support escalation to the user's $EDITOR for long comments.

## Why

Stage 13 requires an input area after selection, Ctrl-Enter (or equivalent) to submit, Escape to cancel, and a key to open $EDITOR. This slice turns a captured selection + body into a real persisted comment via the existing client/server.

## Requirements

1. A multi-line input area (bottom of screen or overlay) appears after a selection is committed (from 13a).

2. The input supports multi-line editing, submission, cancel, and editor escalation.

3. Submission creates the comment using the previously captured location data plus the entered body.

4. On successful creation the selection is cleared, the comment is persisted, and the UI reflects the new comment (via later display slices or live update).

5. Cancellation discards the body and returns to normal mode (selection may also be cleared).

6. A dedicated key inside the composer (e.g. Ctrl-e) writes current text to a temp file, launches $EDITOR, and replaces the composer content with the edited result on exit.

7. Basic status / error feedback is shown for create success or failure.

8. Unit and interaction tests cover submit, cancel, editor round-trip, and error paths.

## Acceptance Criteria

- [ ] After visual selection + Enter, a multi-line input area is presented.
- [ ] Text can be entered over multiple lines.
- [ ] Submit (Ctrl-Enter or designated key) creates the comment and clears the selection/input.
- [ ] Escape cancels the composer without creating a comment.
- [ ] Editor escalation key works and updated text is used on submit.
- [ ] Comments created this way are stored and retrievable via existing list/get APIs.
- [ ] No changes to display of comments (deferred).

## Implementation Notes

- Focus on the input + create flow. Display, panel, and re-anchoring are separate slices.
- Use existing prompt/submit patterns where they fit; a dedicated composer mode is acceptable.
- Keybindings inside the composer must not conflict with global bindings.

## Depends On / Enables

- 13a for the selection data.
- Enables later slices that want to show newly created comments.

## Progress

- (To be filled)
