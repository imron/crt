# 29a: Diff Document Core

## Status

Complete.

## Overview

This slice is part of the Stage 29 diff document model. Read
`29-diff-document-model.md` first for the full architecture, naming, layering,
and migration strategy.

## Goal

Add the core `DiffDocument`, `Document`, row, hunk, and builder types without
changing the active TUI rendering path.

This slice establishes the new app-owned virtual document model and proves it
can represent the current inline, side-by-side, head, and base views.
It also establishes the comment overlay model and marker projection tests early,
before any app update path is migrated to consume it.

## Scope

Add the structural model:

```rust
pub struct ActiveDocument {
    pub key: DocumentKey,
    pub diff: DiffDocument,
}

pub enum DiffDocument {
    Unified(Document),
    SideBySide(SideBySideDocument),
    Base(Document),
    Head(Document),
}

pub struct SideBySideDocument {
    pub base: Document,
    pub head: Document,
}

pub struct Document {
    rows: Vec<DocumentRow>,
    hunk_spans: Vec<HunkSpan>,
    overlays: DocumentOverlays,
}

pub enum DocumentRow {
    Content(ContentRow),
    Spacer,
}
```

Add typed coordinates:

```rust
pub struct RowIndex(pub usize);
pub struct ColumnIndex(pub usize);
pub struct RowSpan {
    pub start: RowIndex,
    pub end: RowIndex,
}
```

Add structural row content:

```rust
pub struct ContentRow {
    pub gutter: Gutter,
    pub kind: LineKind,
    pub text: String,
    pub blame: Option<BlameInfo>,
    pub source: SourceLocation,
}
```

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

pub struct MarkerRows {
    pub start: RowIndex,
    pub end: RowIndex,
}
```

Add document builders:

```rust
pub struct DiffDocumentBuilder;

impl DiffDocumentBuilder {
    pub fn build(input: DiffDocumentInput) -> ActiveDocument;
}

pub fn build_unified_document(input: &DiffDocumentInput<'_>) -> Document;
pub fn build_head_document(input: &DiffDocumentInput<'_>) -> Document;
pub fn build_base_document(input: &DiffDocumentInput<'_>) -> Document;
pub fn build_side_by_side_document(
    input: &DiffDocumentInput<'_>,
) -> SideBySideDocument;
```

The builders now own document row construction directly. The old `diff_rows`
module has been removed, and callers consume the document API.

The builders should also populate `DocumentComments` for the constructed
document. Later slices will migrate app update code to use those overlays, but
the model and marker behavior should be tested in this slice.

## Design Requirements

- `Document` has no base/head-specific state or methods.
- `SideBySideDocument` contains two generic documents.
- `SideBySideDocument.base.len() == SideBySideDocument.head.len()`.
- Spacer rows are represented by `DocumentRow::Spacer`.
- Spacer rows cannot contain source, text, blame, owned comments, or line
  kind.
- Spacer rows can render overlay-only markers and selection state through
  `RenderLine::Spacer`.
- `DocumentKey` includes structural inputs and blame visibility.
- `DocumentKey` excludes cursor, scroll, comments, search, selection, and
  selected comment.
- `HunkSpan.first_change` is the hunk navigation target.
- `HunkSpan.full_span` describes the complete hunk in document row space.
- Blame is attached only when loaded and only to relevant content rows.
- `DocumentComments` maps logical comments onto the generic document row axis.
- Comment overlays are per-document, including in side-by-side mode.
- A compound logical comment may appear in both side-by-side documents using
  the same comment id.
- Selected/current comment precedence is computed by `DocumentComments`, not
  by the TUI.
- `RenderContent.marker` and `RenderLine::Spacer.marker` can be produced from
  `DocumentComments`.

## Blame Rules

Builders attach blame based on the document being built:

- `Head(Document)` uses head blame only.
- `Base(Document)` uses base blame only.
- `Unified(Document)` may use both, depending on row kind/source.
- `SideBySideDocument` uses base blame for `base` and head blame for `head`.

No generic `Document` API should expose base/head-specific blame lookup.

## Tests

Add unit tests for:

- `DocumentKey` changes when document inputs change, including
  `show_blame`.
- `DocumentKey` does not change for cursor, scroll, comments, search,
  selection, or selected comment.
- unified document rows match current inline row count and line ordering.
- head document rows contain only head-side source lines.
- base document rows contain only base-side source lines.
- side-by-side documents have equal lengths.
- side-by-side insertion/deletion/replacement rows insert spacers on the
  correct side.
- spacer rows cannot expose content through `content_row`.
- spacer render lines can expose comment markers and selection state without
  exposing source content.
- hunk spans and first-change rows match existing hunk navigation targets.
- `Document::next_hunk` uses `HunkSpan.first_change`.
- hunk navigation works in unified, base, head, and side-by-side documents.
- blame is attached only when supplied to the builder.
- document comments are projected into row spans for unified documents.
- document comments are projected into row spans for head documents.
- document comments are projected into row spans for base documents.
- document comments are projected separately into both side-by-side documents.
- single-line unresolved comments produce unresolved single-line markers.
- single-line resolved comments produce resolved single-line markers.
- multiline comments produce start, join, and end markers.
- nested comments still expose markers for both comments.
- overlapping comments use the selected/current comment precedence rules.
- same-start comments do not collapse into one unreachable marker.
- selected comments win same-start and overlap tie-breakers.
- current comments mark only the selected/current marker span as current.
- current-comment lookup returns the selected/current visible comment.
- next/previous comment walks same-start comments without skipping.
- next/previous comment from deletion rows advances correctly.
- next/previous unresolved comment returns no target when none exist.
- document text search ignores gutter text.
- document text search returns `ColumnIndex` content coordinates.
- search highlighting is represented through `RenderContent.runs`.
- visual line selection stores document row spans.
- text selection stores document row/column positions.
- selected content rows expose source metadata for anchor capture.
- current-line comment capture can be derived from document source metadata.
- multiline comment capture can be derived from selected document rows.
- deletion/addition/context rows expose enough metadata for anchor segments.
- equivalent source lookup can preserve cursor across document variants.

## Acceptance Criteria

- [x] New document types compile and are covered by unit tests.
- [x] Builders can construct all four `DiffDocument` variants.
- [x] `DocumentComments` is constructed for all four variants.
- [x] Comment markers are covered by document-level unit tests.
- [x] Search, selection, hunk, and source metadata behavior is covered at the
      document layer.
- [x] Existing rendering and update behavior is unchanged.
- [x] No TUI code consumes the new document model yet.
- [x] The slice introduced the document model before later slices removed the
      legacy diff projection code.
