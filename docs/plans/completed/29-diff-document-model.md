# Diff Document Model

## Status

Complete.

## Completion Note

Stage 29 is implemented. The active diff pane is backed by `ActiveDocument`
and `DiffDocument`, the TUI renders `RenderLine` values, and the previous diff
row builder, comment projection fallback, and renderer path have been removed.

## Goal

Replace the current mixed diff-row, comment-projection, and TUI-render cache
shape with an app-owned diff document model. The model should represent the
active diff view as one generic virtual document, or as two aligned generic
virtual documents for side-by-side mode.

The TUI should render visible document rows. It should not reconstruct source
line mappings, comment precedence, gutter contents, selection spans, search
spans, or base/head semantics from rendered terminal lines.

## Why

The old model split related concepts across multiple places:

- `AppState` stores raw review/session state.
- `AppModel` projected some render rows on every render.
- `CommentProjection` separately mapped comments onto rows and source lines.
- The TUI renderer built terminal lines, gutter widths, hunk rows, rendered
  text, and cache keys.
- Some app update paths queried rendered viewport state from the TUI.

This creates too many coordinate systems:

- base source line,
- head source line,
- unified document row,
- side-by-side document row,
- terminal rendered row,
- logical comment range,
- current cursor row.

Most of these are plain integer types, so incorrect conversions are easy to
write and hard to detect. Recent bugs around comment markers, side-by-side row
alignment, current comment lookup, and cursor preservation all came from this
class of problem.

The new model should make the active virtual document the single app-owned
interaction and render projection for the diff pane.

## Design Decisions

### Keep `Document` Generic

`Document` must not know whether it is a base document, head document, unified
document, or side-by-side column. It is only a virtual document:

- it has rows,
- it has gutters,
- it has text,
- it has line kinds,
- it has optional blame data,
- it has overlays.

Base/head/unified semantics belong in the document builders and in
`DiffDocument`, not in `Document`.

### Use `DiffDocument` As The Active View Shape

The active diff pane document is:

```rust
pub enum DiffDocument {
    Unified(Document),
    SideBySide(SideBySideDocument),
    Base(Document),
    Head(Document),
}
```

`Unified`, `Base`, and `Head` each render one generic `Document`.
`SideBySide` renders two aligned generic documents:

```rust
pub struct SideBySideDocument {
    pub base: Document,
    pub head: Document,
}
```

The side-by-side invariant is:

```text
base.len() == head.len()
```

The side-by-side builder inserts spacer rows into either document when needed
so the TUI can render both sides using the same virtual row index.

### Use `ActiveDocument` In App State

The expensive structural projection should live in `AppState` as cached app
state:

```rust
pub struct ActiveDocument {
    pub key: DocumentKey,
    pub diff: DiffDocument,
}
```

This does not make `AppModel` the source of truth. Raw review/session data
still lives in `AppState`. `ActiveDocument` is a disposable cached projection
that can always be rebuilt from raw state.

`App::model()` should become cheaper: it should expose the current
`ActiveDocument` through `AppModel` rather than rebuilding the full diff
projection every render.

### Use `DocumentKey` For Structural Cache Identity

`DocumentKey` answers: does the current `ActiveDocument` still describe the
selected file and view, or must it be rebuilt?

It should include inputs that affect structural rows:

```rust
pub struct DocumentKey {
    pub file_id: String,
    pub diff_hash: String,
    pub content_mode: ContentMode,
    pub render_variant: RenderVariant,
    pub diff_algorithm: DiffAlgorithm,
    pub ignore_whitespace: bool,
    pub show_blame: bool,
    pub head_content_id: Option<ContentId>,
    pub base_content_id: Option<ContentId>,
}
```

It should not include cheap overlay or viewport state:

- cursor row,
- scroll row,
- selected/current comment id,
- comment bodies or resolved state,
- search query,
- visual selection.

If `DocumentKey` changes, rebuild the structural `DiffDocument`. If it does
not change, update overlays and lightweight state.

### Split Structural Rows From Overlays

`DiffDocument` owns stable structural rows. Frequently changing decorations
are represented as overlays.

