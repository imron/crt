//! UI-agnostic application model.
//!
//! `AppModel` describes the conceptual review UI that any frontend can render.
//! It intentionally excludes terminal cells, pixels, ratatui layout, and other
//! backend-specific geometry.

use std::rc::Rc;

use crate::app::document::{
    ActiveDocument, ColumnIndex, ContentId as DocumentContentId, DiffDocumentBuilder,
    DiffDocumentInput, DocumentKey, DocumentPosition, RowIndex, RowSpan,
    VisibleSelection as DocumentVisibleSelection,
};
use crate::app::{
    ActiveDocumentState, AppState, FileListSectionFocus, VisualSelection as StateVisualSelection,
    VisualSelectionMode,
};
use crate::config::DiffAlgorithm;
use crate::core::TextAnchor;
use crate::review_types::{
    self, AnchorStatus, ChangeKind, ConnectionContext, ContentMode, PaneFocus, RenderVariant,
};

#[derive(Debug, Clone)]
pub struct AppModel<'a> {
    pub revision: u64,
    pub context: ConnectionContext,
    pub layout: AppLayout,
    pub file_list: FileList,
    pub diff: DiffPanel,
    pub active_document: Option<&'a ActiveDocumentState>,
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
    pub scroll: usize,
    pub cursor: TextAnchor,
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
pub struct BlameLine {
    pub hash: Rc<str>,
    pub author: Rc<str>,
    pub date: Rc<str>,
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

impl<'a> AppModel<'a> {
    pub fn from_state(state: &'a AppState) -> Self {
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
            active_document: state.active_document.as_ref(),
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

pub fn active_document_key(state: &AppState) -> Option<DocumentKey> {
    let entry = state.selected_file_entry()?;
    Some(DocumentKey {
        file_id: entry.change.path.clone(),
        diff_hash: entry.diff.diff_hash.clone(),
        content_mode: state.content_mode,
        render_variant: state.render_variant,
        diff_algorithm: state.diff_algorithm,
        ignore_whitespace: state.ignore_whitespace,
        show_blame: state.show_blame,
        head_content_id: content_id(state.head_content.as_ref()),
        base_content_id: content_id(state.base_content.as_ref()),
    })
}

pub fn build_active_document(state: &AppState, key: DocumentKey) -> Option<ActiveDocument> {
    let entry = state.selected_file_entry()?;
    let mut active_document = DiffDocumentBuilder::build(DiffDocumentInput {
        key,
        file_path: &entry.change.path,
        hunks: &entry.diff.hunks,
        head_content: state.head_content.as_deref(),
        base_content: state.base_content.as_deref(),
        head_blame: &state.head_blame,
        base_blame: &state.base_blame,
        comments: &state.comments,
        selected_comment_id: state.selected_comment_id,
        cursor: Some(RowIndex(state.diff_line_cursor)),
    })
    .ok()?;
    active_document.refresh_overlays(
        &entry.change.path,
        &state.comments,
        state.selected_comment_id,
        Some(RowIndex(state.diff_line_cursor)),
        document_visible_selection(state.visual_selection.as_ref()),
        Some(DocumentPosition {
            row: RowIndex(state.diff_line_cursor),
            column: ColumnIndex(state.diff_col_cursor),
        }),
        state.diff_search_query.as_deref(),
    );
    Some(active_document)
}

pub fn document_visible_selection(
    selection: Option<&StateVisualSelection>,
) -> Option<DocumentVisibleSelection> {
    let selection = selection?;
    match selection.mode {
        VisualSelectionMode::Line => {
            let start = selection.start.row.min(selection.end.row);
            let end = selection.start.row.max(selection.end.row);
            Some(DocumentVisibleSelection::Line(RowSpan { start, end }))
        }
        VisualSelectionMode::Text => Some(DocumentVisibleSelection::Text {
            start: selection.start,
            end: selection.end,
        }),
    }
}

fn content_id(content: Option<&String>) -> Option<DocumentContentId> {
    let content = content?;
    Some(DocumentContentId::from(format!(
        "{}:{:p}",
        content.len(),
        content.as_ptr()
    )))
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
            *unresolved_counts.entry(comment.file_path()).or_default() += 1;
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
            .filter(|comment| !comment.resolved && comment.file_path() == entry.change.path)
            .collect();
        if comments.is_empty() {
            continue;
        }
        comments.sort_by_key(|comment| (comment.line_start(), comment.line_end(), comment.id));
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
            .filter(|comment| !comment.resolved && comment.file_path() == path)
            .collect();
        comments.sort_by_key(|comment| (comment.line_start(), comment.line_end(), comment.id));
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
        comment_line_start: Some(comment.line_start()),
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
        comment_line_start: Some(comment.line_start()),
        comment_preview: Some(comment_preview(&comment.body)),
    }
}

fn diff_panel_model(state: &AppState) -> DiffPanel {
    let selected = state.selected_file_entry();
    let document_scroll = state
        .active_document
        .as_ref()
        .map_or(state.diff_scroll, |active| active.scroll().0);
    let document_cursor = state.active_document.as_ref().map_or(
        TextAnchor {
            line: state.diff_line_cursor,
            column: state.diff_col_cursor,
        },
        |active| TextAnchor {
            line: active.cursor().row.0,
            column: active.cursor().column.0,
        },
    );

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
        scroll: document_scroll,
        cursor: document_cursor,
    }
}

fn comments_panel_model(state: &AppState) -> CommentsPanel {
    let selected_path = state
        .selected_file_entry()
        .map(|entry| entry.change.path.as_str());
    let active_document_comment_id = state
        .active_document
        .as_ref()
        .and_then(|document| document.document().diff.current_comment_id());
    let current_comment_id = active_document_comment_id;
    let mut file_comments: Vec<&review_types::Comment> = state
        .comments
        .iter()
        .filter(|comment| selected_path.is_none_or(|path| comment.file_path() == path))
        .collect();
    file_comments.sort_by_key(|comment| {
        let (start, end) = logical_comment_line_range(comment)
            .unwrap_or((comment.line_start(), comment.line_end()));
        (start, end, comment.id)
    });
    let file_comment_ids: Vec<i64> = file_comments.iter().map(|comment| comment.id).collect();
    let detail_id = if state.pane_focus == PaneFocus::Comments {
        state.selected_comment_id
    } else {
        current_comment_id
    };
    let selected_comment_out_of_range = state.selected_comment_id.and_then(|id| {
        state
            .comments
            .iter()
            .find(|comment| Some(comment.file_path()) != selected_path && comment.id == id)
    });
    let comments = detail_id
        .and_then(|id| state.comments.iter().find(|comment| comment.id == id))
        .filter(|comment| {
            selected_path.is_none_or(|path| comment.file_path() == path)
                || Some(comment.id) == state.selected_comment_id
        })
        .or(selected_comment_out_of_range)
        .map(|comment| {
            let (line_start, line_end) = logical_comment_line_range(comment)
                .unwrap_or((comment.line_start(), comment.line_end()));
            let current = current_comment_id == Some(comment.id);
            let expanded =
                !comment.resolved || current || state.expanded_comment_ids.contains(&comment.id);
            vec![CommentItem {
                id: comment.id,
                file_path: comment.file_path().to_string(),
                line_start,
                line_end,
                body: comment.body.clone(),
                preview: comment_preview(&comment.body),
                resolved: comment.resolved,
                expanded,
                selected: Some(comment.id) == state.selected_comment_id,
                current,
                anchor_status: comment.anchor_status(),
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

fn logical_comment_line_range(comment: &review_types::Comment) -> Option<(i64, i64)> {
    let mut segments = comment
        .anchor()
        .segments
        .iter()
        .filter(|segment| segment.file_path == comment.file_path());
    let first = segments.next()?;
    let mut line_start = first.line_start;
    let mut line_end = first.line_end;
    for segment in segments {
        line_start = line_start.min(segment.line_start);
        line_end = line_end.max(segment.line_end);
    }
    Some((line_start, line_end))
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
            hash: Rc::from(line.hash.as_str()),
            author: Rc::from(line.author.as_str()),
            date: Rc::from(line.date.as_str()),
        }
    }
}

impl From<crate::git::BlameLine> for BlameLine {
    fn from(line: crate::git::BlameLine) -> Self {
        Self {
            hash: Rc::from(line.hash),
            author: Rc::from(line.author),
            date: Rc::from(line.date),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::app::document::DiffDocument;
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

    fn stored_comment(id: i64, file_path: &str, resolved: bool) -> review_types::Comment {
        review_types::Comment::new(review_types::CommentInit {
            id,
            merge_base: "abc123".to_string(),
            head_ref: "feature".to_string(),
            created_head_commit: "head-commit".to_string(),
            anchor: test_anchor(file_path, 2, 2, "anchor"),
            body: "comment".to_string(),
            resolved,
            created_at: "2026-06-28T00:00:00+10:00".to_string(),
            updated_at: "2026-06-28T00:00:00+10:00".to_string(),
            anchor_status: AnchorStatus::Anchored,
        })
        .expect("test comment anchor should be valid")
    }

    fn test_anchor(
        file_path: &str,
        line_start: i64,
        line_end: i64,
        anchor_text: &str,
    ) -> review_types::CommentAnchor {
        review_types::CommentAnchor {
            segments: vec![review_types::CommentAnchorSegment {
                side: review_types::CommentAnchorSide::Head,
                file_path: file_path.to_string(),
                line_start,
                line_end,
                char_start: None,
                char_end: None,
                anchor_text: anchor_text.to_string(),
                context_before: String::new(),
                context_after: String::new(),
                placement_status: review_types::AnchorPlacementStatus::Anchored,
                match_method: review_types::AnchorMatchMethod::ExactAtLine,
            }],
            aggregate_status: review_types::AnchorAggregateStatus::Anchored,
        }
    }

    fn compound_comment(
        id: i64,
        _file_path: &str,
        segments: Vec<review_types::CommentAnchorSegment>,
    ) -> review_types::Comment {
        review_types::Comment::new(review_types::CommentInit {
            id,
            merge_base: "abc123".to_string(),
            head_ref: "feature".to_string(),
            created_head_commit: "head-commit".to_string(),
            anchor: review_types::CommentAnchor {
                segments,
                aggregate_status: review_types::AnchorAggregateStatus::Anchored,
            },
            body: "compound".to_string(),
            resolved: false,
            created_at: "2026-06-28T00:00:00+10:00".to_string(),
            updated_at: "2026-06-28T00:00:00+10:00".to_string(),
            anchor_status: AnchorStatus::Anchored,
        })
        .expect("test comment anchor should be valid")
    }

    fn move_comment_head_range(
        comment: &mut review_types::Comment,
        line_start: i64,
        line_end: i64,
    ) {
        let file_path = comment.file_path().to_string();
        let anchor_text = comment.anchor_text().to_string();
        comment
            .replace_anchor(test_anchor(&file_path, line_start, line_end, &anchor_text))
            .expect("test comment anchor should be valid");
    }

    fn anchor_segment(
        side: review_types::CommentAnchorSide,
        file_path: &str,
        line_start: i64,
        line_end: i64,
        anchor_text: &str,
    ) -> review_types::CommentAnchorSegment {
        review_types::CommentAnchorSegment {
            side,
            file_path: file_path.to_string(),
            line_start,
            line_end,
            char_start: None,
            char_end: None,
            anchor_text: anchor_text.to_string(),
            context_before: String::new(),
            context_after: String::new(),
            placement_status: review_types::AnchorPlacementStatus::Anchored,
            match_method: review_types::AnchorMatchMethod::ExactAtLine,
        }
    }

    #[test]
    fn comments_panel_uses_logical_inline_comment_on_base_segment_row() {
        let hunk = review_types::DiffHunk {
            old_start: 130,
            old_lines: 1,
            new_start: 139,
            new_lines: 4,
            header: "@@ -130 +139,4 @@".to_string(),
            lines: vec![
                review_types::DiffLine {
                    kind: LineKind::Deletion,
                    content: ".position(|comment| comment.line_start > cursor_line)".to_string(),
                    old_lineno: Some(130),
                    new_lineno: None,
                },
                review_types::DiffLine {
                    kind: LineKind::Addition,
                    content: ".position(|comment| {".to_string(),
                    old_lineno: None,
                    new_lineno: Some(139),
                },
                review_types::DiffLine {
                    kind: LineKind::Addition,
                    content: "    visible_comment_line_range(state, comment)".to_string(),
                    old_lineno: None,
                    new_lineno: Some(140),
                },
                review_types::DiffLine {
                    kind: LineKind::Addition,
                    content: "        .is_some_and(|(line_start, _)| line_start > cursor_line)"
                        .to_string(),
                    old_lineno: None,
                    new_lineno: Some(141),
                },
                review_types::DiffLine {
                    kind: LineKind::Addition,
                    content: "})".to_string(),
                    old_lineno: None,
                    new_lineno: Some(142),
                },
            ],
        };
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![file(
                "src/lib.rs",
                review_types::ReviewStatus::Unreviewed,
                vec![hunk],
            )],
        );
        app.state.content_mode = ContentMode::Diff;
        app.state.render_variant = RenderVariant::Inline;
        app.state.show_comments_panel = true;
        app.state.head_content = Some(
            (1..=143)
                .map(|n| format!("head {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        app.state.comments = vec![compound_comment(
            1,
            "src/lib.rs",
            vec![
                anchor_segment(
                    review_types::CommentAnchorSide::Base,
                    "src/lib.rs",
                    130,
                    130,
                    "base",
                ),
                anchor_segment(
                    review_types::CommentAnchorSide::Head,
                    "src/lib.rs",
                    139,
                    142,
                    "head",
                ),
            ],
        )];
        app.state.rebuild_active_document();
        let deletion_row = app
            .state
            .active_document
            .as_ref()
            .and_then(|document| {
                document
                    .document()
                    .diff
                    .row_for_source_line(review_types::CommentAnchorSide::Base, 130)
            })
            .expect("deletion row should render")
            .0;
        app.state.diff_line_cursor = deletion_row;
        app.state.ensure_active_document();

        let model = app.model();

        assert_eq!(model.comments_panel.comments.len(), 1);
        assert_eq!(model.comments_panel.comments[0].id, 1);
        assert!(model.comments_panel.comments[0].current);
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
        move_comment_head_range(&mut first, 12, 14);
        first.body = "First line of the comment\nsecond line".to_string();
        let mut second = stored_comment(5, "a.rs", false);
        move_comment_head_range(&mut second, 4, 4);
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
        move_comment_head_range(&mut comment, 22, 22);
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
    fn comments_panel_uses_selected_comment_as_same_start_tie_breaker() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![file(
                "a.rs",
                review_types::ReviewStatus::Unreviewed,
                Vec::new(),
            )],
        );
        app.state.content_mode = ContentMode::FullFile;
        app.state.render_variant = RenderVariant::HeadVersion;
        app.state.head_content = Some(
            (1..=30)
                .map(|n| format!("head {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        app.state.diff_line_cursor = 9;
        app.state.pane_focus = PaneFocus::Diff;
        app.state.show_comments_panel = true;
        let mut first = stored_comment(1, "a.rs", false);
        move_comment_head_range(&mut first, 10, 20);
        first.body = "Selected same-start comment".to_string();
        let mut second = stored_comment(2, "a.rs", false);
        move_comment_head_range(&mut second, 10, 25);
        second.body = "Other same-start comment".to_string();
        app.state.comments = vec![first, second];
        app.state.selected_comment_id = Some(1);
        app.state.ensure_active_document();

        let model = app.model();

        assert_eq!(model.comments_panel.comments.len(), 1);
        assert_eq!(model.comments_panel.comments[0].id, 1);
        assert_eq!(
            model.comments_panel.comments[0].body,
            "Selected same-start comment"
        );
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
        app.state.visual_selection = Some(crate::app::VisualSelection {
            mode: VisualSelectionMode::Text,
            start: DocumentPosition {
                row: RowIndex(1),
                column: ColumnIndex(2),
            },
            end: DocumentPosition {
                row: RowIndex(3),
                column: ColumnIndex(4),
            },
        });
        app.state.ensure_active_document();

        let model = app.model();

        assert_eq!(model.diff.file_id, Some("src/main.rs".to_string()));
        assert_eq!(model.diff.selected_file_index, Some(0));
        assert_eq!(model.diff.scroll, 0);
        assert_eq!(model.diff.cursor, TextAnchor { line: 3, column: 7 });
        assert_eq!(
            model
                .active_document
                .as_ref()
                .map(|document| document.document().key.file_id.as_str()),
            Some("src/main.rs")
        );
        assert_eq!(
            model.active_document.as_ref().map(|document| document
                .document()
                .key
                .diff_hash
                .as_str()),
            Some("hash-src/main.rs")
        );
        let active_document = model.active_document.as_ref().expect("active document");
        assert_eq!(
            active_document.document().diff.all_search_matches(),
            vec![crate::app::document::SearchMatch {
                row: RowIndex(1),
                columns: crate::app::document::ColumnSpan {
                    start: ColumnIndex(4),
                    end: ColumnIndex(7),
                },
            }]
        );
        let DiffDocument::Unified(document) = &active_document.document().diff else {
            panic!("expected unified active document");
        };
        assert_eq!(
            document.overlays().selection,
            Some(DocumentVisibleSelection::Text {
                start: DocumentPosition {
                    row: RowIndex(1),
                    column: ColumnIndex(2)
                },
                end: DocumentPosition {
                    row: RowIndex(3),
                    column: ColumnIndex(4)
                }
            })
        );
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
