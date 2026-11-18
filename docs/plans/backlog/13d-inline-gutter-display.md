# Stage 13d: Inline Comment Blocks, Gutter Markers, and c Toggle

## Status: Not Started

## Order

4 of 6

## Depends On

- 13c (real AnchorStatus values)
- Plan 17 (Visual Line Abstraction) – `LineView` / `LineType` model
- Basic comment data in the app model (minimal load path)

## Goal

Store and project loaded comments into the app model, make the existing
`c` toggle actually control visibility, render unresolved comment blocks
inline below the lines they are attached to, and show gutter markers
(● unresolved, ○ resolved).

## Why

The stage 13 spec requires visual feedback for comments directly in the
diff view (inline blocks when visible, gutter indicators always useful,
toggle via `c`).

## Requirements

1. Comments for the current scope (or at least the selected file) are
   available in AppState / AppModel.

2. The `c` / "nocomments" command actually toggles a show_comments flag
   that affects rendering.

3. When show_comments is true, unresolved comments appear as visually
   distinct inline blocks below their attachment lines in both inline
   and side-by-side views.

4. The gutter marker column is always visible when any comments exist on
   the file, regardless of the `show_comments` state. Markers use the
   following style:
   - Single-line comment: `●` (or `○` if resolved)
   - Multiline comment: `●` at the top, `┃` for continuation lines,
     `●` at the bottom (same pattern for resolved comments using `○`)
   - Nested comments inside a block are indicated by additional
     `●` (or `○`) markers

   Example (multiline comment with a nested single-line comment on the
   second line):

   ```
   ●  fn foo() {
   ●      bar();
   ┃  }
   ●
   ```

   The same gutter pattern can represent multiple comment structures
   (e.g. outer block + inner single comment, multiple overlapping blocks,
   etc.). The renderer does not attempt to disambiguate; the actual
   structure is stored in the comment data.

5. Orphaned and resolved comments are rendered appropriately (resolved
   may be hidden from inline per spec).

6. The rendering reuses existing diff line / hunk infrastructure.

7. Basic tests for toggle and presence of markers/blocks.

## Acceptance Criteria

- [ ] `c` toggles inline comment visibility.
- [ ] Unresolved comments appear as inline blocks when visible.
- [ ] Gutter indicators show ● (unresolved) and ○ (resolved).
- [ ] Markers remain visible when scrolling past the block.
- [ ] Toggle state survives file changes within a review where sensible.

## Implementation Notes

- Decide exact block style and placement (below the line range, using
  borders etc.) while matching the spirit of the example in the spec.
- Gutter column decision (always show marker, or only when comments exist)
  can be made here.
- Keep display concerns separate from creation and panel (other slices).

## Depends On / Enables

- Depends on 13c for statuses.
- Enables 13e (panel will reference the same comment data).

## Progress

- (To be filled)