```rust
pub struct Document {
    rows: Vec<DocumentRow>,
    overlays: DocumentOverlays,
}
```

Structural rows include only data that belongs to the document itself:

```rust
pub enum DocumentRow {
    Content(ContentRow),
    Spacer,
}

pub struct ContentRow {
    pub gutter: Gutter,
    pub kind: LineKind,
    pub text: String,
    pub blame: Option<BlameInfo>,
    pub source: SourceLocation,
}
```

`DocumentRow::Spacer` is used for side-by-side alignment. It cannot contain
text, blame, source, line kind, or owned comment data, which makes invalid
spacer state unrepresentable.

Spacer rows can still receive overlay output when rendered. A multi-line
comment may pass through a spacer row, and a visual line selection may include
one. Those are overlay facts about the document row axis, not structural spacer
content.

### Keep Comments Out Of Structural Rows

Comments are overlays, not structural document rows. They change when comments
are added, resolved, unresolved, deleted, or when the current/selected comment
changes.

```rust
pub struct DocumentOverlays {
    pub comments: DocumentComments,
    pub search: SearchOverlay,
    pub selection: Option<VisibleSelection>,
    pub cursor: DocumentCursor,
}
```

`DocumentComments` maps logical comments onto the document row axis. A compound
comment can appear in multiple documents, sharing one logical comment id.

Side-by-side mode has independent comment overlays for each generic document.
The base document overlay maps base comment segments. The head document overlay
maps head comment segments. The logical comment still lives in app/server
state.

### Use `RenderLine` As The TUI Boundary

The TUI should ask a document for visible rows and render returned lines:

```rust
pub enum RenderLine<'a> {
    Content(RenderContent<'a>),
    Spacer {
        marker: CommentMarker,
        selected: bool,
    },
}

pub struct RenderContent<'a> {
    pub gutter: Cow<'a, str>,
    pub marker: CommentMarker,
    pub kind: LineKind,
    pub blame: Option<&'a BlameInfo>,
    pub runs: Vec<TextRun<'a>>,
}
```

`RenderLine` is not a ratatui type. It contains domain-level runs and semantic
line kind for content rows, plus overlay-only spacer state. The TUI maps these
to ratatui spans and styles.

There should not be both `LineKind` and `RenderLineKind`. Content rows use the
existing semantic `LineKind`. Spacer rows are represented by
`RenderLine::Spacer`, so a spacer cannot accidentally claim to be an addition,
deletion, or context row. `RenderLine::Spacer` may still carry a comment marker
and selection flag so markers and selections remain visually continuous across
side-by-side alignment gaps.

### Track Hunk Targets In Documents

Next/previous hunk navigation should not depend on TUI-rendered hunk rows.
Each `Document` should store hunk spans in its own row axis:

```rust
pub struct Document {
    rows: Vec<DocumentRow>,
    hunk_spans: Vec<HunkSpan>,
    overlays: DocumentOverlays,
}

pub struct HunkSpan {
    pub full_span: RowSpan,
    pub first_change: RowIndex,
}
```

`first_change` is the jump target for next/previous hunk navigation. The full
span remains available for titles, scroll context, and future operations that
need the whole hunk range.

For side-by-side mode, the builder should create matching hunk spans in both
aligned documents. Because `SideBySideDocument` preserves `base.len() ==
head.len()`, hunk jump targets are row-compatible across both documents.

### Treat Blame As Per-Row Optional Data

Getting blame is not a cheap TUI-only operation. Current code runs git blame
for both `HEAD` and the merge base when blame is enabled. Therefore blame
should not be eagerly required for every document.

`ContentRow` may contain `blame: Option<BlameInfo>`.

`show_blame` controls whether the document is built with available blame.
Because blame changes per-row render content and the reserved blame column, it
is part of `DocumentKey` and forces an active document rebuild.

When blame is toggled on and blame is not loaded, the app may load blame for
the active file before rebuilding the active document. When blame is toggled
off, the rebuilt active document should omit per-row blame data.

Blame loading should match the active document shape:

