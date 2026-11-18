use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

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

impl ActiveDocument {
    pub fn refresh_overlays(
        &mut self,
        file_path: &str,
        comments: &[Comment],
        selected_comment_id: Option<i64>,
        cursor: Option<RowIndex>,
        search_query: Option<&str>,
    ) {
        self.diff.refresh_overlays(
            file_path,
            comments,
            selected_comment_id,
            cursor,
            search_query,
        );
    }
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

    pub fn hunk_spans(&self) -> &[HunkSpan] {
        match self {
            Self::Unified(document) | Self::Base(document) | Self::Head(document) => {
                document.hunk_spans()
            }
            Self::SideBySide(document) => document.hunk_spans(),
        }
    }

    pub fn search(&mut self, query: impl Into<String>) {
        let query = query.into();
        match self {
            Self::Unified(document) | Self::Base(document) | Self::Head(document) => {
                document.search(query)
            }
            Self::SideBySide(document) => document.search(query),
        }
    }

    fn refresh_overlays(
        &mut self,
        file_path: &str,
        comments: &[Comment],
        selected_comment_id: Option<i64>,
        cursor: Option<RowIndex>,
        search_query: Option<&str>,
    ) {
        match self {
            Self::Unified(document) | Self::Base(document) | Self::Head(document) => {
                refresh_document_overlays(
                    document,
                    None,
                    file_path,
                    comments,
                    selected_comment_id,
                    cursor,
                    search_query,
                );
            }
            Self::SideBySide(document) => {
                document.refresh_overlays(
                    file_path,
                    comments,
                    selected_comment_id,
                    cursor,
                    search_query,
                );
            }
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

    pub fn hunk_spans(&self) -> &[HunkSpan] {
        self.head.hunk_spans()
    }

    pub fn search(&mut self, query: impl Into<String>) {
        let query = query.into();
        self.base.search(query.clone());
        self.head.search(query);
    }

    fn refresh_overlays(
        &mut self,
        file_path: &str,
        comments: &[Comment],
        selected_comment_id: Option<i64>,
        cursor: Option<RowIndex>,
        search_query: Option<&str>,
    ) {
        let marker_source_rows = SourceRowIndex::aligned(self.base.rows(), self.head.rows());
        refresh_document_overlays(
            &mut self.base,
            Some(&marker_source_rows),
            file_path,
            comments,
            selected_comment_id,
            cursor,
            search_query,
        );
        refresh_document_overlays(
            &mut self.head,
            Some(&marker_source_rows),
            file_path,
            comments,
            selected_comment_id,
            cursor,
            search_query,
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    rows: Arc<Vec<DocumentRow>>,
    hunk_spans: Arc<Vec<HunkSpan>>,
    overlays: DocumentOverlays,
}

impl Document {
    pub fn new(rows: Vec<DocumentRow>, hunk_spans: Vec<HunkSpan>) -> Self {
        let overlays = DocumentOverlays::default();
        Self {
            rows: Arc::new(rows),
            hunk_spans: Arc::new(hunk_spans),
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
        self.rows.as_slice()
    }

    pub fn content_row(&self, row: RowIndex) -> Option<&ContentRow> {
        match self.row(row)? {
            DocumentRow::Content(content) => Some(content),
            DocumentRow::Spacer => None,
        }
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
            DocumentRow::Spacer => Some(RenderLine::Spacer {
                marker: self.overlays.comments.marker_for_row(row),
            }),
        }
    }

    pub fn overlays(&self) -> &DocumentOverlays {
        &self.overlays
    }

    pub fn overlays_mut(&mut self) -> &mut DocumentOverlays {
        &mut self.overlays
    }

    pub fn hunk_spans(&self) -> &[HunkSpan] {
        self.hunk_spans.as_slice()
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
        self.overlays.search = SearchOverlay::new(query.into());
    }

    pub fn clear_search(&mut self) {
        self.overlays.search = SearchOverlay::default();
    }

    pub fn search_matches(&self, row: RowIndex) -> impl Iterator<Item = SearchMatch> + '_ {
        self.content_row(row).into_iter().flat_map(move |content| {
            self.overlays
                .search
                .match_spans_for(row, content.text.as_str())
        })
    }

    pub fn all_search_matches(&self) -> impl Iterator<Item = SearchMatch> + '_ {
        self.rows
            .iter()
            .enumerate()
            .filter_map(|(row, document_row)| match document_row {
                DocumentRow::Content(content) => Some((RowIndex(row), content)),
                DocumentRow::Spacer => None,
            })
            .flat_map(|(row, content)| {
                self.overlays
                    .search
                    .match_spans_for(row, content.text.as_str())
            })
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceLocation {
    pub base: Option<u32>,
    pub head: Option<u32>,
}

impl SourceLocation {
    pub fn none() -> Self {
        Self {
            base: None,
            head: None,
        }
    }

    pub fn single(side: CommentAnchorSide, line: u32) -> Self {
        match side {
            CommentAnchorSide::Base => Self {
                base: Some(line),
                head: None,
            },
            CommentAnchorSide::Head => Self {
                base: None,
                head: Some(line),
            },
        }
    }

    pub fn paired(base: Option<u32>, head: Option<u32>) -> Self {
        Self { base, head }
    }

    pub fn has_line_in_range(&self, side: CommentAnchorSide, start: u32, end: u32) -> bool {
        self.line_for_side(side)
            .is_some_and(|line| line >= start && line <= end)
    }

    pub fn line_for_side(&self, side: CommentAnchorSide) -> Option<u32> {
        match side {
            CommentAnchorSide::Base => self.base,
            CommentAnchorSide::Head => self.head,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct SourceRowIndex {
    base_rows: BTreeMap<u32, Vec<RowIndex>>,
    head_rows: BTreeMap<u32, Vec<RowIndex>>,
}

impl SourceRowIndex {
    fn new(rows: &[DocumentRow]) -> Self {
        let mut index = Self::default();
        for (row, document_row) in rows.iter().enumerate() {
            let DocumentRow::Content(content) = document_row else {
                continue;
            };
            index.insert_source(RowIndex(row), content.source);
        }
        index
    }

    fn aligned(base_rows: &[DocumentRow], head_rows: &[DocumentRow]) -> Self {
        let mut index = Self::default();
        for row in 0..base_rows.len().max(head_rows.len()) {
            if let Some(DocumentRow::Content(content)) = base_rows.get(row) {
                index.insert_source(RowIndex(row), content.source);
            }
            if let Some(DocumentRow::Content(content)) = head_rows.get(row) {
                index.insert_source(RowIndex(row), content.source);
            }
        }
        index
    }

    fn insert_source(&mut self, row: RowIndex, source: SourceLocation) {
        if let Some(line) = source.base {
            self.base_rows.entry(line).or_default().push(row);
        }
        if let Some(line) = source.head {
            self.head_rows.entry(line).or_default().push(row);
        }
    }

    fn rows_for_range(
        &self,
        side: CommentAnchorSide,
        start: u32,
        end: u32,
    ) -> impl Iterator<Item = RowIndex> + '_ {
        self.rows_for_side(side)
            .range(start..=end)
            .flat_map(|(_, rows)| rows.iter().copied())
    }

    fn rows_for_side(&self, side: CommentAnchorSide) -> &BTreeMap<u32, Vec<RowIndex>> {
        match side {
            CommentAnchorSide::Base => &self.base_rows,
            CommentAnchorSide::Head => &self.head_rows,
        }
    }
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
    comments_by_id: HashMap<i64, usize>,
    unresolved_comment_indices: Vec<usize>,
    unresolved_comment_positions_by_id: HashMap<i64, usize>,
    comments_by_row: BTreeMap<RowIndex, Vec<usize>>,
    marker_comments_by_row: BTreeMap<RowIndex, Vec<usize>>,
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
        let comments_by_id = comments
            .iter()
            .enumerate()
            .map(|(index, comment)| (comment.id, index))
            .collect();
        let unresolved_comment_indices: Vec<usize> = comments
            .iter()
            .enumerate()
            .filter_map(|(index, comment)| (!comment.resolved).then_some(index))
            .collect();
        let unresolved_comment_positions_by_id = unresolved_comment_indices
            .iter()
            .enumerate()
            .map(|(position, comment_index)| (comments[*comment_index].id, position))
            .collect();
        let current_comment_id =
            current_comment_id_for_rows(&comments, selected_comment_id, cursor);
        for comment in &mut comments {
            comment.selected = Some(comment.id) == selected_comment_id;
            comment.current = Some(comment.id) == current_comment_id;
        }
        let comments_by_row =
            row_index_for_comments(&comments, |comment| (comment.span.start, comment.span.end));
        let marker_comments_by_row = row_index_for_comments(&comments, |comment| {
            (comment.marker_rows.start, comment.marker_rows.end)
        });
        Self {
            comments_by_id,
            unresolved_comment_indices,
            unresolved_comment_positions_by_id,
            comments_by_row,
            marker_comments_by_row,
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
        self.comment_by_id(id).map(|comment| comment.span)
    }

    pub fn comment_by_id(&self, id: i64) -> Option<&DocumentComment> {
        self.comments_by_id
            .get(&id)
            .and_then(|index| self.comments.get(*index))
    }

    pub fn current_comment_at(&self, row: RowIndex) -> Option<&DocumentComment> {
        if let Some(selected) = self
            .selected_comment_id
            .and_then(|id| self.comments_at(row).find(|comment| comment.id == id))
        {
            return Some(selected);
        }
        if let Some(current) = self
            .current_comment_id
            .and_then(|id| self.comments_at(row).find(|comment| comment.id == id))
        {
            return Some(current);
        }
        preferred_comment(self.comments_at(row))
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
        self.marker_comments_at(row)
            .into_iter()
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
        let selected_position = self
            .current_comment_id
            .or(self.selected_comment_id)
            .and_then(|id| self.comments_by_id.get(&id).copied());
        self.next_in_comment_order(row, direction, None, selected_position)
    }

    pub fn next_unresolved_comment(
        &self,
        row: RowIndex,
        direction: Direction,
    ) -> Option<&DocumentComment> {
        let selected_position = self
            .current_comment_id
            .or(self.selected_comment_id)
            .and_then(|id| self.unresolved_comment_positions_by_id.get(&id).copied());
        self.next_in_comment_order(
            row,
            direction,
            Some(&self.unresolved_comment_indices),
            selected_position,
        )
    }

    fn next_in_comment_order(
        &self,
        row: RowIndex,
        direction: Direction,
        indices: Option<&[usize]>,
        selected_position: Option<usize>,
    ) -> Option<&DocumentComment> {
        let ordered_len = indices.map_or(self.comments.len(), <[usize]>::len);
        if ordered_len == 0 {
            return None;
        }

        match (direction, selected_position) {
            (Direction::Next, Some(position)) => self
                .comment_at_order_position(indices, position.saturating_add(1))
                .or_else(|| self.first_comment_starting_after(indices, row)),
            (Direction::Prev, Some(position)) => position
                .checked_sub(1)
                .and_then(|previous| self.comment_at_order_position(indices, previous))
                .or_else(|| self.last_comment_starting_before_or_containing(indices, row)),
            (Direction::Next, None) => self.first_comment_starting_after(indices, row),
            (Direction::Prev, None) => self.last_comment_starting_before(indices, row),
        }
    }

    fn comment_at_order_position(
        &self,
        indices: Option<&[usize]>,
        position: usize,
    ) -> Option<&DocumentComment> {
        match indices {
            Some(indices) => indices
                .get(position)
                .and_then(|comment_index| self.comments.get(*comment_index)),
            None => self.comments.get(position),
        }
    }

    fn first_comment_starting_after(
        &self,
        indices: Option<&[usize]>,
        row: RowIndex,
    ) -> Option<&DocumentComment> {
        let position = self.partition_comment_order(indices, |comment| comment.span.start <= row);
        self.comment_at_order_position(indices, position)
    }

    fn last_comment_starting_before(
        &self,
        indices: Option<&[usize]>,
        row: RowIndex,
    ) -> Option<&DocumentComment> {
        let position = self.partition_comment_order(indices, |comment| comment.span.start < row);
        position
            .checked_sub(1)
            .and_then(|position| self.comment_at_order_position(indices, position))
    }

    fn last_comment_starting_before_or_containing(
        &self,
        indices: Option<&[usize]>,
        row: RowIndex,
    ) -> Option<&DocumentComment> {
        let position = self.partition_comment_order(indices, |comment| {
            comment.span.start < row || comment.span.contains(row)
        });
        position
            .checked_sub(1)
            .and_then(|position| self.comment_at_order_position(indices, position))
    }

    fn partition_comment_order(
        &self,
        indices: Option<&[usize]>,
        predicate: impl Fn(&DocumentComment) -> bool,
    ) -> usize {
        match indices {
            Some(indices) => indices.partition_point(|comment_index| {
                self.comments.get(*comment_index).is_some_and(&predicate)
            }),
            None => self.comments.partition_point(predicate),
        }
    }

    fn current_marker_comment(&self, row: RowIndex) -> Option<&DocumentComment> {
        preferred_marker_comment(self.marker_comments_at(row), row, self.current_comment_id)
    }

    fn comments_at(&self, row: RowIndex) -> impl Iterator<Item = &DocumentComment> {
        comments_for_row(&self.comments, &self.comments_by_row, row)
    }

    fn marker_comments_at(&self, row: RowIndex) -> impl Iterator<Item = &DocumentComment> {
        comments_for_row(&self.comments, &self.marker_comments_by_row, row)
    }
}

fn row_index_for_comments(
    comments: &[DocumentComment],
    rows: impl Fn(&DocumentComment) -> (RowIndex, RowIndex),
) -> BTreeMap<RowIndex, Vec<usize>> {
    let mut index = BTreeMap::new();
    for (comment_index, comment) in comments.iter().enumerate() {
        let (start, end) = rows(comment);
        for row in start.0..=end.0 {
            index
                .entry(RowIndex(row))
                .or_insert_with(Vec::new)
                .push(comment_index);
        }
    }
    index
}

fn comments_for_row<'a>(
    comments: &'a [DocumentComment],
    index: &'a BTreeMap<RowIndex, Vec<usize>>,
    row: RowIndex,
) -> impl Iterator<Item = &'a DocumentComment> {
    index
        .get(&row)
        .into_iter()
        .flat_map(|comment_indexes| comment_indexes.iter())
        .filter_map(|comment_index| comments.get(*comment_index))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowCommentMarker {
    pub comment_id: i64,
    pub marker: CommentMarker,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchOverlay {
    pub query: Option<String>,
}

impl SearchOverlay {
    pub fn new(query: String) -> Self {
        if query.is_empty() {
            return Self::default();
        }
        Self { query: Some(query) }
    }

    fn match_spans_for<'a>(
        &'a self,
        row: RowIndex,
        text: &'a str,
    ) -> impl Iterator<Item = SearchMatch> + 'a {
        self.query
            .as_deref()
            .into_iter()
            .flat_map(move |query| text.match_indices(query))
            .map(move |(start, value)| SearchMatch {
                row,
                columns: ColumnSpan {
                    start: ColumnIndex(start),
                    end: ColumnIndex(start + value.len()),
                },
            })
    }

    fn render_runs_for<'a>(&'a self, row: RowIndex, text: &'a str) -> Vec<TextRun<'a>> {
        if self.query.is_none() {
            return vec![TextRun {
                text: Cow::Borrowed(text),
                kind: TextRunKind::Plain,
            }];
        }

        let mut runs = Vec::new();
        let mut offset = 0;
        for span in self.match_spans_for(row, text) {
            if span.columns.start.0 > offset {
                runs.push(TextRun {
                    text: Cow::Borrowed(&text[offset..span.columns.start.0]),
                    kind: TextRunKind::Plain,
                });
            }
            runs.push(TextRun {
                text: Cow::Borrowed(&text[span.columns.start.0..span.columns.end.0]),
                kind: TextRunKind::SearchMatch,
            });
            offset = span.columns.end.0;
        }
        if runs.is_empty() {
            return vec![TextRun {
                text: Cow::Borrowed(text),
                kind: TextRunKind::Plain,
            }];
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
    pub columns: ColumnSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnSpan {
    pub start: ColumnIndex,
    pub end: ColumnIndex,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderLine<'a> {
    Content(RenderContent<'a>),
    Spacer { marker: CommentMarker },
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
    refresh_document_overlays(
        &mut document,
        None,
        input.file_path,
        input.comments,
        input.selected_comment_id,
        input.cursor,
        None,
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
    refresh_document_overlays(
        &mut document,
        None,
        input.file_path,
        input.comments,
        input.selected_comment_id,
        input.cursor,
        None,
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
    refresh_document_overlays(
        &mut document,
        None,
        input.file_path,
        input.comments,
        input.selected_comment_id,
        input.cursor,
        None,
    );
    document
}

pub fn build_side_by_side_document(
    input: &DiffDocumentInput<'_>,
) -> Result<SideBySideDocument, SideBySideDocumentError> {
    let (mut base, mut head) = build_side_by_side_structural_documents(input);
    let marker_source_rows = SourceRowIndex::aligned(base.rows(), head.rows());
    refresh_document_overlays(
        &mut base,
        Some(&marker_source_rows),
        input.file_path,
        input.comments,
        input.selected_comment_id,
        input.cursor,
        None,
    );
    refresh_document_overlays(
        &mut head,
        Some(&marker_source_rows),
        input.file_path,
        input.comments,
        input.selected_comment_id,
        input.cursor,
        None,
    );
    SideBySideDocument::new(base, head)
}

fn refresh_document_overlays(
    document: &mut Document,
    marker_source_rows: Option<&SourceRowIndex>,
    file_path: &str,
    comments: &[Comment],
    selected_comment_id: Option<i64>,
    cursor: Option<RowIndex>,
    search_query: Option<&str>,
) {
    document.overlays.comments = project_comments(
        document.rows(),
        marker_source_rows,
        file_path,
        comments,
        selected_comment_id,
        cursor,
    );
    if let Some(query) = search_query {
        document.search(query.to_string());
    } else {
        document.clear_search();
    }
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
    SourceLocation::paired(old_lineno, new_lineno)
}

fn blame_for_line(blame: &[BlameLine], line: u32) -> Option<BlameInfo> {
    let index = usize::try_from(line.saturating_sub(1)).ok()?;
    blame.get(index).map(BlameInfo::from)
}

fn project_comments(
    rows: &[DocumentRow],
    marker_source_rows: Option<&SourceRowIndex>,
    file_path: &str,
    comments: &[Comment],
    selected_comment_id: Option<i64>,
    cursor: Option<RowIndex>,
) -> DocumentComments {
    if comments.is_empty() {
        return DocumentComments::default();
    }
    let file_comments: Vec<&Comment> = comments
        .iter()
        .filter(|comment| comment.file_path() == file_path)
        .collect();
    if file_comments.is_empty() {
        return DocumentComments::default();
    }

    let source_rows = SourceRowIndex::new(rows);
    let marker_source_rows = marker_source_rows.unwrap_or(&source_rows);
    let projected = file_comments
        .into_iter()
        .filter_map(|comment| project_comment(&source_rows, marker_source_rows, comment))
        .collect();
    DocumentComments::with_selection(projected, selected_comment_id, cursor)
}

fn project_comment(
    source_rows: &SourceRowIndex,
    marker_source_rows: &SourceRowIndex,
    comment: &Comment,
) -> Option<DocumentComment> {
    let mut matching_rows = Vec::new();
    let mut marker_matching_rows = Vec::new();
    for segment in comment
        .anchor()
        .segments
        .iter()
        .filter(|segment| segment.file_path == comment.file_path())
    {
        let start = u32::try_from(segment.line_start.max(1)).ok()?;
        let end = u32::try_from(segment.line_end.max(segment.line_start).max(1)).ok()?;
        matching_rows.extend(source_rows.rows_for_range(segment.side, start, end));
        marker_matching_rows.extend(marker_source_rows.rows_for_range(segment.side, start, end));
    }

    matching_rows.sort();
    matching_rows.dedup();
    marker_matching_rows.sort();
    marker_matching_rows.dedup();
    let span_rows = if matching_rows.is_empty() {
        &marker_matching_rows
    } else {
        &matching_rows
    };
    let start = *span_rows.first()?;
    let end = *span_rows.last().unwrap_or(&start);
    let span = RowSpan { start, end };
    let marker_start = *marker_matching_rows.first().unwrap_or(&start);
    let marker_end = *marker_matching_rows.last().unwrap_or(&marker_start);
    Some(DocumentComment {
        id: comment.id,
        span,
        marker_rows: MarkerRows {
            start: marker_start,
            end: marker_end,
        },
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
    if let Some(selected) = selected_comment_id.and_then(|id| {
        comments
            .iter()
            .find(|comment| comment.span.contains(cursor) && comment.id == id)
    }) {
        return Some(selected.id);
    }
    preferred_current_comment(
        comments
            .iter()
            .filter(|comment| comment.span.contains(cursor)),
        cursor,
    )
    .map(|comment| comment.id)
}

fn preferred_comment<'a>(
    candidates: impl IntoIterator<Item = &'a DocumentComment>,
) -> Option<&'a DocumentComment> {
    candidates
        .into_iter()
        .max_by_key(|comment| (comment.span.start, comment.id))
}

fn preferred_current_comment<'a>(
    candidates: impl IntoIterator<Item = &'a DocumentComment>,
    row: RowIndex,
) -> Option<&'a DocumentComment> {
    candidates
        .into_iter()
        .max_by_key(|comment| current_comment_key(comment, row))
}

fn current_comment_key(comment: &DocumentComment, row: RowIndex) -> (u8, RowIndex, u8, i64) {
    let kind = marker_kind(comment.marker_rows, row);
    (
        marker_category_priority(kind),
        comment.span.start,
        marker_kind_priority(kind),
        comment.id,
    )
}

fn preferred_marker_comment<'a>(
    candidates: impl IntoIterator<Item = &'a DocumentComment>,
    row: RowIndex,
    current_comment_id: Option<i64>,
) -> Option<&'a DocumentComment> {
    let mut current_boundary = None;
    let mut boundary = None;
    let mut current_join = None;
    let mut join = None;

    for comment in candidates {
        let kind = marker_kind(comment.marker_rows, row);
        if Some(comment.id) == current_comment_id {
            if is_boundary_marker(kind) {
                current_boundary = preferred_marker_candidate(current_boundary, comment, row);
                continue;
            }
            current_join = preferred_marker_candidate(current_join, comment, row);
            continue;
        }

        if is_boundary_marker(kind) {
            boundary = preferred_marker_candidate(boundary, comment, row);
        } else {
            join = preferred_marker_candidate(join, comment, row);
        }
    }

    current_boundary.or(boundary).or(current_join).or(join)
}

fn preferred_marker_candidate<'a>(
    existing: Option<&'a DocumentComment>,
    candidate: &'a DocumentComment,
    row: RowIndex,
) -> Option<&'a DocumentComment> {
    match existing {
        Some(existing)
            if marker_preference_key(existing, row) >= marker_preference_key(candidate, row) =>
        {
            Some(existing)
        }
        _ => Some(candidate),
    }
}

fn marker_preference_key(comment: &DocumentComment, row: RowIndex) -> (RowIndex, u8, i64) {
    (
        comment.span.start,
        marker_kind_priority(marker_kind(comment.marker_rows, row)),
        comment.id,
    )
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

fn marker_category_priority(kind: CommentMarkerKind) -> u8 {
    if is_boundary_marker(kind) { 1 } else { 0 }
}

fn marker_kind_priority(kind: CommentMarkerKind) -> u8 {
    match kind {
        CommentMarkerKind::Join => 0,
        CommentMarkerKind::Start | CommentMarkerKind::End => 1,
        CommentMarkerKind::SingleLine => 2,
    }
}

fn is_boundary_marker(kind: CommentMarkerKind) -> bool {
    !matches!(kind, CommentMarkerKind::Join)
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
        segment_with_path("src/lib.rs", side, start, end)
    }

    fn segment_with_path(
        file_path: &str,
        side: CommentAnchorSide,
        start: i64,
        end: i64,
    ) -> CommentAnchorSegment {
        CommentAnchorSegment {
            side,
            file_path: file_path.to_string(),
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

    fn document_comment(id: i64, start: usize, end: usize) -> DocumentComment {
        DocumentComment {
            id,
            span: RowSpan {
                start: RowIndex(start),
                end: RowIndex(end),
            },
            marker_rows: MarkerRows {
                start: RowIndex(start),
                end: RowIndex(end),
            },
            resolved: false,
            selected: false,
            current: false,
        }
    }

    fn document_comments_at_cursor(
        comments: Vec<DocumentComment>,
        cursor: usize,
    ) -> DocumentComments {
        DocumentComments::with_selection(comments, None, Some(RowIndex(cursor)))
    }

    #[test]
    fn document_key_changes_when_structural_inputs_change() {
        let base = key(ContentMode::Diff, RenderVariant::Inline);

        let mut changed = base.clone();
        changed.file_id = "src/other.rs".to_string();
        assert_ne!(base, changed);

        let mut changed = base.clone();
        changed.diff_hash = "diff-b".to_string();
        assert_ne!(base, changed);

        let mut changed = base.clone();
        changed.content_mode = ContentMode::FullFile;
        assert_ne!(base, changed);

        let mut changed = base.clone();
        changed.render_variant = RenderVariant::SideBySide;
        assert_ne!(base, changed);

        let mut changed = base.clone();
        changed.diff_algorithm = DiffAlgorithm::Histogram;
        assert_ne!(base, changed);

        let mut changed = base.clone();
        changed.ignore_whitespace = true;
        assert_ne!(base, changed);

        let mut changed = base.clone();
        changed.head_content_id = Some(ContentId::from("head-b"));
        assert_ne!(base, changed);

        let mut changed = base.clone();
        changed.base_content_id = Some(ContentId::from("base-b"));
        assert_ne!(base, changed);
    }

    #[test]
    fn active_document_key_ignores_overlay_inputs() {
        let active_key = key(ContentMode::Diff, RenderVariant::Inline);
        let hunk = replacement_hunk();
        let comments = vec![comment(
            1,
            false,
            vec![segment(CommentAnchorSide::Head, 2, 2)],
        )];
        let mut plain_input = input(
            active_key.clone(),
            std::slice::from_ref(&hunk),
            Some("one\nold\nfour\n"),
            Some("one\nnew one\nnew two\nfour\n"),
            &[],
        );
        plain_input.cursor = Some(RowIndex(0));
        let mut decorated_input = input(
            active_key.clone(),
            std::slice::from_ref(&hunk),
            Some("one\nold\nfour\n"),
            Some("one\nnew one\nnew two\nfour\n"),
            &comments,
        );
        decorated_input.selected_comment_id = Some(1);
        decorated_input.cursor = Some(RowIndex(2));

        let plain = DiffDocumentBuilder::build(plain_input).expect("plain active document");
        let decorated =
            DiffDocumentBuilder::build(decorated_input).expect("decorated active document");

        assert_eq!(plain.key, active_key);
        assert_eq!(decorated.key, active_key);
    }

    #[test]
    fn document_overlays_do_not_mutate_document_key() {
        let document_key = key(ContentMode::Diff, RenderVariant::Inline);
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

        assert_eq!(document_key, key(ContentMode::Diff, RenderVariant::Inline));
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
                .has_line_in_range(CommentAnchorSide::Base, 1, 1)
        );
        assert!(
            content(&document, 0)
                .source
                .has_line_in_range(CommentAnchorSide::Head, 1, 1)
        );
        assert_eq!(content(&document, 0).source.base, Some(1));
        assert_eq!(content(&document, 0).source.head, Some(1));
        assert!(
            content(&document, 1)
                .source
                .has_line_in_range(CommentAnchorSide::Base, 2, 2)
        );
        assert!(
            content(&document, 2)
                .source
                .has_line_in_range(CommentAnchorSide::Head, 2, 2)
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
    fn documents_build_without_hunks_from_available_content() {
        let unified = build_unified_document(&input(
            key(ContentMode::Diff, RenderVariant::Inline),
            &[],
            Some("base one\nbase two\n"),
            Some("head one\nhead two\n"),
            &[],
        ));
        let side_by_side = build_side_by_side_document(&input(
            key(ContentMode::Diff, RenderVariant::SideBySide),
            &[],
            Some("base one\nbase two\nbase three\n"),
            Some("head one\nhead two\n"),
            &[],
        ))
        .expect("valid side-by-side document");

        assert_eq!(unified.len(), 2);
        assert!(unified.hunk_spans().is_empty());
        assert_eq!(content(&unified, 0).text, "head one");
        assert_eq!(content(&unified, 0).gutter.text, "1 1");
        assert_eq!(side_by_side.len(), 3);
        assert_eq!(content(side_by_side.base(), 2).text, "base three");
        assert!(matches!(
            side_by_side.head().row(RowIndex(2)),
            Some(DocumentRow::Spacer)
        ));
    }

    #[test]
    fn side_by_side_documents_insert_head_spacers_for_deletions() {
        let hunk = DiffHunk {
            old_start: 2,
            old_lines: 2,
            new_start: 2,
            new_lines: 0,
            header: "@@ -2,2 +1,0 @@".to_string(),
            lines: vec![
                diff_line(LineKind::Deletion, "old two", Some(2), None),
                diff_line(LineKind::Deletion, "old three", Some(3), None),
            ],
        };
        let document = build_side_by_side_document(&input(
            key(ContentMode::Diff, RenderVariant::SideBySide),
            &[hunk],
            Some("one\nold two\nold three\nfour\n"),
            Some("one\nfour\n"),
            &[],
        ))
        .expect("valid side-by-side document");

        assert_eq!(content(document.base(), 1).text, "old two");
        assert!(matches!(
            document.head().row(RowIndex(1)),
            Some(DocumentRow::Spacer)
        ));
        assert_eq!(content(document.base(), 2).text, "old three");
        assert!(matches!(
            document.head().row(RowIndex(2)),
            Some(DocumentRow::Spacer)
        ));
    }

    #[test]
    fn side_by_side_documents_insert_base_spacers_for_insertions() {
        let hunk = DiffHunk {
            old_start: 2,
            old_lines: 0,
            new_start: 2,
            new_lines: 2,
            header: "@@ -1,0 +2,2 @@".to_string(),
            lines: vec![
                diff_line(LineKind::Addition, "new two", None, Some(2)),
                diff_line(LineKind::Addition, "new three", None, Some(3)),
            ],
        };
        let document = build_side_by_side_document(&input(
            key(ContentMode::Diff, RenderVariant::SideBySide),
            &[hunk],
            Some("one\nfour\n"),
            Some("one\nnew two\nnew three\nfour\n"),
            &[],
        ))
        .expect("valid side-by-side document");

        assert!(matches!(
            document.base().row(RowIndex(1)),
            Some(DocumentRow::Spacer)
        ));
        assert_eq!(content(document.head(), 1).text, "new two");
        assert!(matches!(
            document.base().row(RowIndex(2)),
            Some(DocumentRow::Spacer)
        ));
        assert_eq!(content(document.head(), 2).text, "new three");
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
        assert_eq!(document.next_hunk(RowIndex(5), Direction::Next), None);
        assert_eq!(document.next_hunk(RowIndex(0), Direction::Prev), None);
    }

    #[test]
    fn hunk_navigation_works_for_all_document_variants() {
        let hunk = replacement_hunk();
        let base_content = Some("one\nold\nfour\n");
        let head_content = Some("one\nnew one\nnew two\nfour\n");
        let unified = build_unified_document(&input(
            key(ContentMode::Diff, RenderVariant::Inline),
            std::slice::from_ref(&hunk),
            base_content,
            head_content,
            &[],
        ));
        let side_by_side = build_side_by_side_document(&input(
            key(ContentMode::Diff, RenderVariant::SideBySide),
            std::slice::from_ref(&hunk),
            base_content,
            head_content,
            &[],
        ))
        .expect("valid side-by-side document");
        let base = build_base_document(&input(
            key(ContentMode::FullFile, RenderVariant::BaseVersion),
            std::slice::from_ref(&hunk),
            base_content,
            head_content,
            &[],
        ));
        let head = build_head_document(&input(
            key(ContentMode::FullFile, RenderVariant::HeadVersion),
            std::slice::from_ref(&hunk),
            base_content,
            head_content,
            &[],
        ));

        assert_eq!(
            unified.next_hunk(RowIndex(0), Direction::Next),
            Some(RowIndex(1))
        );
        assert_eq!(
            side_by_side.next_hunk(RowIndex(0), Direction::Next),
            Some(RowIndex(1))
        );
        assert_eq!(
            base.next_hunk(RowIndex(0), Direction::Next),
            Some(RowIndex(1))
        );
        assert_eq!(
            head.next_hunk(RowIndex(0), Direction::Next),
            Some(RowIndex(1))
        );
    }

    #[test]
    fn diff_document_builder_routes_variants_and_forwards_accessors() {
        let hunk = replacement_hunk();
        let comments = vec![comment(
            41,
            false,
            vec![segment(CommentAnchorSide::Head, 2, 2)],
        )];
        let mut unified_input = input(
            key(ContentMode::Diff, RenderVariant::Inline),
            std::slice::from_ref(&hunk),
            Some("one\nold\nfour\n"),
            Some("one\nnew one\nnew two\nfour\n"),
            &comments,
        );
        unified_input.cursor = Some(RowIndex(2));
        let unified = DiffDocumentBuilder::build(unified_input).expect("unified document");
        assert!(matches!(unified.diff, DiffDocument::Unified(_)));
        assert_eq!(unified.diff.len(), 5);
        assert_eq!(unified.diff.current_comment_id(), Some(41));

        let side_by_side = DiffDocumentBuilder::build(input(
            key(ContentMode::Diff, RenderVariant::SideBySide),
            std::slice::from_ref(&hunk),
            Some("one\nold\nfour\n"),
            Some("one\nnew one\nnew two\nfour\n"),
            &comments,
        ))
        .expect("side-by-side document");
        assert!(matches!(side_by_side.diff, DiffDocument::SideBySide(_)));
        assert_eq!(side_by_side.diff.len(), 4);

        let mut side_by_side_input = input(
            key(ContentMode::Diff, RenderVariant::SideBySide),
            std::slice::from_ref(&hunk),
            Some("one\nold\nfour\n"),
            Some("one\nnew one\nnew two\nfour\n"),
            &comments,
        );
        side_by_side_input.cursor = Some(RowIndex(1));
        let side_by_side =
            DiffDocumentBuilder::build(side_by_side_input).expect("side-by-side document");
        assert_eq!(side_by_side.diff.current_comment_id(), Some(41));

        let base_comments = vec![comment(
            42,
            false,
            vec![segment(CommentAnchorSide::Base, 2, 2)],
        )];
        let mut base_input = input(
            key(ContentMode::FullFile, RenderVariant::BaseVersion),
            std::slice::from_ref(&hunk),
            Some("one\nold\nfour\n"),
            Some("one\nnew one\nnew two\nfour\n"),
            &base_comments,
        );
        base_input.cursor = Some(RowIndex(1));
        let base = DiffDocumentBuilder::build(base_input).expect("base document");
        assert!(matches!(base.diff, DiffDocument::Base(_)));
        assert_eq!(base.diff.len(), 3);
        assert_eq!(base.diff.current_comment_id(), Some(42));

        let head_comments = vec![comment(
            43,
            false,
            vec![segment(CommentAnchorSide::Head, 2, 2)],
        )];
        let mut head_input = input(
            key(ContentMode::FullFile, RenderVariant::HeadVersion),
            std::slice::from_ref(&hunk),
            Some("one\nold\nfour\n"),
            Some("one\nnew one\nnew two\nfour\n"),
            &head_comments,
        );
        head_input.cursor = Some(RowIndex(1));
        let head = DiffDocumentBuilder::build(head_input).expect("head document");
        assert!(matches!(head.diff, DiffDocument::Head(_)));
        assert_eq!(head.diff.len(), 4);
        assert_eq!(head.diff.current_comment_id(), Some(43));

        let build_error = DiffDocumentBuildError::from(SideBySideDocumentError {
            base_len: 1,
            head_len: 2,
        });
        assert_eq!(
            build_error.to_string(),
            "side-by-side documents must have equal row counts: base=1, head=2"
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
    fn blame_is_routed_by_document_variant_and_source_side() {
        let hunk = replacement_hunk();
        let head_blame = vec![
            BlameLine {
                hash: "head-1".to_string(),
                author: "Head One".to_string(),
                date: "2026-01-01".to_string(),
            },
            BlameLine {
                hash: "head-2".to_string(),
                author: "Head Two".to_string(),
                date: "2026-01-02".to_string(),
            },
            BlameLine {
                hash: "head-3".to_string(),
                author: "Head Three".to_string(),
                date: "2026-01-03".to_string(),
            },
        ];
        let base_blame = vec![
            BlameLine {
                hash: "base-1".to_string(),
                author: "Base One".to_string(),
                date: "2026-01-01".to_string(),
            },
            BlameLine {
                hash: "base-2".to_string(),
                author: "Base Two".to_string(),
                date: "2026-01-02".to_string(),
            },
        ];
        let mut input = input(
            key(ContentMode::Diff, RenderVariant::Inline),
            std::slice::from_ref(&hunk),
            Some("one\nold\n"),
            Some("one\nnew one\nnew two\n"),
            &[],
        );
        input.head_blame = &head_blame;
        input.base_blame = &base_blame;

        let unified = build_unified_document(&input);
        assert_eq!(
            content(&unified, 1).blame.as_ref().map(|blame| &blame.hash),
            Some(&"base-2".to_string())
        );
        assert_eq!(
            content(&unified, 2).blame.as_ref().map(|blame| &blame.hash),
            Some(&"head-2".to_string())
        );

        input.key = key(ContentMode::FullFile, RenderVariant::BaseVersion);
        let base = build_base_document(&input);
        assert_eq!(
            content(&base, 1).blame.as_ref().map(|blame| &blame.hash),
            Some(&"base-2".to_string())
        );
        assert!(
            base.rows()
                .iter()
                .filter_map(|row| match row {
                    DocumentRow::Content(content) => content.blame.as_ref(),
                    DocumentRow::Spacer => None,
                })
                .all(|blame| blame.hash.starts_with("base-"))
        );

        input.key = key(ContentMode::FullFile, RenderVariant::HeadVersion);
        let head = build_head_document(&input);
        assert_eq!(
            content(&head, 1).blame.as_ref().map(|blame| &blame.hash),
            Some(&"head-2".to_string())
        );
        assert!(
            head.rows()
                .iter()
                .filter_map(|row| match row {
                    DocumentRow::Content(content) => content.blame.as_ref(),
                    DocumentRow::Spacer => None,
                })
                .all(|blame| blame.hash.starts_with("head-"))
        );

        input.key = key(ContentMode::Diff, RenderVariant::SideBySide);
        let side_by_side = build_side_by_side_document(&input).expect("side-by-side document");
        assert_eq!(
            content(side_by_side.base(), 1)
                .blame
                .as_ref()
                .map(|blame| &blame.hash),
            Some(&"base-2".to_string())
        );
        assert_eq!(
            content(side_by_side.head(), 1)
                .blame
                .as_ref()
                .map(|blame| &blame.hash),
            Some(&"head-2".to_string())
        );
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
    fn document_public_comment_accessors_use_logical_comment_indexes() {
        let comments = DocumentComments::with_selection(
            vec![
                document_comment(1, 0, 1),
                document_comment(2, 1, 3),
                DocumentComment {
                    resolved: true,
                    ..document_comment(3, 4, 4)
                },
            ],
            None,
            Some(RowIndex(1)),
        );
        let document = Document::new(Vec::new(), Vec::new()).with_overlays(DocumentOverlays {
            comments,
            ..DocumentOverlays::default()
        });

        assert_eq!(document.current_comment_id(), Some(2));
        assert_eq!(
            document.comment_at(RowIndex(0)).map(|comment| comment.id),
            Some(1)
        );
        assert_eq!(
            document.comment_at(RowIndex(1)).map(|comment| comment.id),
            Some(2)
        );
        assert_eq!(
            document
                .next_comment(RowIndex(1), Direction::Next)
                .map(|comment| comment.id),
            Some(3)
        );
        assert_eq!(
            document
                .next_unresolved_comment(RowIndex(1), Direction::Next)
                .map(|comment| comment.id),
            None
        );
    }

    #[test]
    fn empty_documents_and_out_of_range_rows_return_no_render_state() {
        let document = Document::new(Vec::new(), Vec::new());

        assert!(document.is_empty());
        assert_eq!(document.row(RowIndex(0)), None);
        assert_eq!(document.content_row(RowIndex(0)), None);
        assert_eq!(document.line(RowIndex(0)), None);
        assert_eq!(document.comment_at(RowIndex(0)), None);
        assert_eq!(
            format_gutter(None, None),
            "",
            "empty gutters should render as empty text"
        );
        assert_eq!(ContentId::from(String::from("owned")).0, "owned");
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
        assert_eq!(
            SideBySideDocumentError {
                base_len: 0,
                head_len: 1
            }
            .to_string(),
            "side-by-side documents must have equal row counts: base=0, head=1"
        );
    }

    #[test]
    fn side_by_side_accessors_preserve_two_single_documents() {
        let base = Document::new(
            vec![DocumentRow::Content(side_content_row(
                CommentAnchorSide::Base,
                1,
                "base",
                LineKind::Context,
                &[],
            ))],
            vec![HunkSpan {
                full_span: RowSpan {
                    start: RowIndex(0),
                    end: RowIndex(0),
                },
                first_change: RowIndex(0),
            }],
        )
        .with_overlays(DocumentOverlays {
            comments: DocumentComments::with_selection(
                vec![document_comment(10, 0, 0)],
                None,
                Some(RowIndex(0)),
            ),
            ..DocumentOverlays::default()
        });
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
        let side_by_side =
            SideBySideDocument::new(base.clone(), head).expect("valid side-by-side document");

        assert_eq!(side_by_side.current_comment_id(), Some(10));
        assert_eq!(side_by_side.next_hunk(RowIndex(0), Direction::Next), None);

        let head = Document::new(
            vec![DocumentRow::Content(side_content_row(
                CommentAnchorSide::Head,
                1,
                "head",
                LineKind::Context,
                &[],
            ))],
            vec![HunkSpan {
                full_span: RowSpan {
                    start: RowIndex(0),
                    end: RowIndex(0),
                },
                first_change: RowIndex(0),
            }],
        )
        .with_overlays(DocumentOverlays {
            comments: DocumentComments::with_selection(
                vec![document_comment(20, 0, 0)],
                None,
                Some(RowIndex(0)),
            ),
            ..DocumentOverlays::default()
        });
        let side_by_side =
            SideBySideDocument::new(base, head).expect("valid side-by-side document");
        assert_eq!(side_by_side.current_comment_id(), Some(20));
        assert_eq!(side_by_side.next_hunk(RowIndex(0), Direction::Prev), None);
        let (base, head) = side_by_side.into_parts();
        assert_eq!(content(&base, 0).text, "base");
        assert_eq!(content(&head, 0).text, "head");
    }

    #[test]
    fn spacer_rows_can_render_comment_markers() {
        let comments = vec![comment(
            8,
            false,
            vec![segment(CommentAnchorSide::Head, 2, 4)],
        )];
        let hunk = DiffHunk {
            old_start: 1,
            old_lines: 3,
            new_start: 1,
            new_lines: 4,
            header: "@@ -1,3 +1,4 @@".to_string(),
            lines: vec![
                diff_line(LineKind::Context, "one", Some(1), Some(1)),
                diff_line(LineKind::Deletion, "old two", Some(2), None),
                diff_line(LineKind::Addition, "new two", None, Some(2)),
                diff_line(LineKind::Addition, "new three", None, Some(3)),
                diff_line(LineKind::Addition, "new four", None, Some(4)),
            ],
        };
        let side_by_side = build_side_by_side_document(&input(
            key(ContentMode::Diff, RenderVariant::SideBySide),
            &[hunk],
            Some("one\nold two\n"),
            Some("one\nnew two\nnew three\nnew four\n"),
            &comments,
        ))
        .expect("valid side-by-side document");

        assert!(matches!(
            side_by_side.base().row(RowIndex(2)),
            Some(DocumentRow::Spacer)
        ));
        let Some(RenderLine::Spacer { marker }) = side_by_side.base().line(RowIndex(2)) else {
            panic!("expected spacer render line");
        };
        assert_eq!(marker.kind(), Some(CommentMarkerKind::Join));
    }

    #[test]
    fn full_file_hunk_span_closes_when_hunk_reaches_file_end() {
        let hunk = DiffHunk {
            old_start: 2,
            old_lines: 2,
            new_start: 2,
            new_lines: 2,
            header: "@@ -2,2 +2,2 @@".to_string(),
            lines: vec![
                diff_line(LineKind::Deletion, "old two", Some(2), None),
                diff_line(LineKind::Addition, "new two", None, Some(2)),
                diff_line(LineKind::Context, "three", Some(3), Some(3)),
            ],
        };
        let head = build_head_document(&input(
            key(ContentMode::FullFile, RenderVariant::HeadVersion),
            &[hunk],
            Some("one\nold two\nthree\n"),
            Some("one\nnew two\nthree\n"),
            &[],
        ));

        assert_eq!(
            head.hunk_spans(),
            &[HunkSpan {
                full_span: RowSpan {
                    start: RowIndex(1),
                    end: RowIndex(2)
                },
                first_change: RowIndex(1)
            }]
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
            comments: project_comments(
                &rows,
                None,
                "src/lib.rs",
                &comments,
                Some(2),
                Some(RowIndex(2)),
            ),
            ..DocumentOverlays::default()
        };
        let document = Document::new(rows, Vec::new()).with_overlays(overlays);

        assert_eq!(
            marker_kind_at(&document, 0),
            Some(CommentMarkerKind::SingleLine)
        );
        assert_eq!(marker_kind_at(&document, 1), Some(CommentMarkerKind::Start));
        assert_eq!(
            marker_kind_at(&document, 2),
            Some(CommentMarkerKind::SingleLine)
        );
        assert_eq!(marker_kind_at(&document, 3), Some(CommentMarkerKind::End));
        assert!(
            !document
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
        let document_comments = project_comments(
            &rows,
            None,
            "src/lib.rs",
            &comments,
            Some(1),
            Some(RowIndex(0)),
        );

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
    fn comment_projection_drops_other_files_and_unmapped_source_lines() {
        let rows = vec![DocumentRow::Content(side_content_row(
            CommentAnchorSide::Head,
            1,
            "one",
            LineKind::Context,
            &[],
        ))];
        let comments = vec![
            comment(
                1,
                false,
                vec![segment_with_path("other.rs", CommentAnchorSide::Head, 1, 1)],
            ),
            comment(2, false, vec![segment(CommentAnchorSide::Head, 99, 99)]),
            comment(3, false, vec![segment(CommentAnchorSide::Head, 1, 1)]),
        ];
        let projected = project_comments(&rows, None, "src/lib.rs", &comments, None, None);

        assert_eq!(
            projected
                .all()
                .iter()
                .map(|comment| comment.id)
                .collect::<Vec<_>>(),
            vec![3]
        );
    }

    #[test]
    fn document_comments_keep_row_order_and_index_by_id() {
        let comments = DocumentComments::new(vec![
            DocumentComment {
                id: 20,
                span: RowSpan {
                    start: RowIndex(2),
                    end: RowIndex(2),
                },
                marker_rows: MarkerRows {
                    start: RowIndex(2),
                    end: RowIndex(2),
                },
                resolved: false,
                selected: false,
                current: false,
            },
            DocumentComment {
                id: 10,
                span: RowSpan {
                    start: RowIndex(0),
                    end: RowIndex(1),
                },
                marker_rows: MarkerRows {
                    start: RowIndex(0),
                    end: RowIndex(1),
                },
                resolved: false,
                selected: false,
                current: false,
            },
        ]);

        let ordered_ids: Vec<i64> = comments.all().iter().map(|comment| comment.id).collect();
        assert_eq!(ordered_ids, vec![10, 20]);
        assert_eq!(
            comments.comment_by_id(20).map(|comment| comment.span),
            Some(RowSpan {
                start: RowIndex(2),
                end: RowIndex(2)
            })
        );
        assert_eq!(comments.comments_by_row.get(&RowIndex(0)), Some(&vec![0]));
        assert_eq!(comments.comments_by_row.get(&RowIndex(1)), Some(&vec![0]));
        assert_eq!(comments.comments_by_row.get(&RowIndex(2)), Some(&vec![1]));
        assert_eq!(
            comments.marker_comments_by_row.get(&RowIndex(0)),
            Some(&vec![0])
        );
        assert_eq!(
            comments.marker_comments_by_row.get(&RowIndex(1)),
            Some(&vec![0])
        );
        assert_eq!(
            comments.marker_comments_by_row.get(&RowIndex(2)),
            Some(&vec![1])
        );
        assert_eq!(comments.comments_by_id.get(&10), Some(&0));
        assert_eq!(comments.comments_by_id.get(&20), Some(&1));
        assert_eq!(comments.unresolved_comment_indices, vec![0, 1]);
        assert_eq!(
            comments.unresolved_comment_positions_by_id.get(&10),
            Some(&0)
        );
        assert_eq!(
            comments.unresolved_comment_positions_by_id.get(&20),
            Some(&1)
        );
    }

    #[test]
    fn document_comments_index_unresolved_navigation_order() {
        let source_comments = vec![
            document_comment(1, 0, 0),
            DocumentComment {
                resolved: true,
                ..document_comment(2, 1, 1)
            },
            document_comment(3, 2, 2),
        ];
        let comments = DocumentComments::with_selection(source_comments.clone(), Some(1), None);

        assert_eq!(comments.unresolved_comment_indices, vec![0, 2]);
        assert_eq!(
            comments
                .next_unresolved_comment(RowIndex(0), Direction::Next)
                .map(|comment| comment.id),
            Some(3)
        );
        let comments = DocumentComments::with_selection(source_comments, Some(3), None);
        assert_eq!(
            comments
                .next_unresolved_comment(RowIndex(2), Direction::Prev)
                .map(|comment| comment.id),
            Some(1)
        );
    }

    #[test]
    fn comment_navigation_from_arbitrary_cursor_uses_sorted_comment_order() {
        let comments = DocumentComments::new(vec![
            document_comment(1, 2, 4),
            document_comment(2, 5, 5),
            document_comment(3, 7, 8),
        ]);

        assert_eq!(
            comments
                .next_comment(RowIndex(4), Direction::Next)
                .map(|comment| comment.id),
            Some(2)
        );
        assert_eq!(
            comments
                .next_comment(RowIndex(7), Direction::Prev)
                .map(|comment| comment.id),
            Some(2)
        );
        assert_eq!(
            comments
                .next_comment(RowIndex(2), Direction::Prev)
                .map(|comment| comment.id),
            None
        );
        assert_eq!(
            comments
                .next_comment(RowIndex(8), Direction::Next)
                .map(|comment| comment.id),
            None
        );
    }

    #[test]
    fn unresolved_navigation_from_arbitrary_cursor_skips_resolved_comments() {
        let comments = DocumentComments::new(vec![
            document_comment(1, 2, 2),
            DocumentComment {
                resolved: true,
                ..document_comment(2, 4, 4)
            },
            document_comment(3, 6, 6),
        ]);

        assert_eq!(
            comments
                .next_unresolved_comment(RowIndex(3), Direction::Next)
                .map(|comment| comment.id),
            Some(3)
        );
        assert_eq!(
            comments
                .next_unresolved_comment(RowIndex(6), Direction::Prev)
                .map(|comment| comment.id),
            Some(1)
        );
    }

    #[test]
    fn source_row_index_maps_source_lines_to_document_rows() {
        let rows = vec![
            DocumentRow::Content(content_row(
                Some(10),
                Some(20),
                "same",
                LineKind::Context,
                &[],
                &[],
            )),
            DocumentRow::Spacer,
            DocumentRow::Content(side_content_row(
                CommentAnchorSide::Base,
                11,
                "old",
                LineKind::Deletion,
                &[],
            )),
            DocumentRow::Content(side_content_row(
                CommentAnchorSide::Head,
                21,
                "new",
                LineKind::Addition,
                &[],
            )),
        ];
        let index = SourceRowIndex::new(&rows);

        let base_rows: Vec<RowIndex> = index
            .rows_for_range(CommentAnchorSide::Base, 10, 11)
            .collect();
        let head_rows: Vec<RowIndex> = index
            .rows_for_range(CommentAnchorSide::Head, 20, 21)
            .collect();

        assert_eq!(base_rows, vec![RowIndex(0), RowIndex(2)]);
        assert_eq!(head_rows, vec![RowIndex(0), RowIndex(3)]);
    }

    #[test]
    fn aligned_source_row_index_maps_spacer_rows_to_other_side_markers() {
        let base_rows = vec![
            DocumentRow::Content(side_content_row(
                CommentAnchorSide::Base,
                10,
                "old",
                LineKind::Deletion,
                &[],
            )),
            DocumentRow::Spacer,
        ];
        let head_rows = vec![
            DocumentRow::Content(side_content_row(
                CommentAnchorSide::Head,
                20,
                "new one",
                LineKind::Addition,
                &[],
            )),
            DocumentRow::Content(side_content_row(
                CommentAnchorSide::Head,
                21,
                "new two",
                LineKind::Addition,
                &[],
            )),
        ];
        let index = SourceRowIndex::aligned(&base_rows, &head_rows);

        let base_rows: Vec<RowIndex> = index
            .rows_for_range(CommentAnchorSide::Base, 10, 10)
            .collect();
        let head_rows: Vec<RowIndex> = index
            .rows_for_range(CommentAnchorSide::Head, 20, 21)
            .collect();

        assert_eq!(base_rows, vec![RowIndex(0)]);
        assert_eq!(head_rows, vec![RowIndex(0), RowIndex(1)]);
    }

    #[test]
    fn source_location_none_has_no_side_lines() {
        let source = SourceLocation::none();

        assert_eq!(source.line_for_side(CommentAnchorSide::Base), None);
        assert_eq!(source.line_for_side(CommentAnchorSide::Head), None);
        assert!(!source.has_line_in_range(CommentAnchorSide::Base, 1, 1));
        assert!(!source.has_line_in_range(CommentAnchorSide::Head, 1, 1));
    }

    #[test]
    fn current_comment_prefers_boundaries_then_highest_starting_row() {
        let comments = vec![document_comment(1, 10, 20), document_comment(2, 15, 25)];

        assert_eq!(
            document_comments_at_cursor(comments.clone(), 14).current_comment_id(),
            Some(1)
        );
        assert_eq!(
            document_comments_at_cursor(comments.clone(), 15).current_comment_id(),
            Some(2)
        );
        assert_eq!(
            document_comments_at_cursor(comments.clone(), 19).current_comment_id(),
            Some(2)
        );
        assert_eq!(
            document_comments_at_cursor(comments.clone(), 20).current_comment_id(),
            Some(1)
        );
        assert_eq!(
            document_comments_at_cursor(comments, 21).current_comment_id(),
            Some(2)
        );
    }

    #[test]
    fn current_comment_at_prefers_selected_then_current_then_highest_starting_row() {
        let comments = vec![
            document_comment(1, 10, 20),
            document_comment(2, 15, 25),
            document_comment(3, 16, 18),
        ];
        let selected =
            DocumentComments::with_selection(comments.clone(), Some(1), Some(RowIndex(16)));
        let current = DocumentComments::with_selection(comments.clone(), None, Some(RowIndex(16)));
        let fallback = DocumentComments::new(comments);

        assert_eq!(
            selected
                .current_comment_at(RowIndex(16))
                .map(|comment| comment.id),
            Some(1)
        );
        assert_eq!(
            current
                .current_comment_at(RowIndex(17))
                .map(|comment| comment.id),
            Some(3)
        );
        assert_eq!(
            fallback
                .current_comment_at(RowIndex(17))
                .map(|comment| comment.id),
            Some(3)
        );
        assert_eq!(
            fallback
                .current_comment_at(RowIndex(30))
                .map(|comment| comment.id),
            None
        );
    }

    #[test]
    fn preferred_marker_uses_highest_start_row_then_kind_then_id() {
        let same_end = DocumentComments::new(vec![
            document_comment(1, 10, 20),
            document_comment(2, 15, 20),
        ]);
        let same_start = DocumentComments::new(vec![
            document_comment(1, 10, 20),
            document_comment(2, 10, 10),
            document_comment(3, 10, 20),
        ]);

        assert_eq!(
            same_end
                .current_marker_comment(RowIndex(20))
                .map(|comment| comment.id),
            Some(2)
        );
        assert_eq!(
            same_start
                .current_marker_comment(RowIndex(10))
                .map(|comment| comment.id),
            Some(2)
        );
    }

    #[test]
    fn inactive_boundary_marker_overrides_current_join_marker() {
        let comments = document_comments_at_cursor(
            vec![document_comment(1, 10, 20), document_comment(2, 15, 25)],
            20,
        );

        let line_15_marker = comments.marker_for_row(RowIndex(15));
        assert_eq!(line_15_marker.kind(), Some(CommentMarkerKind::Start));
        assert!(!line_15_marker.is_current());

        let line_20_marker = comments.marker_for_row(RowIndex(20));
        assert_eq!(line_20_marker.kind(), Some(CommentMarkerKind::End));
        assert!(line_20_marker.is_current());
    }

    #[test]
    fn current_boundary_marker_overrides_inactive_nested_boundary() {
        let comments = vec![
            document_comment(1, 10, 20),
            document_comment(2, 15, 25),
            document_comment(3, 15, 15),
        ];

        let single_line_current = document_comments_at_cursor(comments.clone(), 15);
        assert_eq!(single_line_current.current_comment_id(), Some(3));
        let marker = single_line_current.marker_for_row(RowIndex(15));
        assert_eq!(marker.kind(), Some(CommentMarkerKind::SingleLine));
        assert!(marker.is_current());

        let nested_range_current = document_comments_at_cursor(comments, 16);
        assert_eq!(nested_range_current.current_comment_id(), Some(2));
        let marker = nested_range_current.marker_for_row(RowIndex(15));
        assert_eq!(marker.kind(), Some(CommentMarkerKind::Start));
        assert!(marker.is_current());
        let marker = nested_range_current.marker_for_row(RowIndex(16));
        assert_eq!(marker.kind(), Some(CommentMarkerKind::Join));
        assert!(marker.is_current());
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
        let RenderLine::Content(rendered) = document.line(RowIndex(0)).expect("render line") else {
            panic!("expected content render line");
        };
        assert_eq!(rendered.runs.len(), 1);
        assert_eq!(rendered.runs[0].kind, TextRunKind::Plain);
        assert_eq!(rendered.runs[0].text, Cow::Borrowed("needle hay needle"));

        document.search("needle");
        assert_eq!(document.overlays().search.query.as_deref(), Some("needle"));
        assert_eq!(
            document.search_matches(RowIndex(0)).collect::<Vec<_>>(),
            vec![
                SearchMatch {
                    row: RowIndex(0),
                    columns: ColumnSpan {
                        start: ColumnIndex(0),
                        end: ColumnIndex(6)
                    }
                },
                SearchMatch {
                    row: RowIndex(0),
                    columns: ColumnSpan {
                        start: ColumnIndex(11),
                        end: ColumnIndex(17)
                    }
                }
            ]
        );
        let RenderLine::Content(rendered) = document.line(RowIndex(0)).expect("render line") else {
            panic!("expected content render line");
        };
        assert_eq!(rendered.runs[0].kind, TextRunKind::SearchMatch);
        assert_eq!(rendered.runs[1].kind, TextRunKind::Plain);
        assert_eq!(rendered.runs[2].kind, TextRunKind::SearchMatch);

        document.search("");
        assert_eq!(document.overlays().search.query, None);
        document.search("hay");
        document.clear_search();
        assert_eq!(
            document.search_matches(RowIndex(0)).collect::<Vec<_>>(),
            Vec::<SearchMatch>::new()
        );
        let RenderLine::Content(rendered) = document.line(RowIndex(0)).expect("render line") else {
            panic!("expected content render line");
        };
        assert_eq!(rendered.runs.len(), 1);
        assert_eq!(rendered.runs[0].kind, TextRunKind::Plain);
        assert_eq!(rendered.runs[0].text, Cow::Borrowed("needle hay needle"));
    }

    #[test]
    fn search_overlay_preserves_trailing_plain_text_after_last_match() {
        let mut document = Document::new(
            vec![DocumentRow::Content(ContentRow {
                gutter: Gutter {
                    text: "1".to_string(),
                },
                kind: LineKind::Context,
                text: "needle tail".to_string(),
                blame: None,
                source: SourceLocation::single(CommentAnchorSide::Head, 1),
            })],
            Vec::new(),
        );

        document.search("needle");
        let RenderLine::Content(rendered) = document.line(RowIndex(0)).expect("render line") else {
            panic!("expected content render line");
        };
        assert_eq!(rendered.runs.len(), 2);
        assert_eq!(rendered.runs[0].kind, TextRunKind::SearchMatch);
        assert_eq!(rendered.runs[0].text, Cow::Borrowed("needle"));
        assert_eq!(rendered.runs[1].kind, TextRunKind::Plain);
        assert_eq!(rendered.runs[1].text, Cow::Borrowed(" tail"));
    }

    #[test]
    fn all_search_matches_iterates_content_rows_in_document_order() {
        let mut document = Document::new(
            vec![
                DocumentRow::Content(ContentRow {
                    gutter: Gutter {
                        text: "1".to_string(),
                    },
                    kind: LineKind::Context,
                    text: "needle first needle".to_string(),
                    blame: None,
                    source: SourceLocation::single(CommentAnchorSide::Head, 1),
                }),
                DocumentRow::Spacer,
                DocumentRow::Content(ContentRow {
                    gutter: Gutter {
                        text: "2".to_string(),
                    },
                    kind: LineKind::Context,
                    text: "second needle".to_string(),
                    blame: None,
                    source: SourceLocation::single(CommentAnchorSide::Head, 2),
                }),
            ],
            Vec::new(),
        );

        assert_eq!(
            document.all_search_matches().collect::<Vec<_>>(),
            Vec::<SearchMatch>::new()
        );

        document.search("needle");

        assert_eq!(
            document.all_search_matches().collect::<Vec<_>>(),
            vec![
                SearchMatch {
                    row: RowIndex(0),
                    columns: ColumnSpan {
                        start: ColumnIndex(0),
                        end: ColumnIndex(6)
                    }
                },
                SearchMatch {
                    row: RowIndex(0),
                    columns: ColumnSpan {
                        start: ColumnIndex(13),
                        end: ColumnIndex(19)
                    }
                },
                SearchMatch {
                    row: RowIndex(2),
                    columns: ColumnSpan {
                        start: ColumnIndex(7),
                        end: ColumnIndex(13)
                    }
                }
            ]
        );
    }

    #[test]
    fn visual_line_selection_stores_document_row_spans() {
        let mut document = Document::new(Vec::new(), Vec::new());
        document.overlays_mut().selection = Some(VisibleSelection::Line(RowSpan {
            start: RowIndex(2),
            end: RowIndex(5),
        }));

        assert_eq!(
            document.overlays().selection,
            Some(VisibleSelection::Line(RowSpan {
                start: RowIndex(2),
                end: RowIndex(5)
            }))
        );
    }

    #[test]
    fn text_selection_stores_document_row_and_column_positions() {
        let mut document = Document::new(Vec::new(), Vec::new());
        document.overlays_mut().selection = Some(VisibleSelection::Text {
            start: DocumentPosition {
                row: RowIndex(1),
                column: ColumnIndex(3),
            },
            end: DocumentPosition {
                row: RowIndex(4),
                column: ColumnIndex(9),
            },
        });

        assert_eq!(
            document.overlays().selection,
            Some(VisibleSelection::Text {
                start: DocumentPosition {
                    row: RowIndex(1),
                    column: ColumnIndex(3)
                },
                end: DocumentPosition {
                    row: RowIndex(4),
                    column: ColumnIndex(9)
                }
            })
        );
    }

    #[test]
    fn selected_content_rows_expose_source_metadata_for_anchor_capture() {
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
                .has_line_in_range(CommentAnchorSide::Base, 2, 2)
        );
        assert!(
            content(&document, 2)
                .source
                .has_line_in_range(CommentAnchorSide::Head, 2, 2)
        );
        assert!(
            content(&document, 3)
                .source
                .has_line_in_range(CommentAnchorSide::Head, 3, 3)
        );
        assert!(matches!(
            document.overlays().selection,
            Some(VisibleSelection::Text { .. })
        ));
    }

    #[test]
    fn current_line_comment_capture_uses_selected_row_source_metadata() {
        let hunk = replacement_hunk();
        let document = build_unified_document(&input(
            key(ContentMode::Diff, RenderVariant::Inline),
            &[hunk],
            Some("one\nold\nfour\n"),
            Some("one\nnew one\nnew two\nfour\n"),
            &[],
        ));

        assert_eq!(
            document
                .content_row(RowIndex(1))
                .and_then(|content| content.source.line_for_side(CommentAnchorSide::Base)),
            Some(2)
        );
        assert_eq!(
            document
                .content_row(RowIndex(2))
                .and_then(|content| content.source.line_for_side(CommentAnchorSide::Head)),
            Some(2)
        );
        assert_eq!(
            document
                .content_row(RowIndex(0))
                .map(|content| content.source),
            Some(SourceLocation::paired(Some(1), Some(1)))
        );
    }

    #[test]
    fn multiline_comment_capture_can_derive_base_and_head_segments() {
        let hunk = replacement_hunk();
        let document = build_unified_document(&input(
            key(ContentMode::Diff, RenderVariant::Inline),
            &[hunk],
            Some("one\nold\nfour\n"),
            Some("one\nnew one\nnew two\nfour\n"),
            &[],
        ));
        let selected_rows = RowSpan {
            start: RowIndex(1),
            end: RowIndex(3),
        };
        let mut base_lines = Vec::new();
        let mut head_lines = Vec::new();
        for row in selected_rows.start.0..=selected_rows.end.0 {
            let source = content(&document, row).source;
            if let Some(line) = source.line_for_side(CommentAnchorSide::Base) {
                base_lines.push(line);
            }
            if let Some(line) = source.line_for_side(CommentAnchorSide::Head) {
                head_lines.push(line);
            }
        }

        assert_eq!(base_lines, vec![2]);
        assert_eq!(head_lines, vec![2, 3]);
    }

    #[test]
    fn source_lookup_can_preserve_cursor_across_document_variants() {
        let hunk = replacement_hunk();
        let unified = build_unified_document(&input(
            key(ContentMode::Diff, RenderVariant::Inline),
            std::slice::from_ref(&hunk),
            Some("one\nold\nfour\n"),
            Some("one\nnew one\nnew two\nfour\n"),
            &[],
        ));
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
        let unified_source = content(&unified, 2).source;
        let head_index = SourceRowIndex::new(head.rows());
        let base_index = SourceRowIndex::new(base.rows());

        let head_row = unified_source
            .line_for_side(CommentAnchorSide::Head)
            .and_then(|line| {
                head_index
                    .rows_for_range(CommentAnchorSide::Head, line, line)
                    .next()
            });
        let base_row = content(&unified, 1)
            .source
            .line_for_side(CommentAnchorSide::Base)
            .and_then(|line| {
                base_index
                    .rows_for_range(CommentAnchorSide::Base, line, line)
                    .next()
            });

        assert_eq!(head_row, Some(RowIndex(1)));
        assert_eq!(base_row, Some(RowIndex(1)));
    }
}
