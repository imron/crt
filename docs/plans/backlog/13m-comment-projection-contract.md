# Comment Projection Contract

## Status

Backlog.

## Goal

Replace the transitional `CommentProjection` shape with a final app-owned
projection contract that is built once from explicit app model inputs and used
for all comment lookup, markers, navigation targets, and display spans.

## Why

The current `CommentProjection` centralizes a lot of previously duplicated
logic, but it still has several weaknesses:

- It is part projection object and part static utility namespace.
- Some projection methods take full `AppState`, hiding their actual inputs.
- Some query paths rebuild diff rows instead of using app-owned row models.
- The renderer-facing `CommentMarkerSet` alias keeps the old marker-only
  concept alive.
- Navigation still owns some ordering and target-selection logic outside the
  projection.
- Logical min/max ranges are used for several meanings without naming which
  meaning is intended.

The final abstraction should make coordinate systems explicit and make invalid
queries difficult to express.

## Requirements

1. Build `CommentProjection` once from explicit inputs:
   - comments for the active file or scope,
   - active content mode,
   - active render variant,
   - app-owned inline, side-by-side, or full-file row model,
   - current cursor row,
   - selected comment id.

2. Do not pass `AppState` into projection query methods. Projection
   construction may happen in app model/update code, but all query methods
   should operate on projected data already stored in the projection.

3. Model projection mode explicitly with Rust types. The shape should
   distinguish:
   - full-file source-line projection,
   - side-by-side side/source-line projection,
   - inline rendered-row projection.

4. Remove `CommentMarkerSet` as a public alias. Renderer-facing fields and
   APIs should use `CommentProjection` or a smaller projected marker type by
   name.

5. Store projected comment records inside the projection. Each record should
   include:
   - comment id,
   - resolved status,
   - anchor status,
   - logical ordering range,
   - view-specific display rows or side ranges,
   - marker candidates,
   - whether it is selected/current in this projection.

6. Give range concepts explicit names. Avoid a generic `logical_range` when
   different callers mean different things. Use names such as:
   - `ordering_range`,
   - `panel_range`,
   - `display_span`,
   - `side_display_range`.

7. Move current-comment selection fully into the projection. The projection
   should expose:
   - `current_comment_id()`,
   - `comment_at_cursor()`,
   - `comment_for_id(id)`,
   - selected-comment tie-breaking for overlaps and identical starts.

8. Move navigation target selection fully into the projection. The projection
   should expose:
   - `next_comment_from_cursor(direction)`,
   - `next_unresolved_from_cursor(direction)`,
   - `display_target(comment_id)`.

9. Keep app update code responsible for state transitions only. Update code
   may decide what to do with a projected target, but it should not recompute
   comment precedence, display spans, or cursor-to-comment mapping.

10. Keep TUI rendering thin. TUI code may ask for a marker at a rendered
    coordinate, but must not reconstruct comment range precedence or convert
    between base/head/source/row coordinate systems.

## Proposed Shape

The exact API can evolve during implementation, but the intended shape is:

```rust
pub enum CommentProjection {
    FullFile(FullFileCommentProjection),
    SideBySide(SideBySideCommentProjection),
    Inline(InlineCommentProjection),
}

pub struct ProjectedComment {
    pub id: i64,
    pub resolved: bool,
    pub anchor_status: AnchorStatus,
    pub ordering_range: CommentOrderingRange,
    pub display_span: Option<CommentDisplaySpan>,
    pub selected: bool,
    pub current: bool,
}

pub enum CommentDisplaySpan {
    FullFile { start_row: usize, end_row: usize },
    SideBySide { start_row: usize, end_row: usize },
    Inline { start_row: usize, end_row: usize },
}

impl CommentProjection {
    pub fn current_comment_id(&self) -> Option<i64>;
    pub fn comment_at_cursor(&self) -> Option<&ProjectedComment>;
    pub fn comment_for_id(&self, id: i64) -> Option<&ProjectedComment>;
    pub fn marker_for_full_file_line(&self, line: u32) -> CommentMarker;
    pub fn marker_for_side_line(
        &self,
        side: CommentAnchorSide,
        line: u32,
    ) -> CommentMarker;
    pub fn marker_for_inline_row(&self, row: usize) -> CommentMarker;
    pub fn display_target(&self, id: i64) -> Option<CommentDisplaySpan>;
    pub fn next_comment_from_cursor(
        &self,
        direction: Direction,
    ) -> Option<ProjectedCommentTarget>;
    pub fn next_unresolved_from_cursor(
        &self,
        direction: Direction,
    ) -> Option<ProjectedCommentTarget>;
}
```

## Implementation Plan

1. Add explicit projection input structs.
   - Avoid passing `AppState` through projection query APIs.
   - Keep construction close to `diff_panel_model` and app update entry
     points that already know the current rows and cursor.

2. Introduce `ProjectedComment` and named range/span types.
   - Use these internally before changing renderer call sites.
   - Keep existing behavior and tests green.

3. Convert marker lookup to use projected records.
   - Preserve inline row markers, side-aware markers, and full-file markers.
   - Remove the `CommentMarkerSet` alias once call sites are migrated.

4. Convert comments panel lookup to use the projection.
   - The panel should ask for the current logical comment, not recompute line
     containment.

5. Convert comment navigation to use projection targets.
   - Move `{`/`}` and comments-panel next/previous selection algorithms into
     the projection.

6. Convert unresolved navigation to use projection targets.
   - Keep cross-file ordering in app code if needed, but per-file cursor
     precedence and target rows should come from projection.

7. Remove transitional helpers.
   - Remove static methods that take `AppState`.
   - Remove duplicated min/max range helpers outside projection construction.
   - Remove renderer-facing `CommentMarkerSet` naming.

## Tests

1. Projection construction tests for each mode:
   - full-file head,
   - full-file base,
   - side-by-side,
   - inline.

2. Marker tests:
   - simple single-line comment,
   - multiline comment,
   - compound base/head comment,
   - nested comments,
   - overlapping comments,
   - identical-start comments,
   - selected comment wins overlap tie-breakers.

3. Current-comment tests:
   - cursor on context row,
   - cursor on deletion row,
   - cursor on addition row,
   - cursor inside compound comment spanning base and head,
   - cursor inside selected wider comment that contains a nested comment.

4. Navigation tests:
   - next/previous from before first comment,
   - next/previous from inside current comment span,
   - next/previous from deletion rows,
   - next/previous through same-start comments,
   - wrap behavior within current file,
   - unresolved navigation handoff to next file remains unchanged.

5. Renderer boundary tests:
   - TUI asks only for markers by projected coordinate.
   - TUI does not compute comment precedence or convert comment coordinate
     systems.

## Acceptance Criteria

- [ ] `CommentProjection` is constructed from explicit inputs, not queried
      with `AppState`.
- [ ] Projection mode is represented by Rust types or enum variants.
- [ ] Renderer-facing APIs no longer use the `CommentMarkerSet` alias.
- [ ] Comments panel current-comment lookup goes through projection instance
      methods.
- [ ] Current-comment commands go through projection instance methods.
- [ ] `{` and `}` navigation uses projection targets.
- [ ] Comments-panel next/previous navigation uses projection targets.
- [ ] Unresolved comment navigation uses projection targets for per-file
      cursor precedence and display targets.
- [ ] No TUI renderer code owns comment precedence, side selection, or
      base/head/source/row coordinate conversion.
- [ ] Existing 13k and 13l behavior remains covered and green.