- `Head(Document)` needs only head blame.
- `Base(Document)` needs only base blame.
- `Unified(Document)` may contain both base and head rows, so it may need both.
- `SideBySide(SideBySideDocument)` contains base and head documents, so it may
  need both.

The document builder attaches loaded blame to the relevant `ContentRow`s. A
generic `Document` still has no base/head concept; it only has rows with
optional blame.

## Referenced Types

Several proposed structs refer to existing or adjacent domain types. Their
roles in this plan are:

- `ContentMode`: existing app mode that distinguishes diff view from full-file
  view. It contributes to `DocumentKey` because it changes the row axis.
- `RenderVariant`: existing view variant for inline, side-by-side, head, and
  base views. It contributes to `DocumentKey` because it changes the active
  `DiffDocument` variant.
- `DiffAlgorithm`: existing diff algorithm setting. It contributes to
  `DocumentKey` because it changes hunks and therefore rows.
- `DiffHunk`: existing raw diff hunk input used by document builders. It is not
  exposed to the TUI once a document is built.
- `LineKind`: existing semantic content kind for content rows: context,
  addition, or deletion. Spacer rows are represented by `DocumentRow::Spacer`
  and `RenderLine::Spacer`.
- `Comment`: existing logical review comment from app/server state. Documents
  project comments into `DocumentComments`; they do not own logical comments.
- `CommentMarker`: semantic marker returned in `RenderContent` or
  `RenderLine::Spacer`, describing no marker, start, join, end, or single-line
  marker plus active/resolved state.
- `Direction`: existing navigation direction, used by document APIs for
  next/previous comment lookup.
- `SearchInput`: proposed builder input containing the active search query and
  selected match state needed to build or refresh `SearchOverlay`.

## Proposed Types

The exact module layout can be decided during implementation, but the intended
core model shape is:

```rust
pub struct ActiveDocument {
    pub key: DocumentKey,
    pub diff: DiffDocument,
}

pub struct DocumentKey {
    pub file_id: String,
    pub diff_hash: String,
    pub content_mode: ContentMode,
    pub render_variant: RenderVariant,
    pub diff_algorithm: DiffAlgorithm,
    pub ignore_whitespace: bool,
    pub show_blame: bool,
    pub head_content_id: Option<ContentId>,
    pub base_content_id: Option<ContentId>,
}

pub struct ContentId {
    pub hash: u64,
    pub len: usize,
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

pub struct ContentRow {
    pub gutter: Gutter,
    pub kind: LineKind,
    pub text: String,
    pub blame: Option<BlameInfo>,
    pub source: SourceLocation,
}

pub struct Gutter {
    pub text: String,
}

pub struct SourceLocation {
    pub line: u32,
}

pub struct BlameInfo {
    pub hash: String,
    pub author: String,
    pub date: String,
}

pub struct DocumentOverlays {
    pub comments: DocumentComments,
    pub search: SearchOverlay,
    pub selection: Option<VisibleSelection>,
    pub cursor: DocumentCursor,
}

pub struct DocumentCursor {
    pub row: RowIndex,
    pub column: usize,
}

pub struct RowIndex(pub usize);

pub struct ColumnIndex(pub usize);

pub struct RowSpan {
    pub start: RowIndex,
    pub end: RowIndex,
}

pub struct HunkSpan {
    pub full_span: RowSpan,
    pub first_change: RowIndex,
}

pub struct VisibleSelection {
    pub start: DocumentPosition,
    pub end: DocumentPosition,
}

pub struct DocumentPosition {
    pub row: RowIndex,
    pub column: ColumnIndex,
}
```

`ColumnIndex` should be used where a value is explicitly a column coordinate,
such as selection positions and search spans. Row-only APIs should use
`RowIndex` alone.

Comment and search overlay types should be explicit enough that the TUI never
needs to compute precedence or coordinate conversions:

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

pub struct SearchOverlay {
    pub query: Option<String>,
    pub matches: Vec<SearchMatchSpan>,
    pub current: Option<usize>,
}

