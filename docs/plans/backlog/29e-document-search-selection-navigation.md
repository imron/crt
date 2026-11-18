# 29e: Document Search, Selection, And Navigation

## Status

Backlog.

## Overview

This slice is part of the Stage 29 diff document model. Read
`29-diff-document-model.md` first for the full architecture, naming, layering,
and migration strategy.

## Goal

Move search, selection, hunk navigation, cursor preservation, and comment
anchor capture onto document APIs so app update code no longer depends on
TUI-rendered text or TUI-owned hunk rows.

## Scope

Use document row/text/source APIs for app-owned interactions:

```rust
impl Document {
    pub fn text_at(&self, row: RowIndex) -> Option<&str>;
    pub fn content_row(&self, row: RowIndex) -> Option<&ContentRow>;
    pub fn hunk_spans(&self) -> &[HunkSpan];
    pub fn next_hunk(
        &self,
        row: RowIndex,
        direction: Direction,
    ) -> Option<RowIndex>;
}
```

`content_row` is for app update code only. The TUI should not call it.

## Search

Diff search should run against document text, not terminal-rendered text.

Search matches should be represented as:

```rust
pub struct SearchMatchSpan {
    pub row: RowIndex,
    pub start_col: ColumnIndex,
    pub end_col: ColumnIndex,
}
```

The TUI renders search styling through `RenderContent.runs`.

## Selection And Comment Anchors

Visual selection should use document positions:

```rust
pub struct VisibleSelection {
    pub start: DocumentPosition,
    pub end: DocumentPosition,
}

pub struct DocumentPosition {
    pub row: RowIndex,
    pub column: ColumnIndex,
}
```

Comment anchor capture should ask the active document for selected content
rows and source metadata. The TUI should only report row/column input.

## Hunk Navigation

Next/previous hunk should use `Document::next_hunk`, not TUI hunk rows.

For side-by-side mode, `SideBySideDocument::next_hunk` returns a row index in
the shared aligned row axis. Both documents can render that same row.

## Mode Switching And Cursor Preservation

Mode switching should preserve cursor location through document source
metadata:

- read the current document row's source/text context in app update code;
- rebuild or select the new `DiffDocument`;
- find the closest corresponding row in the new document;
- update `DocumentCursor` and scroll.

The TUI should not translate cursor locations between modes.

## Tests

Pure document behavior for search, selection, hunk targets, source metadata,
and cursor equivalence is covered in 29a. This slice should test only app-layer
migration and glue:

- diff search update code calls document search APIs, not
  `AppViewport::diff_rendered_text`.
- search navigation updates app cursor/scroll from document match rows.
- visual selection update code stores document positions from app input.
- comment anchor capture asks the active document for selected source rows.
- current-line comment creation asks the active document for source metadata.
- hunk navigation update code calls `Document::next_hunk`.
- side-by-side hunk navigation uses the shared aligned row from
  `SideBySideDocument::next_hunk`.
- mode switching asks the app-owned document for source equivalence and does
  not use TUI-rendered line text.
- TUI remains only a row/column event source for selection and cursor input.

## Acceptance Criteria

- [ ] Diff search no longer depends on `AppViewport::diff_rendered_text`.
- [ ] Hunk navigation no longer depends on TUI hunk rows.
- [ ] Selection capture uses document source metadata.
- [ ] Comment creation uses document source metadata.
- [ ] Mode switching uses app-owned document source metadata.
- [ ] TUI remains a row/column event source and renderer only.
