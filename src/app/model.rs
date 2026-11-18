//! UI-agnostic application model.
//!
//! `AppModel` describes the conceptual review UI that any frontend can render.
//! It intentionally excludes terminal cells, pixels, ratatui layout, and other
//! backend-specific geometry.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use crate::app::{AppState, CommentAnchorCapture, FileListSectionFocus, VisualSelectionMode};
use crate::config::DiffAlgorithm;
use crate::core::TextAnchor;
use crate::review_types::{
    self, AnchorStatus, ChangeKind, ConnectionContext, ContentMode, LineKind, PaneFocus,
    RenderVariant,
};

#[derive(Debug, Clone)]
pub struct AppModel {
    pub revision: u64,
    pub context: ConnectionContext,
    pub layout: AppLayout,
    pub file_list: FileList,
    pub diff: DiffPanel,
    pub comments_panel: CommentsPanel,
    pub focus: PaneFocus,
    pub search_results: Option<SearchResultsOverlay>,
    pub definition_results: Option<DefinitionResultsOverlay>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppLayout {
    pub file_list_visible: bool,
    pub diff_visible: bool,
    pub file_list_width: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileList {
    pub sections: Vec<FileListSection>,
    pub selected_file_id: Option<String>,
    pub selected_file_index: Option<usize>,
    pub scroll: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileListSectionKind {
    Unreviewed,
    Reviewed,
    UnresolvedComments,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileListSection {
    pub kind: FileListSectionKind,
    pub rows: Vec<FileListRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileListRow {
    pub kind: FileListRowKind,
    pub file_index: Option<usize>,
    pub file_id: String,
    pub path: String,
    pub old_path: Option<String>,
    pub change_kind: ChangeKind,
    pub review_status: ReviewStatus,
    pub selected: bool,
    /// Number of unresolved comments on this file.
    pub unresolved_comment_count: usize,
    pub comment_id: Option<i64>,
    pub comment_line_start: Option<i64>,
    pub comment_preview: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileListRowKind {
    File,
    Comment,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewStatus {
    Unreviewed,
    Reviewed {
        at: String,
        reviewed_commit: Option<String>,
    },
    Changed {
        at: String,
        reviewed_commit: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffPanel {
    pub selected_file_index: Option<usize>,
    pub file_id: Option<String>,
    pub path: Option<String>,
    pub review_status: Option<ReviewStatus>,
    pub content_mode: ContentMode,
    pub render_variant: RenderVariant,
    pub diff_algorithm: DiffAlgorithm,
    pub default_diff_algorithm: DiffAlgorithm,
    pub ignore_whitespace: bool,
    pub show_blame: bool,
    pub show_merge_base: bool,
    pub reviewed_diff_expanded: bool,
    pub is_binary: bool,
    pub diff_hash: Option<String>,
    pub hunks: Vec<DiffHunk>,
    pub head_content: Option<String>,
    pub base_content: Option<String>,
    pub head_blame: Vec<BlameLine>,
    pub base_blame: Vec<BlameLine>,
    pub scroll: usize,
    pub cursor: TextAnchor,
    pub search_query: Option<String>,
    pub search_highlights: Vec<TextRange>,
    pub current_search_highlight: Option<usize>,
    pub visual_selection: Option<VisualSelection>,
    pub pending_comment_anchor: Option<CommentAnchorCapture>,
    pub comments: Vec<CommentAttachment>,
    pub comment_markers: CommentMarkerSet,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentsPanel {
    pub visible: bool,
    pub comments: Vec<CommentItem>,
    pub selected_comment_id: Option<i64>,
    pub selected_index: Option<usize>,
    pub total: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentItem {
    pub id: i64,
    pub file_path: String,
    pub line_start: i64,
    pub line_end: i64,
    pub body: String,
    pub preview: String,
    pub resolved: bool,
    pub expanded: bool,
    pub selected: bool,
    pub current: bool,
    pub anchor_status: AnchorStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub content: String,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    pub hash: String,
    pub author: String,
    pub date: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentAttachment {
    pub id: i64,
    pub line_start: i64,
    pub line_end: i64,
    pub resolved: bool,
    pub anchor_status: AnchorStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommentMarkerSet {
    markers_by_line: BTreeMap<u32, MarkerCandidate>,
    current_comment: Option<CurrentComment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommentMarker {
    kind: Option<CommentMarkerKind>,
    resolved: bool,
    current: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommentMarkerKind {
    SingleLine,
    Start,
    End,
    Join,
}

impl CommentMarker {
    pub fn is_current(&self) -> bool {
        self.current
    }

    pub fn is_resolved(&self) -> bool {
        self.resolved
    }

    pub fn kind(&self) -> Option<CommentMarkerKind> {
        self.kind
    }
}

impl CommentMarkerSet {
    pub fn new(comments: &[CommentAttachment], current_line: Option<u32>) -> Self {
        let mut marker_candidates: BTreeMap<u32, MarkerCandidate> = BTreeMap::new();

        for comment in comments {
            let start = comment.line_start.max(1) as u32;
            let end = comment.line_end.max(comment.line_start).max(1) as u32;
            for line in start..=end {
                let candidate = MarkerCandidate::new(comment, start, end, line);
                marker_candidates
                    .entry(line)
                    .and_modify(|existing| {
                        if candidate.is_preferred_to(existing) {
                            *existing = candidate;
                        }
                    })
                    .or_insert(candidate);
            }
        }

        let current_comment = current_comment(comments, current_line);

        Self {
            markers_by_line: marker_candidates,
            current_comment,
        }
    }

    pub fn has_markers(&self) -> bool {
        !self.markers_by_line.is_empty()
    }

    pub fn marker_for_line(&self, line: Option<u32>) -> CommentMarker {
        let Some(line) = line else {
            return CommentMarker {
                kind: None,
                resolved: false,
                current: false,
            };
        };
        if let Some(current_comment) = self.current_comment {
            if current_comment.contains(line)
                && self.line_uses_current_marker(line, current_comment)
            {
                return CommentMarker {
                    kind: Some(current_comment.kind_for_line(line)),
                    resolved: current_comment.resolved,
                    current: true,
                };
            }
        }
        self.inactive_marker_for_line(line)
    }

    fn inactive_marker_for_line(&self, line: u32) -> CommentMarker {
        self.markers_by_line
            .get(&line)
            .copied()
            .map(|marker| CommentMarker {
                kind: Some(marker.kind),
                resolved: marker.resolved,
                current: false,
            })
            .unwrap_or(CommentMarker {
                kind: None,
                resolved: false,
                current: false,
            })
    }

    fn line_uses_current_marker(&self, line: u32, current_comment: CurrentComment) -> bool {
        if current_comment.is_boundary(line) {
            return true;
        }

        !self
            .markers_by_line
            .get(&line)
            .is_some_and(|marker| marker.kind.is_boundary_marker())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CurrentComment {
    id: i64,
    start: u32,
    end: u32,
    resolved: bool,
}

impl CurrentComment {
    fn contains(self, line: u32) -> bool {
        line >= self.start && line <= self.end
    }

    fn is_boundary(self, line: u32) -> bool {
        line == self.start || line == self.end
    }

    fn kind_for_line(self, line: u32) -> CommentMarkerKind {
        if self.start == self.end {
            CommentMarkerKind::SingleLine
        } else if line == self.start {
            CommentMarkerKind::Start
        } else if line == self.end {
            CommentMarkerKind::End
        } else {
            CommentMarkerKind::Join
        }
    }
}

fn current_comment(
    comments: &[CommentAttachment],
    current_line: Option<u32>,
) -> Option<CurrentComment> {
    let current_line = current_line?;
    comments
        .iter()
        .filter_map(|comment| {
            let start = comment.line_start.max(1) as u32;
            let end = comment.line_end.max(comment.line_start).max(1) as u32;
            if current_line < start || current_line > end {
                return None;
            }
            Some(CurrentComment {
                id: comment.id,
                start,
                end,
                resolved: comment.resolved,
            })
        })
        .min_by_key(|comment| {
            (
                comment.end.saturating_sub(comment.start),
                Reverse(comment.id),
            )
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct MarkerCandidate {
    kind: CommentMarkerKind,
    resolved: bool,
    range_len: u32,
    comment_id: i64,
}

impl MarkerCandidate {
    fn new(comment: &CommentAttachment, start: u32, end: u32, line: u32) -> Self {
        let kind = if start == end {
            CommentMarkerKind::SingleLine
        } else if line == start {
            CommentMarkerKind::Start
        } else if line == end {
            CommentMarkerKind::End
        } else {
            CommentMarkerKind::Join
        };

        Self {
            kind,
            resolved: comment.resolved,
            range_len: end.saturating_sub(start).saturating_add(1),
            comment_id: comment.id,
        }
    }

    fn is_preferred_to(self, other: &Self) -> bool {
        self.kind.priority() > other.kind.priority()
            || (self.kind.priority() == other.kind.priority()
                && (self.range_len < other.range_len
                    || (self.range_len == other.range_len && self.comment_id > other.comment_id)))
    }
}

impl CommentMarkerKind {
    fn priority(self) -> u8 {
        match self {
            CommentMarkerKind::Join => 0,
            CommentMarkerKind::SingleLine => 1,
            CommentMarkerKind::Start | CommentMarkerKind::End => 2,
        }
    }

    fn is_boundary_marker(self) -> bool {
        matches!(self, Self::SingleLine | Self::Start | Self::End)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRange {
    pub line: usize,
    pub column_start: usize,
    pub column_end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisualSelection {
    pub mode: VisualSelectionMode,
    pub start: TextAnchor,
    pub end: TextAnchor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResultsOverlay {
    pub query: String,
    pub diff_only: bool,
    pub matches: Vec<SearchResultItem>,
    pub selected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResultItem {
    pub file_path: String,
    pub line_number: u32,
    pub line_content: String,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionResultsOverlay {
    pub symbol: String,
    pub definitions: Vec<DefinitionResultItem>,
    pub selected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionResultItem {
    pub file_path: String,
    pub line_number: u32,
    pub line_content: String,
    pub selected: bool,
}

impl AppModel {
    pub fn from_state(state: &AppState) -> Self {
        Self {
            revision: state.model_revision(),
            context: state.context.clone(),
            layout: AppLayout {
                file_list_visible: state.show_file_list,
                diff_visible: state.show_diff_pane,
                file_list_width: state.file_list_width,
            },
            file_list: file_list_model(state),
            diff: diff_panel_model(state),
            comments_panel: comments_panel_model(state),
            focus: state.pane_focus,
            search_results: search_results_model(state),
            definition_results: definition_results_model(state),
        }
    }

    pub fn file_list_row_count(&self) -> usize {
        self.file_list
            .sections
            .iter()
            .map(|section| section.rows.len())
            .sum()
    }
}

fn search_results_model(state: &AppState) -> Option<SearchResultsOverlay> {
    let results = state.search_results.as_ref()?;
    Some(SearchResultsOverlay {
        query: results.query.clone(),
        diff_only: results.diff_only,
        matches: results
            .matches
            .iter()
            .enumerate()
            .map(|(index, m)| SearchResultItem {
                file_path: m.file_path.clone(),
                line_number: m.line_number,
                line_content: m.line_content.clone(),
                selected: index == results.selected,
            })
            .collect(),
        selected: results.selected,
    })
}

fn definition_results_model(state: &AppState) -> Option<DefinitionResultsOverlay> {
    let results = state.definition_results.as_ref()?;
    Some(DefinitionResultsOverlay {
        symbol: results.symbol.clone(),
        definitions: results
            .definitions
            .iter()
            .enumerate()
            .map(|(index, definition)| DefinitionResultItem {
                file_path: definition.file_path.clone(),
                line_number: definition.line_number,
                line_content: definition.line_content.clone(),
                selected: index == results.selected,
            })
            .collect(),
        selected: results.selected,
    })
}

fn file_list_model(state: &AppState) -> FileList {
    let unreviewed_count = state.unreviewed_count();
    let selected_file_id = state
        .selected_file_entry()
        .map(|entry| entry.change.path.clone());
    let selected_section = selected_file_list_section(state);

    // Build a lookup of unresolved comment counts per file path.
    let mut unresolved_counts: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for comment in &state.comments {
        if !comment.resolved {
            *unresolved_counts
                .entry(comment.file_path.as_str())
                .or_default() += 1;
        }
    }

    let unreviewed_rows = state
        .files
        .iter()
        .take(unreviewed_count)
        .enumerate()
        .map(|(index, entry)| {
            file_row_model(
                index,
                state.selected_file,
                selected_section == FileListSectionKind::Unreviewed,
                entry,
                &unresolved_counts,
            )
        })
        .collect();
    let reviewed_rows = state
        .files
        .iter()
        .enumerate()
        .skip(unreviewed_count)
        .map(|(index, entry)| {
            file_row_model(
                index,
                state.selected_file,
                selected_section == FileListSectionKind::Reviewed,
                entry,
                &unresolved_counts,
            )
        })
        .collect();
    let unresolved_rows = unresolved_comment_rows(state, &unresolved_counts);

    FileList {
        sections: vec![
            FileListSection {
                kind: FileListSectionKind::Unreviewed,
                rows: unreviewed_rows,
            },
            FileListSection {
                kind: FileListSectionKind::Reviewed,
                rows: reviewed_rows,
            },
            FileListSection {
                kind: FileListSectionKind::UnresolvedComments,
                rows: unresolved_rows,
            },
        ],
        selected_file_id,
        selected_file_index: state
            .files
            .get(state.selected_file)
            .map(|_| state.selected_file),
        scroll: state.file_list_scroll,
    }
}

fn file_row_model(
    index: usize,
    selected_file: usize,
    section_selected: bool,
    entry: &crate::review_types::FileEntry,
    unresolved_counts: &std::collections::HashMap<&str, usize>,
) -> FileListRow {
    let count = unresolved_counts
        .get(entry.change.path.as_str())
        .copied()
        .unwrap_or(0);
    FileListRow {
        kind: FileListRowKind::File,
        file_index: Some(index),
        file_id: entry.change.path.clone(),
        path: entry.change.path.clone(),
        old_path: entry.change.old_path.clone(),
        change_kind: entry.change.kind,
        review_status: ReviewStatus::from(&entry.status),
        selected: section_selected && index == selected_file,
        unresolved_comment_count: count,
        comment_id: None,
        comment_line_start: None,
        comment_preview: None,
    }
}

fn selected_file_list_section(state: &AppState) -> FileListSectionKind {
    match state.file_list_section_focus {
        FileListSectionFocus::UnresolvedComments => FileListSectionKind::UnresolvedComments,
        FileListSectionFocus::Unreviewed | FileListSectionFocus::Reviewed => {
            if state.selected_file < state.unreviewed_count() {
                FileListSectionKind::Unreviewed
            } else {
                FileListSectionKind::Reviewed
            }
        }
    }
}

fn unresolved_comment_rows(
    state: &AppState,
    unresolved_counts: &std::collections::HashMap<&str, usize>,
) -> Vec<FileListRow> {
    let mut rows = Vec::new();
    let current_file_paths: std::collections::HashSet<&str> = state
        .files
        .iter()
        .map(|entry| entry.change.path.as_str())
        .collect();
    for (index, entry) in state.files.iter().enumerate() {
        let mut comments: Vec<_> = state
            .comments
            .iter()
            .filter(|comment| !comment.resolved && comment.file_path == entry.change.path)
            .collect();
        if comments.is_empty() {
            continue;
        }
        comments.sort_by_key(|comment| (comment.line_start, comment.line_end, comment.id));
        rows.push(file_row_model(
            index,
            state.selected_file,
            state.file_list_section_focus == FileListSectionFocus::UnresolvedComments
                && state.selected_comment_id.is_none(),
            entry,
            unresolved_counts,
        ));
        for comment in comments {
            rows.push(comment_row_model(
                index,
                state,
                entry,
                comment,
                unresolved_counts,
            ));
        }
    }
    let mut external_paths: Vec<&str> = unresolved_counts
        .keys()
        .copied()
        .filter(|path| !current_file_paths.contains(*path))
        .collect();
    external_paths.sort_unstable();
    for path in external_paths {
        let mut comments: Vec<_> = state
            .comments
            .iter()
            .filter(|comment| !comment.resolved && comment.file_path == path)
            .collect();
        comments.sort_by_key(|comment| (comment.line_start, comment.line_end, comment.id));
        rows.push(external_file_row_model(path, state, unresolved_counts));
        for comment in comments {
            rows.push(external_comment_row_model(
                path,
                state,
                comment,
                unresolved_counts,
            ));
        }
    }
    rows
}

fn external_file_row_model(
    path: &str,
    state: &AppState,
    unresolved_counts: &std::collections::HashMap<&str, usize>,
) -> FileListRow {
    let count = unresolved_counts.get(path).copied().unwrap_or(0);
    FileListRow {
        kind: FileListRowKind::File,
        file_index: None,
        file_id: path.to_string(),
        path: path.to_string(),
        old_path: None,
        change_kind: crate::review_types::ChangeKind::Modified,
        review_status: ReviewStatus::Unreviewed,
        selected: state.file_list_section_focus == FileListSectionFocus::UnresolvedComments
            && state.selected_comment_id.is_none(),
        unresolved_comment_count: count,
        comment_id: None,
        comment_line_start: None,
        comment_preview: None,
    }
}

fn comment_row_model(
    index: usize,
    state: &AppState,
    entry: &crate::review_types::FileEntry,
    comment: &crate::review_types::Comment,
    unresolved_counts: &std::collections::HashMap<&str, usize>,
) -> FileListRow {
    let count = unresolved_counts
        .get(entry.change.path.as_str())
        .copied()
        .unwrap_or(0);
    FileListRow {
        kind: FileListRowKind::Comment,
        file_index: Some(index),
        file_id: entry.change.path.clone(),
        path: entry.change.path.clone(),
        old_path: None,
        change_kind: entry.change.kind,
        review_status: ReviewStatus::from(&entry.status),
        selected: state.file_list_section_focus == FileListSectionFocus::UnresolvedComments
            && state.selected_comment_id == Some(comment.id),
        unresolved_comment_count: count,
        comment_id: Some(comment.id),
        comment_line_start: Some(comment.line_start),
        comment_preview: Some(comment_preview(&comment.body)),
    }
}

fn external_comment_row_model(
    path: &str,
    state: &AppState,
    comment: &crate::review_types::Comment,
    unresolved_counts: &std::collections::HashMap<&str, usize>,
) -> FileListRow {
    let count = unresolved_counts.get(path).copied().unwrap_or(0);
    FileListRow {
        kind: FileListRowKind::Comment,
        file_index: None,
        file_id: path.to_string(),
        path: path.to_string(),
        old_path: None,
        change_kind: crate::review_types::ChangeKind::Modified,
        review_status: ReviewStatus::Unreviewed,
        selected: state.file_list_section_focus == FileListSectionFocus::UnresolvedComments
            && state.selected_comment_id == Some(comment.id),
        unresolved_comment_count: count,
        comment_id: Some(comment.id),
        comment_line_start: Some(comment.line_start),
        comment_preview: Some(comment_preview(&comment.body)),
    }
}

fn diff_panel_model(state: &AppState) -> DiffPanel {
    let selected = state.selected_file_entry();
    let hunks = selected
        .map(|entry| {
            entry
                .diff
                .hunks
                .iter()
                .map(|hunk| DiffHunk {
                    header: hunk.header.clone(),
                    old_start: hunk.old_start,
                    old_lines: hunk.old_lines,
                    new_start: hunk.new_start,
                    new_lines: hunk.new_lines,
                    lines: hunk
                        .lines
                        .iter()
                        .map(|line| DiffLine {
                            kind: line.kind,
                            content: line.content.clone(),
                            old_lineno: line.old_lineno,
                            new_lineno: line.new_lineno,
                        })
                        .collect(),
                })
                .collect()
        })
        .unwrap_or_default();
    let comments = selected
        .map(|entry| comment_attachments_for_file(&state.comments, &entry.change.path))
        .unwrap_or_default();
    let comment_markers = comment_marker_set_for_current_view(state, &comments);

    DiffPanel {
        selected_file_index: selected.map(|_| state.selected_file),
        file_id: selected.map(|entry| entry.change.path.clone()),
        path: selected.map(|entry| entry.change.path.clone()),
        review_status: selected.map(|entry| ReviewStatus::from(&entry.status)),
        content_mode: state.content_mode,
        render_variant: state.render_variant,
        diff_algorithm: state.diff_algorithm,
        default_diff_algorithm: state.default_diff_algorithm,
        ignore_whitespace: state.ignore_whitespace,
        show_blame: state.show_blame,
        show_merge_base: state.show_merge_base,
        reviewed_diff_expanded: state.reviewed_diff_expanded,
        is_binary: selected.is_some_and(|entry| entry.diff.is_binary),
        diff_hash: selected.map(|entry| entry.diff.diff_hash.clone()),
        hunks,
        head_content: state.head_content.clone(),
        base_content: state.base_content.clone(),
        head_blame: state.head_blame.iter().map(BlameLine::from).collect(),
        base_blame: state.base_blame.iter().map(BlameLine::from).collect(),
        scroll: state.diff_scroll,
        cursor: TextAnchor {
            line: state.diff_line_cursor,
            column: state.diff_col_cursor,
        },
        search_query: state.diff_search_query.clone(),
        search_highlights: state
            .diff_search_matches
            .iter()
            .map(|(line, start, end)| TextRange {
                line: *line,
                column_start: *start,
                column_end: *end,
            })
            .collect(),
        current_search_highlight: state
            .diff_search_matches
            .get(state.diff_search_current)
            .map(|_| state.diff_search_current),
        visual_selection: state
            .visual_selection
            .as_ref()
            .map(|selection| VisualSelection {
                mode: selection.mode,
                start: selection.start,
                end: selection.end,
            }),
        pending_comment_anchor: state.pending_comment_anchor.clone(),
        comments,
        comment_markers,
    }
}

fn comment_marker_set_for_current_view(
    state: &AppState,
    comments: &[CommentAttachment],
) -> CommentMarkerSet {
    match state.content_mode {
        ContentMode::FullFile => match state.render_variant {
            RenderVariant::HeadVersion => CommentMarkerSet::new(comments, current_head_line(state)),
            RenderVariant::BaseVersion => CommentMarkerSet::new(&[], None),
            _ => CommentMarkerSet::new(comments, current_head_line(state)),
        },
        ContentMode::Diff => CommentMarkerSet::new(comments, current_head_line(state)),
    }
}

fn comment_attachments_for_file(
    comments: &[review_types::Comment],
    file_path: &str,
) -> Vec<CommentAttachment> {
    comments
        .iter()
        .filter(|comment| comment.file_path == file_path)
        .map(|comment| CommentAttachment {
            id: comment.id,
            line_start: comment.line_start,
            line_end: comment.line_end,
            resolved: comment.resolved,
            anchor_status: comment.anchor_status,
        })
        .collect()
}

fn comments_panel_model(state: &AppState) -> CommentsPanel {
    let selected_path = state
        .selected_file_entry()
        .map(|entry| entry.change.path.as_str());
    let current_line = current_head_line(state).map(i64::from);
    let mut file_comments: Vec<&review_types::Comment> = state
        .comments
        .iter()
        .filter(|comment| selected_path.is_none_or(|path| comment.file_path == path))
        .collect();
    file_comments.sort_by_key(|comment| (comment.line_start, comment.line_end, comment.id));
    let file_comment_ids: Vec<i64> = file_comments.iter().map(|comment| comment.id).collect();
    let detail_id = if state.pane_focus == PaneFocus::Comments {
        state.selected_comment_id
    } else {
        current_line.and_then(|line| {
            selected_path.and_then(|path| {
                state
                    .comments
                    .iter()
                    .filter(|comment| {
                        comment.file_path == path
                            && comment.line_start <= line
                            && comment.line_end >= line
                    })
                    .min_by_key(|comment| {
                        (
                            comment.line_end.saturating_sub(comment.line_start),
                            comment.id,
                        )
                    })
                    .map(|comment| comment.id)
            })
        })
    };
    let selected_comment_out_of_range = state.selected_comment_id.and_then(|id| {
        state
            .comments
            .iter()
            .find(|comment| Some(comment.file_path.as_str()) != selected_path && comment.id == id)
    });
    let comments = detail_id
        .and_then(|id| state.comments.iter().find(|comment| comment.id == id))
        .filter(|comment| {
            selected_path.is_none_or(|path| comment.file_path == path)
                || Some(comment.id) == state.selected_comment_id
        })
        .or(selected_comment_out_of_range)
        .map(|comment| {
            let current = selected_path.is_some_and(|path| comment.file_path == path)
                && current_line
                    .is_some_and(|line| comment.line_start <= line && comment.line_end >= line);
            let expanded =
                !comment.resolved || current || state.expanded_comment_ids.contains(&comment.id);
            vec![CommentItem {
                id: comment.id,
                file_path: comment.file_path.clone(),
                line_start: comment.line_start,
                line_end: comment.line_end,
                body: comment.body.clone(),
                preview: comment_preview(&comment.body),
                resolved: comment.resolved,
                expanded,
                selected: Some(comment.id) == state.selected_comment_id,
                current,
                anchor_status: comment.anchor_status,
            }]
        })
        .unwrap_or_default();
    CommentsPanel {
        visible: state.show_comments_panel,
        comments,
        selected_comment_id: state.selected_comment_id,
        selected_index: detail_id.and_then(|id| {
            file_comment_ids
                .iter()
                .position(|comment_id| *comment_id == id)
                .map(|index| index + 1)
        }),
        total: file_comment_ids.len(),
    }
}

fn comment_preview(body: &str) -> String {
    body.lines()
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .unwrap_or("")
        .chars()
        .take(80)
        .collect()
}

fn current_head_line(state: &AppState) -> Option<u32> {
    match state.content_mode {
        ContentMode::FullFile => match state.render_variant {
            RenderVariant::HeadVersion => Some(state.diff_line_cursor.saturating_add(1) as u32),
            RenderVariant::BaseVersion => None,
            _ => None,
        },
        ContentMode::Diff => diff_new_line_at_row(state, state.diff_line_cursor),
    }
}

fn diff_new_line_at_row(state: &AppState, row: usize) -> Option<u32> {
    if state.render_variant == RenderVariant::SideBySide {
        return side_by_side_new_line_at_row(state, row);
    }
    inline_new_line_at_row(state, row)
}

fn inline_new_line_at_row(state: &AppState, row: usize) -> Option<u32> {
    let entry = state.selected_file_entry()?;
    let head_lines = state
        .head_content
        .as_deref()
        .map(|content| content.lines().count())
        .unwrap_or(0);
    if entry.diff.hunks.is_empty() {
        return (row < head_lines).then_some(row.saturating_add(1) as u32);
    }

    let mut display_row = 0usize;
    let mut new_cursor = 1u32;
    for hunk in &entry.diff.hunks {
        while new_cursor < hunk.new_start && (new_cursor as usize) <= head_lines {
            if display_row == row {
                return Some(new_cursor);
            }
            display_row = display_row.saturating_add(1);
            new_cursor = new_cursor.saturating_add(1);
        }

        for line in &hunk.lines {
            if display_row == row {
                return line.new_lineno;
            }
            display_row = display_row.saturating_add(1);
            if line.new_lineno.is_some() {
                new_cursor = new_cursor.saturating_add(1);
            }
        }
    }

    while (new_cursor as usize) <= head_lines {
        if display_row == row {
            return Some(new_cursor);
        }
        display_row = display_row.saturating_add(1);
        new_cursor = new_cursor.saturating_add(1);
    }
    None
}

fn side_by_side_new_line_at_row(state: &AppState, row: usize) -> Option<u32> {
    let entry = state.selected_file_entry()?;
    let head_lines = state
        .head_content
        .as_deref()
        .map(|content| content.lines().count())
        .unwrap_or(0);
    if entry.diff.hunks.is_empty() {
        return (row < head_lines).then_some(row.saturating_add(1) as u32);
    }

    let mut display_row = 0usize;
    let mut new_cursor = 1u32;
    for hunk in &entry.diff.hunks {
        while new_cursor < hunk.new_start && (new_cursor as usize) <= head_lines {
            if display_row == row {
                return Some(new_cursor);
            }
            display_row = display_row.saturating_add(1);
            new_cursor = new_cursor.saturating_add(1);
        }

        let mut index = 0;
        while index < hunk.lines.len() {
            let line = &hunk.lines[index];
            if line.kind == LineKind::Context {
                if display_row == row {
                    return line.new_lineno;
                }
                display_row = display_row.saturating_add(1);
                new_cursor = new_cursor.saturating_add(1);
                index += 1;
                continue;
            }

            let block_start = index;
            let mut del_end = index;
            while del_end < hunk.lines.len() && hunk.lines[del_end].kind == LineKind::Deletion {
                del_end += 1;
            }
            let mut add_end = del_end;
            while add_end < hunk.lines.len() && hunk.lines[add_end].kind == LineKind::Addition {
                add_end += 1;
            }

            let deletion_count = del_end.saturating_sub(block_start);
            let additions = &hunk.lines[del_end..add_end];
            let max_count = deletion_count.max(additions.len());
            for offset in 0..max_count {
                let new_lineno = additions.get(offset).and_then(|line| line.new_lineno);
                if display_row == row {
                    return new_lineno;
                }
                display_row = display_row.saturating_add(1);
                if new_lineno.is_some() {
                    new_cursor = new_cursor.saturating_add(1);
                }
            }
            index = add_end;
        }
    }

    while (new_cursor as usize) <= head_lines {
        if display_row == row {
            return Some(new_cursor);
        }
        display_row = display_row.saturating_add(1);
        new_cursor = new_cursor.saturating_add(1);
    }
    None
}

impl From<&review_types::ReviewStatus> for ReviewStatus {
    fn from(status: &review_types::ReviewStatus) -> Self {
        match status {
            review_types::ReviewStatus::Unreviewed => Self::Unreviewed,
            review_types::ReviewStatus::Reviewed {
                at,
                reviewed_commit,
            } => Self::Reviewed {
                at: at.clone(),
                reviewed_commit: reviewed_commit.clone(),
            },
            review_types::ReviewStatus::Changed {
                at,
                reviewed_commit,
            } => Self::Changed {
                at: at.clone(),
                reviewed_commit: reviewed_commit.clone(),
            },
        }
    }
}

impl From<&crate::git::BlameLine> for BlameLine {
    fn from(line: &crate::git::BlameLine) -> Self {
        Self {
            hash: line.hash.clone(),
            author: line.author.clone(),
            date: line.date.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::config::Config;
    use crate::review_types::{
        self, AnchorStatus, ChangeKind, DiffContent, FileChange, FileEntry, LineKind,
    };

    fn test_context() -> ConnectionContext {
        ConnectionContext {
            repo_root: "/repo".to_string(),
            worktree: "/repo".to_string(),
            base_ref: "main".to_string(),
            head_ref: "feature".to_string(),
            merge_base: "abc123".to_string(),
        }
    }

    fn file(
        path: &str,
        status: review_types::ReviewStatus,
        hunks: Vec<review_types::DiffHunk>,
    ) -> FileEntry {
        FileEntry {
            change: FileChange {
                path: path.to_string(),
                old_path: None,
                kind: ChangeKind::Modified,
            },
            status,
            diff: DiffContent {
                hunks,
                is_binary: false,
                diff_hash: format!("hash-{path}"),
            },
        }
    }

    fn hunk() -> review_types::DiffHunk {
        review_types::DiffHunk {
            old_start: 1,
            old_lines: 2,
            new_start: 1,
            new_lines: 2,
            header: "@@ -1,2 +1,2 @@".to_string(),
            lines: vec![
                review_types::DiffLine {
                    kind: LineKind::Context,
                    content: "fn main() {".to_string(),
                    old_lineno: Some(1),
                    new_lineno: Some(1),
                },
                review_types::DiffLine {
                    kind: LineKind::Addition,
                    content: "    run();".to_string(),
                    old_lineno: None,
                    new_lineno: Some(2),
                },
            ],
        }
    }

    fn comment(id: i64, start: i64, end: i64, resolved: bool) -> CommentAttachment {
        CommentAttachment {
            id,
            line_start: start,
            line_end: end,
            resolved,
            anchor_status: AnchorStatus::Anchored,
        }
    }

    fn stored_comment(id: i64, file_path: &str, resolved: bool) -> review_types::Comment {
        review_types::Comment {
            id,
            merge_base: "abc123".to_string(),
            head_ref: "feature".to_string(),
            file_path: file_path.to_string(),
            line_start: 2,
            line_end: 2,
            char_start: None,
            char_end: None,
            anchor_text: "anchor".to_string(),
            context_before: String::new(),
            context_after: String::new(),
            body: "comment".to_string(),
            resolved,
            created_at: "2026-06-28T00:00:00+10:00".to_string(),
            updated_at: "2026-06-28T00:00:00+10:00".to_string(),
            anchor_status: AnchorStatus::Anchored,
        }
    }

    fn assert_marker(
        markers: &CommentMarkerSet,
        line: u32,
        kind: Option<CommentMarkerKind>,
        resolved: bool,
        current: bool,
    ) {
        let marker = markers.marker_for_line(Some(line));
        assert_eq!(marker.kind(), kind, "line {line}");
        assert_eq!(marker.is_resolved(), resolved, "line {line}");
        assert_eq!(marker.is_current(), current, "line {line}");
    }

    #[test]
    fn comment_markers_describe_single_line_resolution_state() {
        let markers =
            CommentMarkerSet::new(&[comment(1, 3, 3, false), comment(2, 5, 5, true)], None);

        assert!(markers.has_markers());
        assert_marker(
            &markers,
            3,
            Some(CommentMarkerKind::SingleLine),
            false,
            false,
        );
        assert_marker(
            &markers,
            5,
            Some(CommentMarkerKind::SingleLine),
            true,
            false,
        );
        assert_marker(&markers, 4, None, false, false);
    }

    #[test]
    fn comment_markers_connect_multiline_ranges() {
        let markers = CommentMarkerSet::new(&[comment(1, 3, 5, false)], None);

        assert_marker(&markers, 3, Some(CommentMarkerKind::Start), false, false);
        assert_marker(&markers, 4, Some(CommentMarkerKind::Join), false, false);
        assert_marker(&markers, 5, Some(CommentMarkerKind::End), false, false);
    }

    #[test]
    fn comment_markers_merge_nested_comments_to_one_column() {
        let markers =
            CommentMarkerSet::new(&[comment(1, 3, 5, false), comment(2, 4, 4, true)], None);

        assert_marker(&markers, 3, Some(CommentMarkerKind::Start), false, false);
        assert_marker(
            &markers,
            4,
            Some(CommentMarkerKind::SingleLine),
            true,
            false,
        );
        assert_marker(&markers, 5, Some(CommentMarkerKind::End), false, false);
    }

    #[test]
    fn multiline_boundary_wins_over_nested_single_line_marker() {
        let markers = CommentMarkerSet::new(
            &[
                comment(1, 31, 41, false),
                comment(2, 36, 40, true),
                comment(3, 36, 36, false),
            ],
            None,
        );

        assert_marker(&markers, 36, Some(CommentMarkerKind::Start), true, false);
        assert_marker(&markers, 37, Some(CommentMarkerKind::Join), true, false);
        assert_marker(&markers, 40, Some(CommentMarkerKind::End), true, false);
    }

    #[test]
    fn current_comment_marks_innermost_comment_range() {
        let markers = CommentMarkerSet::new(
            &[
                comment(1, 31, 41, false),
                comment(2, 36, 40, true),
                comment(3, 36, 36, false),
            ],
            Some(38),
        );

        assert_marker(&markers, 36, Some(CommentMarkerKind::Start), true, true);
        assert_marker(&markers, 38, Some(CommentMarkerKind::Join), true, true);
        assert_marker(&markers, 41, Some(CommentMarkerKind::End), false, false);
    }

    #[test]
    fn current_outer_comment_leaves_nested_boundaries_inactive() {
        let markers = CommentMarkerSet::new(
            &[
                comment(1, 31, 41, false),
                comment(2, 36, 40, true),
                comment(3, 37, 37, false),
            ],
            Some(32),
        );

        assert_marker(&markers, 31, Some(CommentMarkerKind::Start), false, true);
        assert_marker(&markers, 34, Some(CommentMarkerKind::Join), false, true);
        assert_marker(&markers, 36, Some(CommentMarkerKind::Start), true, false);
        assert_marker(
            &markers,
            37,
            Some(CommentMarkerKind::SingleLine),
            false,
            false,
        );
        assert_marker(&markers, 40, Some(CommentMarkerKind::End), true, false);
        assert_marker(&markers, 41, Some(CommentMarkerKind::End), false, true);
    }

    fn reviewed() -> review_types::ReviewStatus {
        review_types::ReviewStatus::Reviewed {
            at: "2026-01-01T00:00:00Z".to_string(),
            reviewed_commit: Some("abc".to_string()),
        }
    }

    fn changed() -> review_types::ReviewStatus {
        review_types::ReviewStatus::Changed {
            at: "2026-01-01T00:00:00Z".to_string(),
            reviewed_commit: Some("def".to_string()),
        }
    }

    #[test]
    fn model_projects_file_sections_and_selection() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![
                file("c.rs", reviewed(), Vec::new()),
                file("b.rs", changed(), Vec::new()),
                file("a.rs", review_types::ReviewStatus::Unreviewed, Vec::new()),
            ],
        );
        app.state.selected_file = app
            .state
            .files
            .iter()
            .position(|entry| entry.change.path == "b.rs")
            .expect("b.rs should be present");

        let model = app.model();

        assert_eq!(model.file_list.selected_file_id, Some("b.rs".to_string()));
        assert_eq!(model.file_list.selected_file_index, Some(1));
        assert_eq!(model.file_list.scroll, 0);
        assert_eq!(model.file_list.sections.len(), 3);
        assert_eq!(
            model.file_list.sections[0].kind,
            FileListSectionKind::Unreviewed
        );
        assert_eq!(
            model.file_list.sections[1].kind,
            FileListSectionKind::Reviewed
        );
        assert_eq!(
            model.file_list.sections[2].kind,
            FileListSectionKind::UnresolvedComments
        );
        assert_eq!(
            model.file_list.sections[0]
                .rows
                .iter()
                .map(|row| row.path.as_str())
                .collect::<Vec<_>>(),
            vec!["a.rs", "b.rs"]
        );
        assert!(model.file_list.sections[0].rows[1].selected);
        assert_eq!(model.file_list.sections[0].rows[1].file_index, Some(1));
        assert!(matches!(
            model.file_list.sections[0].rows[1].review_status,
            ReviewStatus::Changed { .. }
        ));
        assert_eq!(model.file_list.sections[1].rows[0].path, "c.rs");
        assert!(model.file_list.sections[2].rows.is_empty());
    }

    #[test]
    fn model_projects_unresolved_comment_counts_for_file_indicators() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![
                file("a.rs", review_types::ReviewStatus::Unreviewed, Vec::new()),
                file("b.rs", review_types::ReviewStatus::Unreviewed, Vec::new()),
            ],
        );
        app.state.comments = vec![
            stored_comment(1, "a.rs", false),
            stored_comment(2, "a.rs", true),
            stored_comment(3, "b.rs", true),
        ];

        let model = app.model();
        let rows = &model.file_list.sections[0].rows;

        assert_eq!(rows[0].path, "a.rs");
        assert_eq!(rows[0].unresolved_comment_count, 1);
        assert_eq!(rows[1].path, "b.rs");
        assert_eq!(rows[1].unresolved_comment_count, 0);
    }

    #[test]
    fn model_projects_unresolved_comments_section() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![
                file("a.rs", review_types::ReviewStatus::Unreviewed, Vec::new()),
                file("b.rs", review_types::ReviewStatus::Unreviewed, Vec::new()),
            ],
        );
        let mut first = stored_comment(4, "a.rs", false);
        first.line_start = 12;
        first.line_end = 14;
        first.body = "First line of the comment\nsecond line".to_string();
        let mut second = stored_comment(5, "a.rs", false);
        second.line_start = 4;
        second.body = "Earlier comment".to_string();
        app.state.comments = vec![first, stored_comment(6, "b.rs", true), second];
        app.state.file_list_section_focus = FileListSectionFocus::UnresolvedComments;
        app.state.selected_comment_id = Some(5);

        let model = app.model();
        let rows = &model.file_list.sections[2].rows;

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].kind, FileListRowKind::File);
        assert_eq!(rows[0].path, "a.rs");
        assert_eq!(rows[1].kind, FileListRowKind::Comment);
        assert_eq!(rows[1].comment_id, Some(5));
        assert_eq!(rows[1].comment_line_start, Some(4));
        assert_eq!(rows[1].comment_preview.as_deref(), Some("Earlier comment"));
        assert!(rows[1].selected);
        assert_eq!(rows[2].comment_id, Some(4));
        assert_eq!(
            rows[2].comment_preview.as_deref(),
            Some("First line of the comment")
        );
    }

    #[test]
    fn model_projects_unresolved_comments_for_files_outside_current_diff() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![file(
                "a.rs",
                review_types::ReviewStatus::Unreviewed,
                Vec::new(),
            )],
        );
        let mut comment = stored_comment(8, "mcp.rs", false);
        comment.line_start = 22;
        comment.body = "Previous session feedback".to_string();
        app.state.comments = vec![comment];
        app.state.file_list_section_focus = FileListSectionFocus::UnresolvedComments;
        app.state.selected_comment_id = Some(8);

        let model = app.model();
        let rows = &model.file_list.sections[2].rows;

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].kind, FileListRowKind::File);
        assert_eq!(rows[0].path, "mcp.rs");
        assert_eq!(rows[0].file_index, None);
        assert_eq!(rows[1].kind, FileListRowKind::Comment);
        assert_eq!(rows[1].comment_id, Some(8));
        assert_eq!(rows[1].file_index, None);
        assert!(rows[1].selected);
    }

    #[test]
    fn model_projects_diff_content_cursor_and_search_highlights() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![file(
                "src/main.rs",
                review_types::ReviewStatus::Unreviewed,
                vec![hunk()],
            )],
        );
        app.state.diff_line_cursor = 3;
        app.state.diff_col_cursor = 7;
        app.state.diff_search_query = Some("run".to_string());
        app.state.diff_search_matches = vec![(3, 10, 13), (8, 2, 5)];
        app.state.diff_search_current = 1;

        let model = app.model();

        assert_eq!(model.diff.file_id, Some("src/main.rs".to_string()));
        assert_eq!(model.diff.selected_file_index, Some(0));
        assert_eq!(model.diff.diff_hash, Some("hash-src/main.rs".to_string()));
        assert_eq!(model.diff.scroll, 0);
        assert_eq!(model.diff.cursor, TextAnchor { line: 3, column: 7 });
        assert_eq!(model.diff.hunks.len(), 1);
        assert_eq!(model.diff.hunks[0].header, "@@ -1,2 +1,2 @@");
        assert_eq!(model.diff.hunks[0].lines[1].kind, LineKind::Addition);
        assert_eq!(model.diff.search_query, Some("run".to_string()));
        assert_eq!(
            model.diff.search_highlights,
            vec![
                TextRange {
                    line: 3,
                    column_start: 10,
                    column_end: 13,
                },
                TextRange {
                    line: 8,
                    column_start: 2,
                    column_end: 5,
                },
            ]
        );
        assert_eq!(model.diff.current_search_highlight, Some(1));
    }

    #[test]
    fn model_projects_search_and_definition_overlays() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![file(
                "src/main.rs",
                review_types::ReviewStatus::Unreviewed,
                Vec::new(),
            )],
        );
        app.state.search_results = Some(crate::app::SearchResults {
            query: "needle".to_string(),
            diff_only: true,
            matches: vec![
                review_types::SearchMatch {
                    file_path: "src/main.rs".to_string(),
                    line_number: 10,
                    line_content: "first needle".to_string(),
                },
                review_types::SearchMatch {
                    file_path: "src/lib.rs".to_string(),
                    line_number: 20,
                    line_content: "second needle".to_string(),
                },
            ],
            selected: 1,
        });
        app.state.definition_results = Some(crate::app::DefinitionResults {
            symbol: "needle".to_string(),
            definitions: vec![review_types::DefinitionLocation {
                file_path: "src/main.rs".to_string(),
                line_number: 10,
                line_content: "fn needle()".to_string(),
            }],
            selected: 0,
        });

        let model = app.model();

        let search = model.search_results.expect("search overlay");
        assert_eq!(search.query, "needle");
        assert!(search.diff_only);
        assert_eq!(search.selected, 1);
        assert!(!search.matches[0].selected);
        assert!(search.matches[1].selected);
        assert_eq!(search.matches[1].file_path, "src/lib.rs");

        let definitions = model.definition_results.expect("definition overlay");
        assert_eq!(definitions.symbol, "needle");
        assert_eq!(definitions.selected, 0);
        assert!(definitions.definitions[0].selected);
        assert_eq!(definitions.definitions[0].line_content, "fn needle()");
    }

    #[test]
    fn model_projects_core_ui_concepts() {
        let app = App::new(
            Config::default(),
            test_context(),
            vec![file(
                "src/lib.rs",
                review_types::ReviewStatus::Unreviewed,
                Vec::new(),
            )],
        );

        let model = app.model();

        assert_eq!(model.focus, PaneFocus::FileList);
        assert_eq!(model.revision, app.state.model_revision());
        assert!(model.layout.file_list_visible);
        assert!(model.layout.diff_visible);
        assert_eq!(
            model.layout.file_list_width,
            Config::default().layout.file_list_width
        );
        assert!(model.search_results.is_none());
        assert!(model.definition_results.is_none());
    }
}
