use std::borrow::Cow;

use crate::app::model::{BlameLine, CommentMarker, CommentMarkerKind};
use crate::config::DiffAlgorithm;
use crate::core::navigation::Direction;
use crate::review_types::{
    Comment, CommentAnchorSide, ContentMode, DiffHunk, DiffLine, LineKind, RenderVariant,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveDocument {
    pub key: DocumentKey,
    pub diff: DiffDocument,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentKey {
    pub file_id: String,
    pub diff_hash: String,
    pub content_mode: ContentMode,
    pub render_variant: RenderVariant,
    pub diff_algorithm: DiffAlgorithm,
    pub ignore_whitespace: bool,
    pub head_content_id: Option<ContentId>,
    pub base_content_id: Option<ContentId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentId(pub String);

impl From<&str> for ContentId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for ContentId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffDocument {
    Unified(Document),
    SideBySide(SideBySideDocument),
    Base(Document),
    Head(Document),
}

impl DiffDocument {
    pub fn len(&self) -> usize {
        match self {
            Self::Unified(document) | Self::Base(document) | Self::Head(document) => document.len(),
            Self::SideBySide(document) => document.len(),
        }
    }

    pub fn current_comment_id(&self) -> Option<i64> {
        match self {
            Self::Unified(document) | Self::Base(document) | Self::Head(document) => {
                document.current_comment_id()
            }
            Self::SideBySide(document) => document.current_comment_id(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffDocumentBuildError {
    InvalidSideBySideDocument(SideBySideDocumentError),
}

impl std::fmt::Display for DiffDocumentBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSideBySideDocument(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for DiffDocumentBuildError {}

impl From<SideBySideDocumentError> for DiffDocumentBuildError {
    fn from(value: SideBySideDocumentError) -> Self {
        Self::InvalidSideBySideDocument(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SideBySideDocument {
    base: Document,
    head: Document,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SideBySideDocumentError {
    pub base_len: usize,
    pub head_len: usize,
}

impl std::fmt::Display for SideBySideDocumentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "side-by-side documents must have equal row counts: base={}, head={}",
            self.base_len, self.head_len
        )
    }
}

impl std::error::Error for SideBySideDocumentError {}

impl SideBySideDocument {
    pub fn new(base: Document, head: Document) -> Result<Self, SideBySideDocumentError> {
        let base_len = base.len();
        let head_len = head.len();
        if base_len != head_len {
            return Err(SideBySideDocumentError { base_len, head_len });
        }
        Ok(Self { base, head })
    }

    pub fn base(&self) -> &Document {
        &self.base
    }

    pub fn head(&self) -> &Document {
        &self.head
    }

    pub fn into_parts(self) -> (Document, Document) {
        (self.base, self.head)
    }

    pub fn len(&self) -> usize {
        self.base.len()
    }

    pub fn current_comment_id(&self) -> Option<i64> {
        self.head
            .current_comment_id()
            .or_else(|| self.base.current_comment_id())
    }

    pub fn next_hunk(&self, row: RowIndex, direction: Direction) -> Option<RowIndex> {
        self.head
            .next_hunk(row, direction)
            .or_else(|| self.base.next_hunk(row, direction))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    rows: Vec<DocumentRow>,
    hunk_spans: Vec<HunkSpan>,
    overlays: DocumentOverlays,
}

impl Document {
    pub fn new(rows: Vec<DocumentRow>, hunk_spans: Vec<HunkSpan>) -> Self {
        let overlays = DocumentOverlays::default();
        Self {
            rows,
            hunk_spans,
            overlays,
        }
    }

    pub fn with_overlays(mut self, overlays: DocumentOverlays) -> Self {
        self.overlays = overlays;
        self
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn row(&self, row: RowIndex) -> Option<&DocumentRow> {
        self.rows.get(row.0)
    }

    pub fn rows(&self) -> &[DocumentRow] {
        &self.rows
    }

    pub fn content_row(&self, row: RowIndex) -> Option<&ContentRow> {
        match self.row(row)? {
            DocumentRow::Content(content) => Some(content),
            DocumentRow::Spacer => None,
        }
    }

    pub fn text_at(&self, row: RowIndex) -> Option<&str> {
        Some(&self.content_row(row)?.text)
    }

    pub fn line(&self, row: RowIndex) -> Option<RenderLine<'_>> {
        match self.row(row)? {
            DocumentRow::Content(content) => Some(RenderLine::Content(RenderContent {
                gutter: Cow::Borrowed(content.gutter.text.as_str()),
                marker: self.overlays.comments.marker_for_row(row),
                kind: content.kind,
                blame: content.blame.as_ref(),
                runs: self.overlays.render_runs_for(row, content),
            })),
            DocumentRow::Spacer => Some(RenderLine::Spacer),
        }
    }

    pub fn overlays(&self) -> &DocumentOverlays {
        &self.overlays
    }

    pub fn overlays_mut(&mut self) -> &mut DocumentOverlays {
        &mut self.overlays
    }

    pub fn hunk_spans(&self) -> &[HunkSpan] {
        &self.hunk_spans
    }

    pub fn current_comment_id(&self) -> Option<i64> {
        self.overlays.comments.current_comment_id()
    }

    pub fn comment_at(&self, row: RowIndex) -> Option<&DocumentComment> {
        self.overlays.comments.current_comment_at(row)
    }

    pub fn comment_span(&self, id: i64) -> Option<RowSpan> {
        self.overlays.comments.comment_span(id)
    }

    pub fn next_comment(&self, row: RowIndex, direction: Direction) -> Option<&DocumentComment> {
        self.overlays.comments.next_comment(row, direction)
    }

    pub fn next_unresolved_comment(
        &self,
        row: RowIndex,
        direction: Direction,
    ) -> Option<&DocumentComment> {
        self.overlays
            .comments
            .next_unresolved_comment(row, direction)
    }

    pub fn next_hunk(&self, row: RowIndex, direction: Direction) -> Option<RowIndex> {
        match direction {
            Direction::Next => self
                .hunk_spans
                .iter()
                .find(|span| span.first_change > row)
                .map(|span| span.first_change),
            Direction::Prev => self
                .hunk_spans
                .iter()
                .rev()
                .find(|span| span.first_change < row)
                .map(|span| span.first_change),
        }
    }

    pub fn search(&mut self, query: impl Into<String>) {
        self.overlays.search = SearchOverlay::new(query.into(), &self.rows);
    }

    pub fn clear_search(&mut self) {
        self.overlays.search = SearchOverlay::default();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentRow {
    Content(ContentRow),
    Spacer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentRow {
    pub gutter: Gutter,
    pub kind: LineKind,
    pub text: String,
    pub blame: Option<BlameInfo>,
    pub source: SourceLocation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gutter {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameInfo {
    pub hash: String,
    pub author: String,
    pub date: String,
}

impl From<&BlameLine> for BlameInfo {
    fn from(value: &BlameLine) -> Self {
        Self {
            hash: value.hash.clone(),
            author: value.author.clone(),
            date: value.date.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLocation {
    pub lines: Vec<SourceLine>,
}

impl SourceLocation {
    pub fn none() -> Self {
        Self { lines: Vec::new() }
    }

    pub fn single(side: CommentAnchorSide, line: u32) -> Self {
        Self {
            lines: vec![SourceLine { side, line }],
        }
    }

    pub fn from_lines(lines: Vec<SourceLine>) -> Self {
        Self { lines }
    }

    pub fn contains(&self, side: CommentAnchorSide, start: u32, end: u32) -> bool {
        self.lines
            .iter()
            .any(|line| line.side == side && line.line >= start && line.line <= end)
    }

    pub fn line_for_side(&self, side: CommentAnchorSide) -> Option<u32> {
        self.lines
            .iter()
            .find(|line| line.side == side)
            .map(|line| line.line)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceLine {
    pub side: CommentAnchorSide,
    pub line: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RowIndex(pub usize);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ColumnIndex(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowSpan {
    pub start: RowIndex,
    pub end: RowIndex,
}

impl RowSpan {
    pub fn contains(self, row: RowIndex) -> bool {
        row >= self.start && row <= self.end
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HunkSpan {
    pub full_span: RowSpan,
    pub first_change: RowIndex,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DocumentOverlays {
    pub comments: DocumentComments,
    pub search: SearchOverlay,
    pub selection: Option<VisibleSelection>,
    pub cursor: DocumentCursor,
}

impl DocumentOverlays {
    fn render_runs_for<'a>(&'a self, row: RowIndex, content: &'a ContentRow) -> Vec<TextRun<'a>> {
        self.search.render_runs_for(row, content.text.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DocumentCursor {
    pub row: RowIndex,
    pub column: ColumnIndex,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisibleSelection {
    Line(RowSpan),
    Text {
        start: DocumentPosition,
        end: DocumentPosition,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentPosition {
    pub row: RowIndex,
    pub column: ColumnIndex,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DocumentComments {
    comments: Vec<DocumentComment>,
    selected_comment_id: Option<i64>,
    current_comment_id: Option<i64>,
}

impl DocumentComments {
    pub fn new(comments: Vec<DocumentComment>) -> Self {
        Self::with_selection(comments, None, None)
    }

    pub fn with_selection(
        mut comments: Vec<DocumentComment>,
        selected_comment_id: Option<i64>,
        cursor: Option<RowIndex>,
    ) -> Self {
        comments.sort_by_key(|comment| (comment.span.start, comment.id));
        let current_comment_id =
            current_comment_id_for_rows(&comments, selected_comment_id, cursor);
        for comment in &mut comments {
            comment.selected = Some(comment.id) == selected_comment_id;
            comment.current = Some(comment.id) == current_comment_id;
        }
        Self {
            comments,
            selected_comment_id,
            current_comment_id,
        }
    }

    pub fn all(&self) -> &[DocumentComment] {
        &self.comments
    }

    pub fn current_comment_id(&self) -> Option<i64> {
        self.current_comment_id
    }

    pub fn selected_comment_id(&self) -> Option<i64> {
        self.selected_comment_id
    }

    pub fn comment_span(&self, id: i64) -> Option<RowSpan> {
        self.comments
            .iter()
            .find(|comment| comment.id == id)
            .map(|comment| comment.span)
    }

    pub fn current_comment_at(&self, row: RowIndex) -> Option<&DocumentComment> {
        let candidates: Vec<&DocumentComment> = self
            .comments
            .iter()
            .filter(|comment| comment.span.contains(row))
            .collect();
        if let Some(selected) = self
            .selected_comment_id
            .and_then(|id| candidates.iter().copied().find(|comment| comment.id == id))
        {
            return Some(selected);
        }
        if let Some(current) = self
            .current_comment_id
            .and_then(|id| candidates.iter().copied().find(|comment| comment.id == id))
        {
            return Some(current);
        }
        preferred_comment(candidates)
    }

    pub fn marker_for_row(&self, row: RowIndex) -> CommentMarker {
        let Some(comment) = self.current_marker_comment(row) else {
            return CommentMarker::none();
        };
        CommentMarker::new(
            marker_kind(comment.marker_rows, row),
            comment.resolved,
            comment.current,
        )
    }

    pub fn markers_for_row(&self, row: RowIndex) -> Vec<RowCommentMarker> {
        self.comments
            .iter()
            .filter(|comment| comment.marker_rows.contains(row))
            .map(|comment| RowCommentMarker {
                comment_id: comment.id,
                marker: CommentMarker::new(
                    marker_kind(comment.marker_rows, row),
                    comment.resolved,
                    comment.current,
                ),
            })
            .collect()
    }

    pub fn next_comment(&self, row: RowIndex, direction: Direction) -> Option<&DocumentComment> {
        self.next_matching_comment(row, direction, |_| true)
    }

    pub fn next_unresolved_comment(
        &self,
        row: RowIndex,
        direction: Direction,
    ) -> Option<&DocumentComment> {
        self.next_matching_comment(row, direction, |comment| !comment.resolved)
    }

    fn next_matching_comment(
        &self,
        row: RowIndex,
        direction: Direction,
        predicate: impl Fn(&DocumentComment) -> bool,
    ) -> Option<&DocumentComment> {
        let comments: Vec<&DocumentComment> = self
            .comments
            .iter()
            .filter(|comment| predicate(comment))
            .collect();
        if comments.is_empty() {
            return None;
        }
        let selected_index = self
            .current_comment_id
            .or(self.selected_comment_id)
            .and_then(|id| comments.iter().position(|comment| comment.id == id));
        match (direction, selected_index) {
            (Direction::Next, Some(index)) => {
                comments.get(index.saturating_add(1)).copied().or_else(|| {
                    comments
                        .iter()
                        .copied()
                        .find(|comment| comment.span.start > row)
                })
            }
            (Direction::Prev, Some(index)) => index
                .checked_sub(1)
                .and_then(|previous| comments.get(previous).copied())
                .or_else(|| {
                    comments
                        .iter()
                        .rev()
                        .copied()
                        .find(|comment| comment.span.start < row || comment.span.contains(row))
                }),
            (Direction::Next, None) => comments.iter().copied().find(|comment| {
                comment.span.start > row
                    || (comment.span.start == row && !comment.span.contains(row))
            }),
            (Direction::Prev, None) => comments
                .iter()
                .rev()
                .copied()
                .find(|comment| comment.span.start < row),
        }
    }

    fn current_marker_comment(&self, row: RowIndex) -> Option<&DocumentComment> {
        let candidates: Vec<&DocumentComment> = self
            .comments
            .iter()
            .filter(|comment| comment.marker_rows.contains(row))
            .collect();
        if let Some(current) = self
            .current_comment_id
            .and_then(|id| candidates.iter().copied().find(|comment| comment.id == id))
        {
            return Some(current);
        }
        preferred_comment(candidates)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentComment {
    pub id: i64,
    pub span: RowSpan,
    pub marker_rows: MarkerRows,
    pub resolved: bool,
    pub selected: bool,
    pub current: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkerRows {
    pub start: RowIndex,
    pub end: RowIndex,
}

impl MarkerRows {
    fn contains(self, row: RowIndex) -> bool {
        row >= self.start && row <= self.end
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowCommentMarker {
    pub comment_id: i64,
    pub marker: CommentMarker,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchOverlay {
    pub query: Option<String>,
    pub matches: Vec<SearchMatch>,
}

impl SearchOverlay {
    pub fn new(query: String, rows: &[DocumentRow]) -> Self {
        if query.is_empty() {
            return Self::default();
        }
        let matches = rows
            .iter()
            .enumerate()
            .filter_map(|(row, document_row)| match document_row {
                DocumentRow::Content(content) => Some((row, content.text.as_str())),
                DocumentRow::Spacer => None,
            })
            .flat_map(|(row, text)| {
                text.match_indices(&query)
                    .map(move |(start, value)| SearchMatch {
                        row: RowIndex(row),
                        start: ColumnIndex(start),
                        end: ColumnIndex(start + value.len()),
                    })
            })
            .collect();
        Self {
            query: Some(query),
            matches,
        }
    }

    fn render_runs_for<'a>(&'a self, row: RowIndex, text: &'a str) -> Vec<TextRun<'a>> {
        let matches: Vec<&SearchMatch> = self
            .matches
            .iter()
            .filter(|search_match| search_match.row == row)
            .collect();
        if matches.is_empty() {
            return vec![TextRun {
                text: Cow::Borrowed(text),
                kind: TextRunKind::Plain,
            }];
        }

        let mut runs = Vec::new();
        let mut offset = 0;
        for search_match in matches {
            if search_match.start.0 > offset {
                runs.push(TextRun {
                    text: Cow::Borrowed(&text[offset..search_match.start.0]),
                    kind: TextRunKind::Plain,
                });
            }
            runs.push(TextRun {
                text: Cow::Borrowed(&text[search_match.start.0..search_match.end.0]),
                kind: TextRunKind::SearchMatch,
            });
            offset = search_match.end.0;
        }
        if offset < text.len() {
            runs.push(TextRun {
                text: Cow::Borrowed(&text[offset..]),
                kind: TextRunKind::Plain,
            });
        }
        runs
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchMatch {
    pub row: RowIndex,
    pub start: ColumnIndex,
    pub end: ColumnIndex,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderLine<'a> {
    Content(RenderContent<'a>),
    Spacer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderContent<'a> {
    pub gutter: Cow<'a, str>,
    pub marker: CommentMarker,
    pub kind: LineKind,
    pub blame: Option<&'a BlameInfo>,
    pub runs: Vec<TextRun<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRun<'a> {
    pub text: Cow<'a, str>,
    pub kind: TextRunKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextRunKind {
    Plain,
    SearchMatch,
}

#[derive(Debug, Clone)]
pub struct DiffDocumentInput<'a> {
    pub key: DocumentKey,
    pub file_path: &'a str,
    pub hunks: &'a [DiffHunk],
    pub head_content: Option<&'a str>,
    pub base_content: Option<&'a str>,
    pub head_blame: &'a [BlameLine],
    pub base_blame: &'a [BlameLine],
    pub comments: &'a [Comment],
    pub selected_comment_id: Option<i64>,
    pub cursor: Option<RowIndex>,
}

pub struct DiffDocumentBuilder;

impl DiffDocumentBuilder {
    pub fn build(input: DiffDocumentInput<'_>) -> Result<ActiveDocument, DiffDocumentBuildError> {
        let diff = match (input.key.content_mode, input.key.render_variant) {
            (ContentMode::Diff, RenderVariant::SideBySide) => {
                DiffDocument::SideBySide(build_side_by_side_document(&input)?)
            }
            (ContentMode::FullFile, RenderVariant::BaseVersion) => {
                DiffDocument::Base(build_base_document(&input))
            }
            (ContentMode::FullFile, RenderVariant::HeadVersion) => {
                DiffDocument::Head(build_head_document(&input))
            }
            _ => DiffDocument::Unified(build_unified_document(&input)),
        };
        Ok(ActiveDocument {
            key: input.key,
            diff,
        })
    }
}

pub fn build_unified_document(input: &DiffDocumentInput<'_>) -> Document {
    let mut document = build_unified_structural_document(input);
    document.overlays.comments = project_comments(
        document.rows(),
        input.file_path,
        input.comments,
        input.selected_comment_id,
        input.cursor,
    );
    document
}

pub fn build_head_document(input: &DiffDocumentInput<'_>) -> Document {
    let mut document = build_full_file_document(
        input.head_content,
        input.hunks,
        CommentAnchorSide::Head,
        input.head_blame,
    );
    document.overlays.comments = project_comments(
        document.rows(),
        input.file_path,
        input.comments,
        input.selected_comment_id,
        input.cursor,
    );
    document
}

pub fn build_base_document(input: &DiffDocumentInput<'_>) -> Document {
    let mut document = build_full_file_document(
        input.base_content,
        input.hunks,
        CommentAnchorSide::Base,
        input.base_blame,
    );
    document.overlays.comments = project_comments(
        document.rows(),
        input.file_path,
        input.comments,
        input.selected_comment_id,
        input.cursor,
    );
    document
}

pub fn build_side_by_side_document(
    input: &DiffDocumentInput<'_>,
) -> Result<SideBySideDocument, SideBySideDocumentError> {
    let (mut base, mut head) = build_side_by_side_structural_documents(input);
    base.overlays.comments = project_comments(
        base.rows(),
        input.file_path,
        input.comments,
        input.selected_comment_id,
        input.cursor,
    );
    head.overlays.comments = project_comments(
        head.rows(),
        input.file_path,
        input.comments,
        input.selected_comment_id,
        input.cursor,
    );
    SideBySideDocument::new(base, head)
}

fn build_unified_structural_document(input: &DiffDocumentInput<'_>) -> Document {
    let head_lines = split_content_lines(input.head_content);
    if input.hunks.is_empty() {
        let rows = head_lines
            .iter()
            .enumerate()
            .map(|(idx, text)| {
                let line = (idx + 1) as u32;
                content_row(
                    Some(line),
                    Some(line),
                    text,
                    LineKind::Context,
                    input.head_blame,
                    input.base_blame,
                )
            })
            .map(DocumentRow::Content)
            .collect();
        return Document::new(rows, Vec::new());
    }

    let mut rows = Vec::new();
    let mut hunk_spans = Vec::new();
    let mut old_cursor = 1;
    let mut new_cursor = 1;

    for hunk in input.hunks {
        while old_cursor < hunk.old_start || new_cursor < hunk.new_start {
            let old_line = (old_cursor < hunk.old_start).then_some(old_cursor);
            let new_line = (new_cursor < hunk.new_start).then_some(new_cursor);
            if let Some(text) = new_line.and_then(|line| source_line(&head_lines, line)) {
                rows.push(DocumentRow::Content(content_row(
                    old_line,
                    new_line,
                    text,
                    LineKind::Context,
                    input.head_blame,
                    input.base_blame,
                )));
            }
            if old_line.is_some() {
                old_cursor = old_cursor.saturating_add(1);
            }
            if new_line.is_some() {
                new_cursor = new_cursor.saturating_add(1);
            }
        }

        let hunk_start = RowIndex(rows.len());
        let first_change = RowIndex(rows.len() + leading_context_len(hunk));
        push_unified_hunk_rows(
            &mut rows,
            &mut old_cursor,
            &mut new_cursor,
            hunk,
            input.head_blame,
            input.base_blame,
        );
        hunk_spans.push(HunkSpan {
            full_span: RowSpan {
                start: hunk_start,
                end: RowIndex(rows.len().saturating_sub(1).max(hunk_start.0)),
            },
            first_change,
        });
    }

    while let Some(text) = source_line(&head_lines, new_cursor) {
        rows.push(DocumentRow::Content(content_row(
            Some(old_cursor),
            Some(new_cursor),
            text,
            LineKind::Context,
            input.head_blame,
            input.base_blame,
        )));
        old_cursor = old_cursor.saturating_add(1);
        new_cursor = new_cursor.saturating_add(1);
    }

    Document::new(rows, hunk_spans)
}

fn build_full_file_document(
    content: Option<&str>,
    hunks: &[DiffHunk],
    side: CommentAnchorSide,
    blame: &[BlameLine],
) -> Document {
    let file_lines = split_content_lines(content);
    let mut hunk_spans = Vec::new();
    let changed_lines = changed_lines_for_side(hunks, side);
    let hunk_ranges = hunk_ranges_for_side(hunks, side);
    let mut active_hunk: Option<(usize, Option<usize>)> = None;
    let mut rows = Vec::new();

    for (idx, text) in file_lines.iter().enumerate() {
        let line = (idx + 1) as u32;
        for &(start, end) in &hunk_ranges {
            if line == start {
                active_hunk = Some((rows.len(), None));
            }
            if line == end
                && let Some((start_row, first_change)) = active_hunk.take()
            {
                hunk_spans.push(full_file_hunk_span(start_row, rows.len(), first_change));
            }
        }

        let changed = changed_lines.contains(&line);
        if changed && let Some((_, first_change)) = &mut active_hunk {
            first_change.get_or_insert(rows.len());
        }
        let kind = match (side, changed) {
            (CommentAnchorSide::Head, true) => LineKind::Addition,
            (CommentAnchorSide::Base, true) => LineKind::Deletion,
            _ => LineKind::Context,
        };
        rows.push(DocumentRow::Content(side_content_row(
            side, line, text, kind, blame,
        )));
    }
    if let Some((start_row, first_change)) = active_hunk.take() {
        hunk_spans.push(full_file_hunk_span(start_row, rows.len(), first_change));
    }

    Document::new(rows, hunk_spans)
}

fn full_file_hunk_span(start_row: usize, end_row: usize, first_change: Option<usize>) -> HunkSpan {
    HunkSpan {
        full_span: RowSpan {
            start: RowIndex(start_row),
            end: RowIndex(end_row.saturating_sub(1).max(start_row)),
        },
        first_change: RowIndex(first_change.unwrap_or(start_row)),
    }
}

fn build_side_by_side_structural_documents(input: &DiffDocumentInput<'_>) -> (Document, Document) {
    let base_lines = split_content_lines(input.base_content);
    let head_lines = split_content_lines(input.head_content);
    let mut base_rows = Vec::new();
    let mut head_rows = Vec::new();
    let mut hunk_spans = Vec::new();
    let mut old_cursor = 1;
    let mut new_cursor = 1;

    if input.hunks.is_empty() {
        while source_line(&base_lines, old_cursor).is_some()
            || source_line(&head_lines, new_cursor).is_some()
        {
            push_side_by_side_context_row(
                &mut base_rows,
                &mut head_rows,
                Some(old_cursor),
                Some(new_cursor),
                &base_lines,
                &head_lines,
                input.base_blame,
                input.head_blame,
            );
            old_cursor = old_cursor.saturating_add(1);
            new_cursor = new_cursor.saturating_add(1);
        }
        return (
            Document::new(base_rows, Vec::new()),
            Document::new(head_rows, Vec::new()),
        );
    }

    for hunk in input.hunks {
        while old_cursor < hunk.old_start || new_cursor < hunk.new_start {
            push_side_by_side_context_row(
                &mut base_rows,
                &mut head_rows,
                (old_cursor < hunk.old_start).then_some(old_cursor),
                (new_cursor < hunk.new_start).then_some(new_cursor),
                &base_lines,
                &head_lines,
                input.base_blame,
                input.head_blame,
            );
            if old_cursor < hunk.old_start {
                old_cursor = old_cursor.saturating_add(1);
            }
            if new_cursor < hunk.new_start {
                new_cursor = new_cursor.saturating_add(1);
            }
        }

        let hunk_start = RowIndex(base_rows.len());
        let first_change = RowIndex(base_rows.len() + leading_context_len(hunk));
        push_side_by_side_hunk_rows(
            &mut base_rows,
            &mut head_rows,
            &mut old_cursor,
            &mut new_cursor,
            hunk,
            input.base_blame,
            input.head_blame,
        );
        hunk_spans.push(HunkSpan {
            full_span: RowSpan {
                start: hunk_start,
                end: RowIndex(base_rows.len().saturating_sub(1).max(hunk_start.0)),
            },
            first_change,
        });
    }

    while source_line(&base_lines, old_cursor).is_some()
        || source_line(&head_lines, new_cursor).is_some()
    {
        push_side_by_side_context_row(
            &mut base_rows,
            &mut head_rows,
            source_line(&base_lines, old_cursor).map(|_| old_cursor),
            source_line(&head_lines, new_cursor).map(|_| new_cursor),
            &base_lines,
            &head_lines,
            input.base_blame,
            input.head_blame,
        );
        old_cursor = old_cursor.saturating_add(1);
        new_cursor = new_cursor.saturating_add(1);
    }

    (
        Document::new(base_rows, hunk_spans.clone()),
        Document::new(head_rows, hunk_spans),
    )
}

fn push_unified_hunk_rows(
    rows: &mut Vec<DocumentRow>,
    old_cursor: &mut u32,
    new_cursor: &mut u32,
    hunk: &DiffHunk,
    head_blame: &[BlameLine],
    base_blame: &[BlameLine],
) {
    let mut line_index = 0;
    while line_index < hunk.lines.len() {
        let line = &hunk.lines[line_index];
        if line.kind == LineKind::Context {
            rows.push(DocumentRow::Content(content_row(
                line.old_lineno.or(Some(*old_cursor)),
                line.new_lineno.or(Some(*new_cursor)),
                trimmed_diff_content(line),
                LineKind::Context,
                head_blame,
                base_blame,
            )));
            *old_cursor = old_cursor.saturating_add(1);
            *new_cursor = new_cursor.saturating_add(1);
            line_index += 1;
            continue;
        }

        let (deletion_end, addition_end) = paired_change_bounds(&hunk.lines, line_index);
        for line in &hunk.lines[line_index..deletion_end] {
            rows.push(DocumentRow::Content(content_row(
                line.old_lineno.or(Some(*old_cursor)),
                None,
                trimmed_diff_content(line),
                LineKind::Deletion,
                head_blame,
                base_blame,
            )));
            *old_cursor = old_cursor.saturating_add(1);
        }
        for line in &hunk.lines[deletion_end..addition_end] {
            rows.push(DocumentRow::Content(content_row(
                None,
                line.new_lineno.or(Some(*new_cursor)),
                trimmed_diff_content(line),
                LineKind::Addition,
                head_blame,
                base_blame,
            )));
            *new_cursor = new_cursor.saturating_add(1);
        }
        line_index = addition_end;
    }
}

fn push_side_by_side_hunk_rows(
    base_rows: &mut Vec<DocumentRow>,
    head_rows: &mut Vec<DocumentRow>,
    old_cursor: &mut u32,
    new_cursor: &mut u32,
    hunk: &DiffHunk,
    base_blame: &[BlameLine],
    head_blame: &[BlameLine],
) {
    let mut line_index = 0;
    while line_index < hunk.lines.len() {
        let line = &hunk.lines[line_index];
        if line.kind == LineKind::Context {
            let old_line = line.old_lineno.unwrap_or(*old_cursor);
            let new_line = line.new_lineno.unwrap_or(*new_cursor);
            let text = trimmed_diff_content(line);
            base_rows.push(DocumentRow::Content(side_content_row(
                CommentAnchorSide::Base,
                old_line,
                text,
                LineKind::Context,
                base_blame,
            )));
            head_rows.push(DocumentRow::Content(side_content_row(
                CommentAnchorSide::Head,
                new_line,
                text,
                LineKind::Context,
                head_blame,
            )));
            *old_cursor = old_cursor.saturating_add(1);
            *new_cursor = new_cursor.saturating_add(1);
            line_index += 1;
            continue;
        }

        let (deletion_end, addition_end) = paired_change_bounds(&hunk.lines, line_index);
        let deletions = &hunk.lines[line_index..deletion_end];
        let additions = &hunk.lines[deletion_end..addition_end];
        for idx in 0..deletions.len().max(additions.len()) {
            if let Some(line) = deletions.get(idx) {
                base_rows.push(DocumentRow::Content(side_content_row(
                    CommentAnchorSide::Base,
                    line.old_lineno.unwrap_or(*old_cursor),
                    trimmed_diff_content(line),
                    LineKind::Deletion,
                    base_blame,
                )));
                *old_cursor = old_cursor.saturating_add(1);
            } else {
                base_rows.push(DocumentRow::Spacer);
            }

            if let Some(line) = additions.get(idx) {
                head_rows.push(DocumentRow::Content(side_content_row(
                    CommentAnchorSide::Head,
                    line.new_lineno.unwrap_or(*new_cursor),
                    trimmed_diff_content(line),
                    LineKind::Addition,
                    head_blame,
                )));
                *new_cursor = new_cursor.saturating_add(1);
            } else {
                head_rows.push(DocumentRow::Spacer);
            }
        }
        line_index = addition_end;
    }
}

fn push_side_by_side_context_row(
    base_rows: &mut Vec<DocumentRow>,
    head_rows: &mut Vec<DocumentRow>,
    old_cursor: Option<u32>,
    new_cursor: Option<u32>,
    base_lines: &[&str],
    head_lines: &[&str],
    base_blame: &[BlameLine],
    head_blame: &[BlameLine],
) {
    match old_cursor.and_then(|line| source_line(base_lines, line).map(|text| (line, text))) {
        Some((line, text)) => base_rows.push(DocumentRow::Content(side_content_row(
            CommentAnchorSide::Base,
            line,
            text,
            LineKind::Context,
            base_blame,
        ))),
        None => base_rows.push(DocumentRow::Spacer),
    }
    match new_cursor.and_then(|line| source_line(head_lines, line).map(|text| (line, text))) {
        Some((line, text)) => head_rows.push(DocumentRow::Content(side_content_row(
            CommentAnchorSide::Head,
            line,
            text,
            LineKind::Context,
            head_blame,
        ))),
        None => head_rows.push(DocumentRow::Spacer),
    }
}

fn content_row(
    old_lineno: Option<u32>,
    new_lineno: Option<u32>,
    text: &str,
    kind: LineKind,
    head_blame: &[BlameLine],
    base_blame: &[BlameLine],
) -> ContentRow {
    ContentRow {
        gutter: Gutter {
            text: format_gutter(old_lineno, new_lineno),
        },
        kind,
        text: text.to_string(),
        blame: new_lineno
            .and_then(|line| blame_for_line(head_blame, line))
            .or_else(|| old_lineno.and_then(|line| blame_for_line(base_blame, line))),
        source: source_from_lines(old_lineno, new_lineno),
    }
}

fn side_content_row(
    side: CommentAnchorSide,
    line: u32,
    text: &str,
    kind: LineKind,
    blame: &[BlameLine],
) -> ContentRow {
    ContentRow {
        gutter: Gutter {
            text: line.to_string(),
        },
        kind,
        text: text.to_string(),
        blame: blame_for_line(blame, line),
        source: SourceLocation::single(side, line),
    }
}

fn format_gutter(old_lineno: Option<u32>, new_lineno: Option<u32>) -> String {
    match (old_lineno, new_lineno) {
        (Some(old), Some(new)) => format!("{old} {new}"),
        (Some(old), None) => old.to_string(),
        (None, Some(new)) => new.to_string(),
        (None, None) => String::new(),
    }
}

fn source_from_lines(old_lineno: Option<u32>, new_lineno: Option<u32>) -> SourceLocation {
    let mut lines = Vec::new();
    if let Some(line) = old_lineno {
        lines.push(SourceLine {
            side: CommentAnchorSide::Base,
            line,
        });
    }
    if let Some(line) = new_lineno {
        lines.push(SourceLine {
            side: CommentAnchorSide::Head,
            line,
        });
    }
    SourceLocation::from_lines(lines)
}

fn blame_for_line(blame: &[BlameLine], line: u32) -> Option<BlameInfo> {
    let index = usize::try_from(line.saturating_sub(1)).ok()?;
    blame.get(index).map(BlameInfo::from)
}

fn project_comments(
    rows: &[DocumentRow],
    file_path: &str,
    comments: &[Comment],
    selected_comment_id: Option<i64>,
    cursor: Option<RowIndex>,
) -> DocumentComments {
    let projected = comments
        .iter()
        .filter(|comment| comment.file_path() == file_path)
        .filter_map(|comment| project_comment(rows, comment))
        .collect();
    DocumentComments::with_selection(projected, selected_comment_id, cursor)
}

fn project_comment(rows: &[DocumentRow], comment: &Comment) -> Option<DocumentComment> {
    let mut matching_rows = Vec::new();
    for segment in comment
        .anchor()
        .segments
        .iter()
        .filter(|segment| segment.file_path == comment.file_path())
    {
        let start = u32::try_from(segment.line_start.max(1)).ok()?;
        let end = u32::try_from(segment.line_end.max(segment.line_start).max(1)).ok()?;
        matching_rows.extend(rows.iter().enumerate().filter_map(|(idx, row)| {
            let content = match row {
                DocumentRow::Content(content) => content,
                DocumentRow::Spacer => return None,
            };
            content
                .source
                .contains(segment.side, start, end)
                .then_some(RowIndex(idx))
        }));
    }

    matching_rows.sort();
    matching_rows.dedup();
    let start = *matching_rows.first()?;
    let end = *matching_rows.last().unwrap_or(&start);
    let span = RowSpan { start, end };
    Some(DocumentComment {
        id: comment.id,
        span,
        marker_rows: MarkerRows { start, end },
        resolved: comment.resolved,
        selected: false,
        current: false,
    })
}

fn current_comment_id_for_rows(
    comments: &[DocumentComment],
    selected_comment_id: Option<i64>,
    cursor: Option<RowIndex>,
) -> Option<i64> {
    let cursor = cursor?;
    let candidates: Vec<&DocumentComment> = comments
        .iter()
        .filter(|comment| comment.span.contains(cursor))
        .collect();
    if let Some(selected) = selected_comment_id
        .and_then(|id| candidates.iter().copied().find(|comment| comment.id == id))
    {
        return Some(selected.id);
    }
    preferred_comment(candidates).map(|comment| comment.id)
}

fn preferred_comment(candidates: Vec<&DocumentComment>) -> Option<&DocumentComment> {
    let max_start = candidates.iter().map(|comment| comment.span.start).max()?;
    candidates
        .into_iter()
        .filter(|comment| comment.span.start == max_start)
        .max_by_key(|comment| comment.id)
}

fn marker_kind(rows: MarkerRows, row: RowIndex) -> CommentMarkerKind {
    if rows.start == rows.end {
        CommentMarkerKind::SingleLine
    } else if row == rows.start {
        CommentMarkerKind::Start
    } else if row == rows.end {
        CommentMarkerKind::End
    } else {
        CommentMarkerKind::Join
    }
}

fn split_content_lines(content: Option<&str>) -> Vec<&str> {
    content
        .map(|content| content.lines().collect())
        .unwrap_or_default()
}

fn source_line<'a>(lines: &'a [&str], line: u32) -> Option<&'a str> {
    let index = usize::try_from(line.saturating_sub(1)).ok()?;
    lines.get(index).copied()
}

fn trimmed_diff_content(line: &DiffLine) -> &str {
    line.content.trim_end_matches('\n')
}

fn leading_context_len(hunk: &DiffHunk) -> usize {
    hunk.lines
        .iter()
        .take_while(|line| line.kind == LineKind::Context)
        .count()
}

fn paired_change_bounds(lines: &[DiffLine], start: usize) -> (usize, usize) {
    let mut deletion_end = start;
    while deletion_end < lines.len() && lines[deletion_end].kind == LineKind::Deletion {
        deletion_end += 1;
    }
    let mut addition_end = deletion_end;
    while addition_end < lines.len() && lines[addition_end].kind == LineKind::Addition {
        addition_end += 1;
    }
    (deletion_end, addition_end)
}

fn changed_lines_for_side(
    hunks: &[DiffHunk],
    side: CommentAnchorSide,
) -> std::collections::BTreeSet<u32> {
    hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .filter_map(|line| match (side, line.kind) {
            (CommentAnchorSide::Base, LineKind::Deletion) => line.old_lineno,
            (CommentAnchorSide::Head, LineKind::Addition) => line.new_lineno,
            _ => None,
        })
        .collect()
}

fn hunk_ranges_for_side(hunks: &[DiffHunk], side: CommentAnchorSide) -> Vec<(u32, u32)> {
    hunks
        .iter()
        .map(|hunk| match side {
            CommentAnchorSide::Base => (hunk.old_start, hunk.old_start + hunk.old_lines),
            CommentAnchorSide::Head => (hunk.new_start, hunk.new_start + hunk.new_lines),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_types::{
        AnchorMatchMethod, AnchorPlacementStatus, CommentAnchor, CommentAnchorSegment, CommentInit,
    };

    fn key(content_mode: ContentMode, render_variant: RenderVariant) -> DocumentKey {
        DocumentKey {
            file_id: "src/lib.rs".to_string(),
            diff_hash: "diff-a".to_string(),
            content_mode,
            render_variant,
            diff_algorithm: DiffAlgorithm::Myers,
            ignore_whitespace: false,
            head_content_id: Some(ContentId::from("head-a")),
            base_content_id: Some(ContentId::from("base-a")),
        }
    }

    fn input<'a>(
        key: DocumentKey,
        hunks: &'a [DiffHunk],
        base_content: Option<&'a str>,
        head_content: Option<&'a str>,
        comments: &'a [Comment],
    ) -> DiffDocumentInput<'a> {
        DiffDocumentInput {
            key,
            file_path: "src/lib.rs",
            hunks,
            head_content,
            base_content,
            head_blame: &[],
            base_blame: &[],
            comments,
            selected_comment_id: None,
            cursor: None,
        }
    }

    fn diff_line(
        kind: LineKind,
        content: &str,
        old_lineno: Option<u32>,
        new_lineno: Option<u32>,
    ) -> DiffLine {
        DiffLine {
            kind,
            content: content.to_string(),
            old_lineno,
            new_lineno,
        }
    }

    fn replacement_hunk() -> DiffHunk {
        DiffHunk {
            old_start: 2,
            old_lines: 1,
            new_start: 2,
            new_lines: 2,
            header: "@@ -2 +2,2 @@".to_string(),
            lines: vec![
                diff_line(LineKind::Deletion, "old", Some(2), None),
                diff_line(LineKind::Addition, "new one", None, Some(2)),
                diff_line(LineKind::Addition, "new two", None, Some(3)),
            ],
        }
    }

    fn segment(side: CommentAnchorSide, start: i64, end: i64) -> CommentAnchorSegment {
        CommentAnchorSegment {
            side,
            file_path: "src/lib.rs".to_string(),
            line_start: start,
            line_end: end,
            char_start: None,
            char_end: None,
            anchor_text: format!("{side:?}:{start}-{end}"),
            context_before: String::new(),
            context_after: String::new(),
            placement_status: AnchorPlacementStatus::Anchored,
            match_method: AnchorMatchMethod::ExactAtLine,
        }
    }

    fn comment(id: i64, resolved: bool, segments: Vec<CommentAnchorSegment>) -> Comment {
        Comment::new(CommentInit {
            id,
            merge_base: "base".to_string(),
            head_ref: "head".to_string(),
            created_head_commit: "head".to_string(),
            anchor: CommentAnchor::try_new(segments).expect("valid anchor"),
            body: format!("comment {id}"),
            resolved,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            anchor_status: crate::review_types::AnchorStatus::Anchored,
        })
        .expect("valid comment")
    }

    fn content(document: &Document, row: usize) -> &ContentRow {
        document.content_row(RowIndex(row)).expect("content row")
    }

    fn marker_kind_at(document: &Document, row: usize) -> Option<CommentMarkerKind> {
        document
            .overlays()
            .comments
            .marker_for_row(RowIndex(row))
            .kind()
    }

    #[test]
    fn document_key_changes_when_structural_inputs_change() {
        let base = key(ContentMode::Diff, RenderVariant::Inline);
        let mut changed = base.clone();
        changed.diff_hash = "diff-b".to_string();
        assert_ne!(base, changed);

        let mut changed = base.clone();
        changed.render_variant = RenderVariant::SideBySide;
        assert_ne!(base, changed);

        let mut changed = base.clone();
        changed.ignore_whitespace = true;
        assert_ne!(base, changed);
    }

    #[test]
    fn document_key_ignores_overlay_state_by_construction() {
        let base = key(ContentMode::Diff, RenderVariant::Inline);
        let comments = DocumentComments::new(vec![DocumentComment {
            id: 1,
            span: RowSpan {
                start: RowIndex(0),
                end: RowIndex(0),
            },
            marker_rows: MarkerRows {
                start: RowIndex(0),
                end: RowIndex(0),
            },
            resolved: false,
            selected: false,
            current: false,
        }]);
        let overlays = DocumentOverlays {
            comments,
            search: SearchOverlay {
                query: Some("needle".to_string()),
                matches: vec![SearchMatch {
                    row: RowIndex(0),
                    start: ColumnIndex(1),
                    end: ColumnIndex(3),
                }],
            },
            selection: Some(VisibleSelection::Line(RowSpan {
                start: RowIndex(0),
                end: RowIndex(1),
            })),
            cursor: DocumentCursor {
                row: RowIndex(4),
                column: ColumnIndex(2),
            },
        };
        let document = Document::new(Vec::new(), Vec::new()).with_overlays(overlays);

        assert_eq!(base, key(ContentMode::Diff, RenderVariant::Inline));
        assert_eq!(document.overlays().comments.selected_comment_id(), None);
    }

    #[test]
    fn unified_document_rows_preserve_order_and_source_metadata() {
        let hunk = replacement_hunk();
        let document = build_unified_document(&input(
            key(ContentMode::Diff, RenderVariant::Inline),
            &[hunk],
            Some("one\nold\nfour\n"),
            Some("one\nnew one\nnew two\nfour\n"),
            &[],
        ));

        assert_eq!(document.len(), 5);
        assert_eq!(content(&document, 0).text, "one");
        assert_eq!(content(&document, 1).text, "old");
        assert_eq!(content(&document, 2).text, "new one");
        assert_eq!(content(&document, 3).text, "new two");
        assert_eq!(content(&document, 4).text, "four");
        assert_eq!(content(&document, 1).kind, LineKind::Deletion);
        assert_eq!(content(&document, 2).kind, LineKind::Addition);
        assert!(
            content(&document, 0)
                .source
                .contains(CommentAnchorSide::Base, 1, 1)
        );
        assert!(
            content(&document, 0)
                .source
                .contains(CommentAnchorSide::Head, 1, 1)
        );
        assert!(
            content(&document, 1)
                .source
                .contains(CommentAnchorSide::Base, 2, 2)
        );
        assert!(
            content(&document, 2)
                .source
                .contains(CommentAnchorSide::Head, 2, 2)
        );
    }

    #[test]
    fn full_file_documents_contain_only_their_side_source_lines() {
        let hunk = replacement_hunk();
        let head = build_head_document(&input(
            key(ContentMode::FullFile, RenderVariant::HeadVersion),
            std::slice::from_ref(&hunk),
            Some("one\nold\nfour\n"),
            Some("one\nnew one\nnew two\nfour\n"),
            &[],
        ));
        let base = build_base_document(&input(
            key(ContentMode::FullFile, RenderVariant::BaseVersion),
            &[hunk],
            Some("one\nold\nfour\n"),
            Some("one\nnew one\nnew two\nfour\n"),
            &[],
        ));

        assert_eq!(head.len(), 4);
        assert_eq!(base.len(), 3);
        assert_eq!(
            content(&head, 1)
                .source
                .line_for_side(CommentAnchorSide::Head),
            Some(2)
        );
        assert_eq!(
            content(&head, 1)
                .source
                .line_for_side(CommentAnchorSide::Base),
            None
        );
        assert_eq!(
            content(&base, 1)
                .source
                .line_for_side(CommentAnchorSide::Base),
            Some(2)
        );
        assert_eq!(
            content(&base, 1)
                .source
                .line_for_side(CommentAnchorSide::Head),
            None
        );
        assert_eq!(head.hunk_spans()[0].first_change, RowIndex(1));
        assert_eq!(base.hunk_spans()[0].first_change, RowIndex(1));
    }

    #[test]
    fn side_by_side_documents_insert_spacers_for_replacements() {
        let hunk = replacement_hunk();
        let document = build_side_by_side_document(&input(
            key(ContentMode::Diff, RenderVariant::SideBySide),
            &[hunk],
            Some("one\nold\nfour\n"),
            Some("one\nnew one\nnew two\nfour\n"),
            &[],
        ))
        .expect("valid side-by-side document");

        assert_eq!(document.base().len(), document.head().len());
        assert_eq!(content(document.base(), 1).text, "old");
        assert_eq!(content(document.head(), 1).text, "new one");
        assert!(matches!(
            document.base().row(RowIndex(2)),
            Some(DocumentRow::Spacer)
        ));
        assert_eq!(content(document.head(), 2).text, "new two");
        assert_eq!(document.base().content_row(RowIndex(2)), None);
    }

    #[test]
    fn side_by_side_documents_render_shared_tail_after_offset_hunk() {
        let mut hunk_lines = vec![
            diff_line(LineKind::Deletion, "old changed", Some(18), None),
            diff_line(LineKind::Addition, "new changed", None, Some(20)),
        ];
        for offset in 0..8 {
            hunk_lines.push(diff_line(
                LineKind::Context,
                &format!("context {offset}"),
                Some(19 + offset),
                Some(21 + offset),
            ));
        }
        hunk_lines.push(diff_line(
            LineKind::Addition,
            "new trailing addition",
            None,
            Some(29),
        ));
        let hunk = DiffHunk {
            old_start: 18,
            old_lines: 9,
            new_start: 20,
            new_lines: 10,
            header: "@@ -18,9 +20,10 @@".to_string(),
            lines: hunk_lines,
        };
        let mut base_lines: Vec<String> = (1..=90).map(|n| format!("base {n}")).collect();
        let mut head_lines: Vec<String> = (1..=93).map(|n| format!("head {n}")).collect();
        for base_lineno in 27..=90 {
            let head_lineno = base_lineno + 3;
            let text = format!("shared {base_lineno}/{head_lineno}");
            base_lines[(base_lineno - 1) as usize] = text.clone();
            head_lines[(head_lineno - 1) as usize] = text;
        }
        let base_content = format!("{}\n", base_lines.join("\n"));
        let head_content = format!("{}\n", head_lines.join("\n"));

        let document = build_side_by_side_document(&input(
            key(ContentMode::Diff, RenderVariant::SideBySide),
            &[hunk],
            Some(&base_content),
            Some(&head_content),
            &[],
        ))
        .expect("valid side-by-side document");
        let row = (0..document.len())
            .find(|row| {
                document
                    .base()
                    .content_row(RowIndex(*row))
                    .and_then(|content| content.source.line_for_side(CommentAnchorSide::Base))
                    == Some(27)
                    && document
                        .head()
                        .content_row(RowIndex(*row))
                        .and_then(|content| content.source.line_for_side(CommentAnchorSide::Head))
                        == Some(30)
            })
            .expect("shared tail row");

        assert_eq!(content(document.base(), row).text, "shared 27/30");
        assert_eq!(content(document.head(), row).text, "shared 27/30");
    }

    #[test]
    fn hunk_navigation_uses_first_change_rows() {
        let hunks = vec![
            DiffHunk {
                old_start: 2,
                old_lines: 2,
                new_start: 2,
                new_lines: 2,
                header: "@@ -2,2 +2,2 @@".to_string(),
                lines: vec![
                    diff_line(LineKind::Context, "same", Some(2), Some(2)),
                    diff_line(LineKind::Deletion, "old", Some(3), None),
                    diff_line(LineKind::Addition, "new", None, Some(3)),
                ],
            },
            DiffHunk {
                old_start: 5,
                old_lines: 1,
                new_start: 5,
                new_lines: 1,
                header: "@@ -5 +5 @@".to_string(),
                lines: vec![
                    diff_line(LineKind::Deletion, "old two", Some(5), None),
                    diff_line(LineKind::Addition, "new two", None, Some(5)),
                ],
            },
        ];
        let document = build_unified_document(&input(
            key(ContentMode::Diff, RenderVariant::Inline),
            &hunks,
            Some("1\nsame\nold\n4\nold two\n"),
            Some("1\nsame\nnew\n4\nnew two\n"),
            &[],
        ));

        assert_eq!(document.hunk_spans()[0].first_change, RowIndex(2));
        assert_eq!(
            document.next_hunk(RowIndex(0), Direction::Next),
            Some(RowIndex(2))
        );
        assert_eq!(
            document.next_hunk(RowIndex(3), Direction::Next),
            Some(RowIndex(5))
        );
        assert_eq!(
            document.next_hunk(RowIndex(5), Direction::Prev),
            Some(RowIndex(2))
        );
    }

    #[test]
    fn blame_is_attached_only_when_supplied() {
        let hunk = replacement_hunk();
        let hunks = vec![hunk];
        let head_blame = vec![
            BlameLine {
                hash: "aaaaaaa".to_string(),
                author: "Ada".to_string(),
                date: "2026-01-01".to_string(),
            },
            BlameLine {
                hash: "bbbbbbb".to_string(),
                author: "Bea".to_string(),
                date: "2026-01-02".to_string(),
            },
        ];
        let mut input = input(
            key(ContentMode::Diff, RenderVariant::Inline),
            &hunks,
            Some("one\nold\n"),
            Some("one\nnew one\nnew two\n"),
            &[],
        );
        input.head_blame = &head_blame;

        let document = build_unified_document(&input);

        assert_eq!(
            content(&document, 2)
                .blame
                .as_ref()
                .map(|blame| blame.author.as_str()),
            Some("Bea")
        );
        assert_eq!(content(&document, 1).blame, None);
    }

    #[test]
    fn document_comments_project_into_each_document_variant() {
        let hunk = replacement_hunk();
        let comments = vec![comment(
            7,
            false,
            vec![
                segment(CommentAnchorSide::Base, 2, 2),
                segment(CommentAnchorSide::Head, 2, 3),
            ],
        )];
        let unified = build_unified_document(&input(
            key(ContentMode::Diff, RenderVariant::Inline),
            std::slice::from_ref(&hunk),
            Some("one\nold\nfour\n"),
            Some("one\nnew one\nnew two\nfour\n"),
            &comments,
        ));
        let head = build_head_document(&input(
            key(ContentMode::FullFile, RenderVariant::HeadVersion),
            std::slice::from_ref(&hunk),
            Some("one\nold\nfour\n"),
            Some("one\nnew one\nnew two\nfour\n"),
            &comments,
        ));
        let base = build_base_document(&input(
            key(ContentMode::FullFile, RenderVariant::BaseVersion),
            std::slice::from_ref(&hunk),
            Some("one\nold\nfour\n"),
            Some("one\nnew one\nnew two\nfour\n"),
            &comments,
        ));
        let side_by_side = build_side_by_side_document(&input(
            key(ContentMode::Diff, RenderVariant::SideBySide),
            &[hunk],
            Some("one\nold\nfour\n"),
            Some("one\nnew one\nnew two\nfour\n"),
            &comments,
        ))
        .expect("valid side-by-side document");

        assert_eq!(
            unified.comment_span(7),
            Some(RowSpan {
                start: RowIndex(1),
                end: RowIndex(3)
            })
        );
        assert_eq!(
            head.comment_span(7),
            Some(RowSpan {
                start: RowIndex(1),
                end: RowIndex(2)
            })
        );
        assert_eq!(
            base.comment_span(7),
            Some(RowSpan {
                start: RowIndex(1),
                end: RowIndex(1)
            })
        );
        assert_eq!(
            side_by_side.base().comment_span(7),
            Some(RowSpan {
                start: RowIndex(1),
                end: RowIndex(1)
            })
        );
        assert_eq!(
            side_by_side.head().comment_span(7),
            Some(RowSpan {
                start: RowIndex(1),
                end: RowIndex(2)
            })
        );
    }

    #[test]
    fn side_by_side_document_rejects_mismatched_row_counts() {
        let base = Document::new(Vec::new(), Vec::new());
        let head = Document::new(
            vec![DocumentRow::Content(side_content_row(
                CommentAnchorSide::Head,
                1,
                "head",
                LineKind::Context,
                &[],
            ))],
            Vec::new(),
        );

        assert_eq!(
            SideBySideDocument::new(base, head),
            Err(SideBySideDocumentError {
                base_len: 0,
                head_len: 1
            })
        );
    }

    #[test]
    fn comment_markers_cover_single_multiline_nested_and_overlap_cases() {
        let rows = vec![
            DocumentRow::Content(side_content_row(
                CommentAnchorSide::Head,
                1,
                "one",
                LineKind::Context,
                &[],
            )),
            DocumentRow::Content(side_content_row(
                CommentAnchorSide::Head,
                2,
                "two",
                LineKind::Context,
                &[],
            )),
            DocumentRow::Content(side_content_row(
                CommentAnchorSide::Head,
                3,
                "three",
                LineKind::Context,
                &[],
            )),
            DocumentRow::Content(side_content_row(
                CommentAnchorSide::Head,
                4,
                "four",
                LineKind::Context,
                &[],
            )),
        ];
        let comments = vec![
            comment(1, false, vec![segment(CommentAnchorSide::Head, 1, 1)]),
            comment(2, true, vec![segment(CommentAnchorSide::Head, 2, 4)]),
            comment(3, false, vec![segment(CommentAnchorSide::Head, 3, 3)]),
        ];
        let overlays = DocumentOverlays {
            comments: project_comments(&rows, "src/lib.rs", &comments, Some(2), Some(RowIndex(2))),
            ..DocumentOverlays::default()
        };
        let document = Document::new(rows, Vec::new()).with_overlays(overlays);

        assert_eq!(
            marker_kind_at(&document, 0),
            Some(CommentMarkerKind::SingleLine)
        );
        assert_eq!(marker_kind_at(&document, 1), Some(CommentMarkerKind::Start));
        assert_eq!(marker_kind_at(&document, 2), Some(CommentMarkerKind::Join));
        assert_eq!(marker_kind_at(&document, 3), Some(CommentMarkerKind::End));
        assert!(
            document
                .overlays()
                .comments
                .marker_for_row(RowIndex(2))
                .is_current()
        );
        assert_eq!(
            document
                .overlays()
                .comments
                .markers_for_row(RowIndex(2))
                .len(),
            2
        );
    }

    #[test]
    fn same_start_comments_are_reachable_with_next_and_previous() {
        let rows = vec![
            DocumentRow::Content(side_content_row(
                CommentAnchorSide::Head,
                1,
                "one",
                LineKind::Context,
                &[],
            )),
            DocumentRow::Content(side_content_row(
                CommentAnchorSide::Head,
                2,
                "two",
                LineKind::Context,
                &[],
            )),
        ];
        let comments = vec![
            comment(1, false, vec![segment(CommentAnchorSide::Head, 1, 1)]),
            comment(2, false, vec![segment(CommentAnchorSide::Head, 1, 2)]),
            comment(3, false, vec![segment(CommentAnchorSide::Head, 2, 2)]),
        ];
        let document_comments =
            project_comments(&rows, "src/lib.rs", &comments, Some(1), Some(RowIndex(0)));

        assert_eq!(
            document_comments
                .next_comment(RowIndex(0), Direction::Next)
                .map(|comment| comment.id),
            Some(2)
        );
        let document_comments =
            DocumentComments::with_selection(document_comments.all().to_vec(), Some(3), None);
        assert_eq!(
            document_comments
                .next_comment(RowIndex(1), Direction::Prev)
                .map(|comment| comment.id),
            Some(2)
        );
    }

    #[test]
    fn next_unresolved_comment_returns_none_when_all_are_resolved() {
        let comments = DocumentComments::new(vec![DocumentComment {
            id: 1,
            span: RowSpan {
                start: RowIndex(0),
                end: RowIndex(0),
            },
            marker_rows: MarkerRows {
                start: RowIndex(0),
                end: RowIndex(0),
            },
            resolved: true,
            selected: false,
            current: false,
        }]);

        assert_eq!(
            comments
                .next_unresolved_comment(RowIndex(0), Direction::Next)
                .map(|comment| comment.id),
            None
        );
    }

    #[test]
    fn document_search_ignores_gutter_and_highlights_content_runs() {
        let mut document = Document::new(
            vec![DocumentRow::Content(ContentRow {
                gutter: Gutter {
                    text: "42".to_string(),
                },
                kind: LineKind::Context,
                text: "needle hay needle".to_string(),
                blame: None,
                source: SourceLocation::single(CommentAnchorSide::Head, 42),
            })],
            Vec::new(),
        );

        document.search("42");
        assert!(document.overlays().search.matches.is_empty());

        document.search("needle");
        assert_eq!(
            document.overlays().search.matches,
            vec![
                SearchMatch {
                    row: RowIndex(0),
                    start: ColumnIndex(0),
                    end: ColumnIndex(6),
                },
                SearchMatch {
                    row: RowIndex(0),
                    start: ColumnIndex(11),
                    end: ColumnIndex(17),
                },
            ]
        );
        let RenderLine::Content(rendered) = document.line(RowIndex(0)).expect("render line") else {
            panic!("expected content render line");
        };
        assert_eq!(rendered.runs[0].kind, TextRunKind::SearchMatch);
        assert_eq!(rendered.runs[1].kind, TextRunKind::Plain);
        assert_eq!(rendered.runs[2].kind, TextRunKind::SearchMatch);
    }

    #[test]
    fn selection_and_source_metadata_support_anchor_capture() {
        let hunk = replacement_hunk();
        let mut document = build_unified_document(&input(
            key(ContentMode::Diff, RenderVariant::Inline),
            &[hunk],
            Some("one\nold\nfour\n"),
            Some("one\nnew one\nnew two\nfour\n"),
            &[],
        ));
        document.overlays_mut().selection = Some(VisibleSelection::Text {
            start: DocumentPosition {
                row: RowIndex(1),
                column: ColumnIndex(0),
            },
            end: DocumentPosition {
                row: RowIndex(3),
                column: ColumnIndex(4),
            },
        });

        assert!(
            content(&document, 1)
                .source
                .contains(CommentAnchorSide::Base, 2, 2)
        );
        assert!(
            content(&document, 2)
                .source
                .contains(CommentAnchorSide::Head, 2, 2)
        );
        assert!(
            content(&document, 3)
                .source
                .contains(CommentAnchorSide::Head, 3, 3)
        );
        assert!(matches!(
            document.overlays().selection,
            Some(VisibleSelection::Text { .. })
        ));
    }
}
