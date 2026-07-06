# 29b: RenderLine TUI Adapter

## Status

Backlog.

## Overview

This slice is part of the Stage 29 diff document model. Read
`29-diff-document-model.md` first for the full architecture, naming, layering,
and migration strategy.

## Goal

Move diff rendering onto the new document boundary by making the TUI render
`RenderLine` values instead of reconstructing semantic rows from old diff row
models.

This slice should keep `ActiveDocument` derived through `AppModel` rather than
stored in `AppState`. The goal is to prove the renderer can consume the new
shape before changing state ownership.

## Scope

Add the render boundary:

```rust
pub enum RenderLine<'a> {
    Content(RenderContent<'a>),
    Spacer,
}

pub struct RenderContent<'a> {
    pub gutter: Cow<'a, str>,
    pub marker: CommentMarker,
    pub kind: LineKind,
    pub blame: Option<&'a BlameInfo>,
    pub runs: Vec<TextRun<'a>>,
}
```

Add document render APIs:

```rust
impl Document {
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn line(&self, row: RowIndex) -> Option<RenderLine<'_>>;
    pub fn text_at(&self, row: RowIndex) -> Option<&str>;
    pub fn content_row(&self, row: RowIndex) -> Option<&ContentRow>;
}
```

Update TUI diff rendering so:

- single-document variants render with `doc.line(row)`;
- side-by-side renders `sbs.base.line(row)` on the left and
  `sbs.head.line(row)` on the right;
- ratatui-specific styling remains in the TUI;
- semantic runs and line kinds come from the document.

## Design Requirements

- The TUI may decide terminal layout, widths, clipping, and ratatui styles.
- The TUI must not compute base/head line mappings.
- The TUI must not compute hunk row positions from hunks.
- The TUI must not compute comment marker precedence.
- The TUI must not inspect raw diff hunks to draw document rows.
- Side-by-side rendering must destructure `DiffDocument::SideBySide`.
- Do not add `DiffPaneSide`, `document_for_pane`, `base_line`, or
  `head_line` helpers.

## Cache Rules

The current TUI diff cache can remain temporarily, but its key should move
toward document identity plus terminal width. It should not require raw hunks
or old diff row models once this slice is complete.

If cache migration is too large for this slice, keep the old cache behind the
new document render adapter and defer deletion to 29f.

## Tests

Add renderer boundary tests for:

- unified rows render from `RenderLine`.
- head rows render from `RenderLine`.
- base rows render from `RenderLine`.
- side-by-side renders the base document on the left and head document on the
  right.
- spacer rows render blank content and do not expose source text.
- line kind styling is derived from `RenderContent.kind`.
- blame is rendered only when `show_blame` is enabled and blame exists.
- TUI rendering does not call old base/head source mapping helpers.

## Acceptance Criteria

- [ ] TUI diff rendering consumes `RenderLine`.
- [ ] Side-by-side rendering destructures `SideBySideDocument`.
- [ ] TUI no longer reconstructs source line mappings for visible rows.
- [ ] Existing visual output remains equivalent.
- [ ] Existing diff, side-by-side, hunk, blame, and comment marker tests stay
      green or are replaced by document-render tests.
