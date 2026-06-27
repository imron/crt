# Stage 13b: Comment Composer and Creation

## Status: Complete

## Order

2 of 6

## Depends On

- 13a (visual selection + capture of attachment data)

## Goal

Provide a multi-line text input for the body of a review comment, support
submission that creates the comment using data captured by 13a, support
cancellation, and support escalation to the user's $EDITOR for long comments.

## Why

Stage 13 requires an input area after selection, Ctrl-Enter (or equivalent)
to submit, Escape to cancel, and a key to open $EDITOR. This slice turns a
captured selection + body into a real persisted comment via the existing
client/server.

## Requirements

1. A multi-line input area (bottom of screen or overlay) appears after a
   selection is committed (from 13a).

2. The input supports multi-line editing, submission, cancel, and editor
   escalation.

3. Submission creates the comment using the previously captured location
   data plus the entered body.

4. On successful creation the selection is cleared, the comment is persisted,
   and the UI reflects the new comment (via later display slices or live
   update).

5. Cancellation discards the body and returns to normal mode (selection may
   also be cleared).

6. A dedicated key inside the composer (e.g. Ctrl-e) writes current text to
   a temp file, launches $EDITOR, and replaces the composer content with the
   edited result on exit.

7. Basic status / error feedback is shown for create success or failure.

8. Unit and interaction tests cover submit, cancel, editor round-trip, and
   error paths.

## Acceptance Criteria

- [x] After visual selection + Enter, a multi-line input area is presented.
- [x] Text can be entered over multiple lines.
- [x] Submit (Ctrl-Enter or designated key) creates the comment and clears
      the selection/input.
- [x] Escape cancels the composer without creating a comment.
- [x] Editor escalation key works and updated text is used on submit.
- [x] Comments created this way are stored and retrievable via existing
      list/get APIs.
- [x] No changes to display of comments (deferred).

## Implementation Notes

- Focus on the input + create flow. Display, panel, and re-anchoring are
  separate slices.
- Use existing prompt/submit patterns where they fit; a dedicated composer
  mode is acceptable.
- Keybindings inside the composer must not conflict with global bindings.

## Depends On / Enables

- 13a for the selection data.
- Enables later slices that want to show newly created comments.

## Progress

- Added a first-class comment prompt contract in core. Committing a visual
  selection now requests a comment composer, and submitting/canceling the
  prompt emits comment-specific core effects.
- Added a TUI comment composer mode with multi-line editing, `Ctrl-S` submit,
  `Ctrl-Enter` submit where supported, `Escape` cancel, and `Ctrl-E`
  `$EDITOR` escalation.
- Queued comment creation in the app layer using the captured 13a anchor and
  the submitted body, then called the existing `create_comment` client API.
- Added status feedback for empty bodies, cancellation, in-flight creation,
  creation success, and creation failure.
- Added focused core, app, and TUI tests for the comment prompt lifecycle,
  multi-line input, cancellation, editor escalation dispatch, and queued
  create-comment work.
