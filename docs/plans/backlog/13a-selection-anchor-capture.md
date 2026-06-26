# Stage 13a: Visual Selection and Anchor Capture for Comments

## Status: Not Started

## Order

1 of 6 (recommended implementation order for stage 13)

## Depends On

- Stage 12 (polish elements already present: column cursor, TextAnchor support)
- Existing diff hunks, AppModel, TextAnchor, and cursor infrastructure

## Goal

Add support for visual selection modes in the diff pane (V for line ranges,
v for character ranges) and the ability to capture the selection details
(file, line/character range, anchor text, surrounding context) so that it
can be used as the attachment point for a new review comment.

## Why

The stage 13 spec requires users to be able to select a precise location in
a diff (lines or characters) before creating a comment. This slice provides
the input mechanism and the capture of the data needed for comment creation
(anchor_text, context_before, context_after, line/char ranges) without yet
building the composer or persistence flow.

This is the foundation for "Selection and Comment Creation" requirements in
the parent plan.

## Requirements

1. Introduce visual selection state (start and end anchors + mode: line or
   character) that is active only in the diff pane.

2. Add key handling so that:
   - `V` (no modifiers) enters line-based visual selection mode from the
     current cursor position.
   - `v` (no modifiers) enters character-based visual selection mode from
     the current cursor position.
   - In either mode, movement keys (j/k, arrows, h/l when applicable)
     extend the selection end point.
   - The selection is visually highlighted in the diff view (distinct style
     for the range).

3. `Escape` while in a visual selection mode cancels the selection and
   returns to normal mode without creating anything.

4. When Enter is pressed while a visual selection is active, capture the
   selection details for later use as a comment attachment point:
   - Current file path.
   - Line start / line end (for V or v).
   - Character start / character end within the lines (for v mode; None
     for V).
   - The exact anchor_text spanned by the selection.
   - Context before (3-5 lines) and context after (3-5 lines) from the
     diff content.

5. The captured data must be sufficient to populate the fields of
   CreateCommentParams when the comment body is later supplied.

6. Selection state must integrate cleanly with existing TextAnchor, diff
   cursor, and hunk line data already present in the app model.

7. Unit tests cover entering/exiting modes, extending ranges, cancellation,
   and correct extraction of anchor + context data from representative diff
   hunks.

## Acceptance Criteria

- [ ] `V` enters line selection; `j`/`k` extend the highlighted range.
- [ ] `v` enters character selection; appropriate movement extends the
      character range.
- [ ] `Escape` in visual mode clears the selection and returns to normal
      input without side effects.
- [ ] Enter with an active selection produces a complete capture of file,
      ranges, anchor_text, and context strings.
- [ ] Capture works for both line ranges and sub-line character ranges.
- [ ] No changes to comment persistence or UI creation flow (deferred to
      later slices).
- [ ] Existing cursor, search, and review navigation behavior is unaffected
      outside of visual mode.

## Implementation Notes

- Keep this slice focused on selection mechanics and data capture. Do not
  implement the comment input area, server calls, or display of comments.
- Follow the "what and why" style; leave detailed code choices to the
  implementer.
- Keybindings for comment next/prev (} / {) are out of scope for this slice.

## Depends On / Enables

- Enables 13b (composer will consume the captured selection).
- Can be developed in parallel with 13c (re-anchoring).

## Progress

- (To be filled during implementation)