pub struct SearchMatchSpan {
    pub row: RowIndex,
    pub start_col: ColumnIndex,
    pub end_col: ColumnIndex,
}

pub struct TextRun<'a> {
    pub text: Cow<'a, str>,
    pub style: RunStyle,
}

pub enum RunStyle {
    Normal,
    Addition,
    Deletion,
    SearchMatch,
    CurrentSearchMatch,
    Selection,
    Cursor,
}

pub enum RenderLine<'a> {
    Content(RenderContent<'a>),
    Spacer {
        marker: CommentMarker,
        selected: bool,
    },
}

pub struct RenderContent<'a> {
    pub gutter: Cow<'a, str>,
    pub marker: CommentMarker,
    pub kind: LineKind,
    pub blame: Option<&'a BlameInfo>,
    pub runs: Vec<TextRun<'a>>,
}
```

## Proposed APIs

The document API should make the active document useful for both rendering and
app-owned interactions:

```rust
impl Document {
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn line(&self, row: RowIndex) -> Option<RenderLine<'_>>;
    pub fn text_at(&self, row: RowIndex) -> Option<&str>;
    pub fn content_row(&self, row: RowIndex) -> Option<&ContentRow>;
    pub fn current_comment_id(&self) -> Option<i64>;
    pub fn comment_at(&self, row: RowIndex) -> Option<i64>;
    pub fn comment_span(&self, id: i64) -> Option<RowSpan>;
    pub fn hunk_spans(&self) -> &[HunkSpan];
    pub fn next_hunk(
        &self,
        row: RowIndex,
        direction: Direction,
    ) -> Option<RowIndex>;
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

`content_row` replaces a narrower `source_at` helper. The source location is
used for comment anchor capture, search/definition targets, and any operation
that needs to translate a document row back to the underlying file line. Those
callers usually need the row text and line kind as well, so returning the full
`ContentRow` is clearer than exposing source coordinates alone.

`DiffDocument` should not hide its shape behind pane lookup helpers. A document
always renders as a single document. A side-by-side document always renders as
two documents. Callers should match on the enum and render or navigate the
appropriate document directly.

The top-level API should stay small:

```rust
impl DiffDocument {
    pub fn len(&self) -> usize;
    pub fn current_comment_id(&self) -> Option<i64>;
}
```

The side-by-side API should preserve the aligned-row invariant:

```rust
impl SideBySideDocument {
    pub fn len(&self) -> usize;
    pub fn next_hunk(
        &self,
        row: RowIndex,
        direction: Direction,
    ) -> Option<RowIndex>;
}
```

The TUI renders side-by-side mode by destructuring:

```rust
match diff {
    DiffDocument::SideBySide(sbs) => {
        let left = &sbs.base;
        let right = &sbs.head;
        // render left.line(row) and right.line(row)
    }
    DiffDocument::Unified(doc)
    | DiffDocument::Base(doc)
    | DiffDocument::Head(doc) => {
        // render doc.line(row)
    }
}
```

Builders should be explicit about which document they build:

```rust
pub struct DiffDocumentBuilder;

impl DiffDocumentBuilder {
    pub fn build(input: DiffDocumentInput) -> ActiveDocument;
}

pub struct DiffDocumentInput<'a> {
    pub key: DocumentKey,
    pub hunks: &'a [DiffHunk],
    pub head_content: Option<&'a str>,
    pub base_content: Option<&'a str>,
    pub head_blame: &'a [BlameInfo],
    pub base_blame: &'a [BlameInfo],
    pub comments: &'a [Comment],
    pub cursor: DocumentCursor,
    pub selected_comment_id: Option<i64>,
    pub search: SearchInput<'a>,
    pub selection: Option<VisibleSelection>,
}
```

Specific builders may exist internally:

```rust
pub fn build_unified_document(input: &DiffDocumentInput<'_>) -> Document;
pub fn build_head_document(input: &DiffDocumentInput<'_>) -> Document;
pub fn build_base_document(input: &DiffDocumentInput<'_>) -> Document;
pub fn build_side_by_side_document(
    input: &DiffDocumentInput<'_>,
) -> SideBySideDocument;
```

## App State And Update Flow

`AppState` should own the active document:

```rust
pub struct AppState {
    // existing raw state...
    pub active_document: Option<ActiveDocument>,
}
```

File changes, mode changes, diff algorithm changes, whitespace changes, and
base/head content changes rebuild the active document if `DocumentKey` changes.

Comment changes should update document comment overlays when possible. The raw
comments vector remains authoritative. If an incremental overlay update is
unclear, the active document can be rebuilt from raw state.

Search changes should update `SearchOverlay`. Search should run against
document text, not rendered terminal text.

Selection changes should update `VisibleSelection`. Capturing a comment anchor
should ask the active document to convert selected document rows into the
correct logical anchor segments.

Cursor movement should update `DocumentCursor` and current-comment overlay
state. It should not rebuild structural rows.

`AppModel` should become a cheap render snapshot:

```rust
pub struct AppModel {
    pub revision: u64,
    pub context: ConnectionContext,
    pub layout: AppLayout,
    pub file_list: FileList,
    pub diff: DiffPanel,
    pub comments_panel: CommentsPanel,
    pub focus: PaneFocus,
}

pub struct DiffPanel {
    pub document: Option<DiffDocument>,
    pub scroll: usize,
    pub cursor: DocumentCursor,
    pub show_blame: bool,
}
```

The exact ownership may use references, clones, or `Arc` depending on lifetime
and performance constraints. The design requirement is that `AppModel` exposes
the active app-owned document rather than rebuilding it every render.

## Rendering Flow

The TUI renderer should use only document APIs:

```text
read model.diff.document
read model.diff.scroll
read visible height from terminal layout
for each visible document row:
  ask Document::line(row)
  map Gutter, CommentMarker, BlameInfo, LineKind, TextRun to ratatui
  render cells
```

For side-by-side mode:

```text
for each visible row:
  left = side_by_side.base.line(row)
  right = side_by_side.head.line(row)
  render left document column
  render right document column
```

The TUI may own terminal geometry and ratatui styling. It must not own:

- source line lookup,
- base/head line conversion,
- comment marker precedence,
- comment current/selected lookup,
- search result computation,
- selection projection,
- hunk row computation.

## Migration Strategy

This plan should be implemented in focused subplans:

- `29a-diff-document-core.md`: add core document types and builders.
- `29b-renderline-tui-adapter.md`: render documents through `RenderLine`.
- `29c-active-document-state.md`: store `ActiveDocument` in `AppState`.
- `29d-document-comment-overlays.md`: move comments into document overlays.
- `29e-document-search-selection-navigation.md`: move search, selection, and
  navigation to document APIs.
- `29f-remove-legacy-diff-projections.md`: remove obsolete projections and
  TUI-owned semantic reconstruction.

29f is complete: the local `*` renderer toggle has been removed, and the TUI
renders only from `DiffDocument`.

The first implementation should prefer correctness and clear ownership over
micro-optimized incremental updates. Once behavior is equivalent, comment,
search, blame, and cursor overlays can be updated incrementally.

## Acceptance Criteria

- [ ] The active diff view is represented by `DiffDocument`.
- [ ] `SideBySideDocument` contains two aligned generic `Document`s.
- [ ] `Document` has no base/head-specific APIs or state.
- [ ] Spacer rows cannot contain text, source, blame, or comments.
- [ ] TUI rendering consumes `RenderLine` values.
- [ ] TUI code does not reconstruct source line mappings.
- [ ] TUI code does not compute comment precedence or current comments.
- [ ] Diff search runs against document text, not terminal rendered text.
- [ ] Comment navigation uses document comment spans.
- [ ] Comment creation captures anchors from document source metadata.
- [ ] `show_blame` changes `DocumentKey` and rebuilds the active document.
- [ ] `AppState` owns the active document projection.
- [ ] `AppModel` no longer rebuilds the full diff projection every render.
- [ ] Existing comment, search, selection, hunk, and side-by-side tests remain
      green or are replaced by higher-fidelity document tests.
