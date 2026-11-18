# 29d: Document Comment Overlays

## Status

Complete.

## Overview

This slice is part of the Stage 29 diff document model. Read
`29-diff-document-model.md` first for the full architecture, naming, layering,
and migration strategy.

## Goal

Move comment marker projection, current-comment lookup, comment spans, and
comment navigation into `DocumentComments` overlays.

After this slice, app update code should ask the active document for comment
state instead of recomputing comment precedence from source lines.

## Scope

Add comment overlay structures:

```rust
pub struct DocumentComments {
    comments: Vec<DocumentComment>,
}

pub struct DocumentComment {
    pub id: i64,
    pub span: RowSpan,
    pub marker_rows: MarkerRows,
    pub resolved: bool,
    pub selected: bool,
    pub current: bool,
}
```

Add document comment APIs:

```rust
impl Document {
    pub fn current_comment_id(&self) -> Option<i64>;
    pub fn comment_at(&self, row: RowIndex) -> Option<i64>;
    pub fn comment_span(&self, id: i64) -> Option<RowSpan>;
    pub fn next_comment(
        &self,
        row: RowIndex,
        direction: Direction,
    ) -> Option<RowIndex>;
    pub fn next_unresolved_comment(
        &self,
        row: RowIndex,
        direction: Direction,
    ) -> Option<RowIndex>;
}
```

`RenderContent.marker` and `RenderLine::Spacer.marker` should be produced from
`DocumentComments`.

## Design Requirements

- Logical comments remain in app/server state.
- `DocumentComments` is a projection into one generic document's row axis.
- Side-by-side mode has one comment overlay per document.
- A compound logical comment may appear in both side-by-side documents using
  the same comment id.
- Selected/current comment precedence is owned by `DocumentComments`.
- TUI code must not choose comment precedence.
- App update code must not manually convert base/head/source lines to decide
  which visible comment is current.

## Migration Targets

Move behavior out of the transitional comment projection into document
overlays:

- marker lookup,
- current comment lookup,
- same-start and overlap tie-breaking,
- selected-comment override,
- display span lookup,
- next/previous comment in current file,
- next/previous unresolved comment within a document.

Cross-file unresolved comment ordering can stay in app update code. Per-file
row precedence and target row calculation should come from documents.

## Tests

Document-level marker, precedence, span, and navigation behavior is covered in
29a. This slice should test only the migration and app-layer glue:

- current-comment commands call document APIs instead of old projection
  helpers.
- comments panel current lookup reads the active document overlay.
- `{` and `}` use document comment navigation for current-file movement.
- cross-file unresolved navigation keeps app-owned file ordering while using
  document row targets for the active file.
- resolving, unresolving, editing, and deleting comments refresh document
  overlays without rebuilding structural rows.
- status messages for no unresolved comments still come from app update code.
- TUI rendering receives markers through `RenderContent.marker` or
  `RenderLine::Spacer.marker` and does not compute marker precedence.

## Implementation Notes

- Added `DiffDocument` and `SideBySideDocument` comment accessors so app code
  can ask the active document for current comment, comment span, next comment,
  and next unresolved comment.
- Updated comments panel lookup, current-comment context, direct comment
  navigation, and same-file unresolved navigation to use active document
  comment APIs first.
- Active comment lookup and navigation now use active document comment APIs.
  Test setup that needs comment semantics builds an active document explicitly.
- Comment create, edit, resolve, unresolve, delete, and reload paths now
  refresh document overlays after changing `AppState.comments`.
- Adjusted document navigation so explicit selected comments drive ordered
  movement, while ordinary cursor-based navigation starts from the document row.

## Acceptance Criteria

- [x] `RenderContent.marker` and `RenderLine::Spacer.marker` come from
      `DocumentComments`.
- [x] Current-comment commands use document APIs.
- [x] Comments panel current lookup uses document APIs.
- [x] `{` and `}` use document comment navigation for current-file movement.
- [x] TUI code does not compute comment marker precedence.
- [x] Transitional comment projection APIs are no longer used for active
      document rendering.
