# Stage 13d: Gutter Markers for Comments

## Status: In Progress

## Order

4 of 6

## Depends On

- 13c (real AnchorStatus values)
- Basic comment data in the app model (minimal load path)

## Goal

Show markers in the diff gutter for lines that have comments. Markers must
clearly indicate single-line comments, multiline comment ranges, and nested
comments. Markers are always visible when comments exist for the file. This
supports a panel-first comments UX without requiring inline comment blocks
or changes to line rendering.

## Why

Gutter markers give a lightweight, always-on indication that comments exist
on specific lines. They allow the user (and future panel navigation) to see
at a glance where feedback is attached, even when the comments panel is
closed and without inserting comment text into the diff itself.

## Requirements

1. Gutter markers are shown for every line that has one or more attached
   comments.

2. Marker style:
   - Unresolved single-line: `●`
   - Resolved single-line: `○`
   - Multiline unresolved: `●` on the first line of the range, `┃` on
     continuation lines, `●` on the last line of the range.
   - Resolved multiline uses the same connected style with `○` instead
     of `●`.

3. Nested comments are indicated by additional markers in the gutter
   (extra `●` or `○` as appropriate).

4. Markers are visible whenever comments exist for the file. Their
   visibility is independent of whether the comments panel is open.

5. Gutter marker display does not depend on inline comment blocks or any
   virtual row / LineView abstraction.

6. Basic tests cover presence of markers, multiline ranges, and nested cases.

## Acceptance Criteria

- [ ] Lines with comments show `●` (unresolved) or `○` (resolved) in
      the gutter.
- [ ] Multiline comments use the connected `●` / `┃` / `●` (or `○` /
      `┃` / `○`) style.
- [ ] Nested comments produce multiple markers on the relevant lines.
- [ ] Markers remain visible when the comments panel is closed.
- [ ] No dependency on inline block rendering or virtual line changes.

## Implementation Notes

- Keep the gutter marker logic simple and independent of any future
  inline comment display.
- The exact gutter column width and how markers are combined with blame /
  line numbers can be decided during implementation.
- The multiline/nested marker style should match the example in the
  parent plan.

## Depends On / Enables

- Depends on 13c for AnchorStatus.
- Enables 13e (the comments panel relies on gutter markers to know which
  lines have comments).

## Progress

- Added app-owned comment loading for the current review scope, including
  resolved comments.
- App model now projects current-file comment attachments for renderers.
- Diff cache invalidates when comment attachment state changes.
- Added gutter marker generation for single-line, multiline, resolved, and
  nested comment ranges.
- Rendered markers in inline diff, full-file HEAD/base, and side-by-side
  diff modes without introducing inline comment blocks or virtual rows.
- CommentChanged notifications now refresh cached comments.
- Added tests for marker generation, app-model comment projection, and
  comment notification reload classification.
- Collapsed nested markers to a single gutter column. Multiline boundaries
  win over nested single-line markers so resolved range openings remain
  hollow when appropriate.
- Fixed rendered-text extraction paths to convert character columns to byte
  indexes before slicing, avoiding crashes on multibyte marker glyphs.
- Verified with `cargo test`.
