# Stage 13a: Visual Selection and Anchor Capture for Comments

## Status: Complete

## Order

1 of 6 (recommended implementation order for stage 13)

## Depends On

- Stage 12 (polish elements already present: column cursor, TextAnchor support)
- Existing diff hunks, AppModel, TextAnchor, and cursor infrastructure

## Goal

Add support for visual line selection in the diff pane and the ability to
capture the selection details (file, line range, anchor text, surrounding
context) so that it can be used as the attachment point for a new review
comment.

## Why

The stage 13 spec requires users to be able to select a location in a diff
before creating a comment. This slice provides the input mechanism and the
capture of the data needed for comment creation (anchor_text,
context_before, context_after, and line ranges) without yet building the
composer or persistence flow.

This is the foundation for "Selection and Comment Creation" requirements in
the parent plan.

## Requirements

1. Introduce visual line selection state (start and end anchors) that is
   active only in the diff pane.

2. Add key handling so that:
   - `v` (no modifiers) enters line-based visual selection mode from the
     current cursor position.
   - `V` (shifted `v`) is accepted as the same line selection mode.
   - Movement keys (j/k, arrows, h/l when applicable) extend the selection
     end point by line.
   - The selection is visually highlighted in the diff view (distinct style
     for the range).

3. `Escape` while in a visual selection mode cancels the selection and
   returns to normal mode without creating anything.

4. When Enter is pressed while a visual selection is active, capture the
   selection details for later use as a comment attachment point:
   - Current file path.
   - Line start / line end.
   - Character start / character end set to None.
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

- [x] `v` enters line selection; `j`/`k` extend the highlighted range.
- [x] `V` enters the same line selection mode.
- [x] `Escape` in visual mode clears the selection and returns to normal
      input without side effects.
- [x] Enter with an active selection produces a complete capture of file,
      ranges, anchor_text, and context strings.
- [x] Capture works for line ranges and stores character fields as None.
- [x] No changes to comment persistence or UI creation flow (deferred to
      later slices).
- [x] Existing cursor, search, and review navigation behavior is unaffected
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

- Added core visual-selection effects for line mode, with active-mode
  routing for movement, Escape, and Enter.
- Added app-owned visual-selection state plus a committed comment-anchor
  capture shaped for later `CreateCommentParams` use.
- Added diff-pane highlighting for active visual selections using the
  existing selection style configuration.
- Added unit coverage for key routing, cancellation, and line capture.
