# Visual Line Abstraction

## Status: Abandoned

## Goal

This plan is abandoned. Inline comment rows are no longer part of the Stage 13
comment workflow, so the diff view does not need a visual-row abstraction for
comment display.

## Why

The comments panel and the file panel's `Unresolved Comments` section cover
the review workflows that inline rows were intended to support. Keeping the
diff renderer simple is preferable until another concrete feature needs
virtual rows.

## Previous Goal

Introduce a `LineType` enum and a minimal `LineView` abstraction so that the
diff view can support different kinds of visual rows while keeping cursor
navigation, scrolling, and rendering logic simple and consistent.

## Design Decisions

- `LineType` variants (`Source`, `Addition`, `Removal`, `Comment`)
  are peers; none is a subtype of another.
- The abstraction begins with a strict 1:1 mapping so that all existing
  diff view code continues to work unchanged.
- The interface is deliberately minimal. Future plans may extend
  `LineView` with additional methods once concrete needs arise.
- A permanent debug feature ("double line spacing") is provided from
  the start. Pressing `Shift-D` toggles an alternate `LineView`
  implementation that inserts an extra blank visual row after every
  logical line. This exercises the abstraction continuously and serves
  as a visual and integration test.

## Requirements

1. Define a `LineType` enum with the following variants as peers:
   - `Source`
   - `Addition`
   - `Removal`
   - `Comment`

2. Introduce a `LineView` trait (or struct) that provides:
   - `logical_height() -> usize` — number of source diff lines.
   - `visual_height() -> usize` — number of rows that will actually be
     rendered (initially identical to `logical_height`).
   - `logical_to_visual(logical: usize) -> Vec<usize>` — maps a logical
     source line to zero or more visual row indices.
   - `visual_to_logical(visual: usize) -> usize` — maps a visual row
     back to its logical source line.
   - `line_type(visual: usize) -> LineType` — returns the kind of row
     that will be rendered at that visual position.

   These five methods are considered sufficient for the initial
   1:1 implementation and for the first extension that will insert
   additional rows.

3. Provide a default 1:1 implementation of `LineView` that maps each
   logical line directly to one visual row with its original `LineType`.

4. The abstraction must be usable by cursor movement, scrolling, and
   viewport slicing with no knowledge of comments or other virtual rows.

5. The initial implementation must introduce no observable change in
   behavior for existing diff views when the double-spacing mode is off.

6. A `Shift-D` keybinding must toggle double line spacing at runtime
   in debug builds (or via a compile-time feature flag). The toggle must
   affect only the visual presentation and must not alter the underlying
   logical diff data.

## Acceptance Criteria

- [ ] `LineType` enum exists with `Source`, `Addition`, `Removal`, and
      `Comment` as peers.
- [ ] `LineView` trait (or struct) is defined with the methods listed above.
- [ ] A 1:1 implementation is provided and used by the diff view.
- [ ] `Shift-D` toggles a double-spaced `LineView` implementation.
- [ ] No existing cursor, scroll, or rendering behavior changes when
      double spacing is off.
- [ ] All lines in the plan file are ≤ 80 columns.

## Implementation Notes

- Keep the abstraction deliberately minimal. Do not add comment-specific
  rendering logic in this plan.
- The 1:1 implementation can be a simple identity mapping.
- A second `LineView` implementation (`DoubleSpacedView`) must be
  provided. It returns `visual_height() == logical_height() * 2` and
  maps each logical line to two consecutive visual rows (the second
  being empty). `Shift-D` toggles between the normal and double-spaced
  views.

## Depends On / Enables

- Enables Stage 13 (all sub-plans) by providing a stable row model.

## Progress

- Abandoned after the Stage 13 workflow settled on panel-based comment display
  and file-panel unresolved comment scanning.
