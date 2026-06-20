//! UI-agnostic application model.
//!
//! `AppModel` describes the conceptual review UI that any frontend can render.
//! It intentionally excludes terminal cells, pixels, ratatui layout, and other
//! backend-specific geometry.

use crate::app::AppState;
use crate::config::DiffAlgorithm;
use crate::core::{PromptId, PromptKind, TextAnchor};
use crate::model::{
    ChangeKind, ConnectionContext, ContentMode, LineKind, PaneFocus, RenderVariant, ReviewStatus,
};

#[derive(Debug, Clone)]
pub struct AppModel {
    pub context: ConnectionContext,
    pub file_list: FileListModel,
    pub diff: DiffPanelModel,
    pub focus: PaneFocus,
    pub prompt: Option<PromptModel>,
    pub overlays: Vec<OverlayModel>,
    pub status: Option<StatusModel>,
    pub selection: Option<SemanticSelectionModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileListModel {
    pub sections: Vec<FileListSectionModel>,
    pub selected_file_id: Option<String>,
    pub selected_file_index: Option<usize>,
    pub scroll: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileListSectionKind {
    Unreviewed,
    Reviewed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileListSectionModel {
    pub kind: FileListSectionKind,
    pub rows: Vec<FileListRowModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileListRowModel {
    pub file_index: usize,
    pub file_id: String,
    pub path: String,
    pub old_path: Option<String>,
    pub change_kind: ChangeKind,
    pub review_status: ReviewStatusModel,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewStatusModel {
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
pub struct DiffPanelModel {
    pub selected_file_index: Option<usize>,
    pub file_id: Option<String>,
    pub path: Option<String>,
    pub review_status: Option<ReviewStatusModel>,
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
    pub hunks: Vec<DiffHunkModel>,
    pub head_content: Option<String>,
    pub base_content: Option<String>,
    pub head_blame: Vec<BlameLineModel>,
    pub base_blame: Vec<BlameLineModel>,
    pub scroll: usize,
    pub cursor: TextAnchor,
    pub search_query: Option<String>,
    pub search_highlights: Vec<TextRangeModel>,
    pub current_search_highlight: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunkModel {
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLineModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLineModel {
    pub kind: LineKind,
    pub content: String,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLineModel {
    pub hash: String,
    pub author: String,
    pub date: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRangeModel {
    pub line: usize,
    pub column_start: usize,
    pub column_end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptModel {
    pub id: PromptId,
    pub kind: PromptKind,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayModel {
    pub kind: OverlayKind,
    pub title: String,
    pub items: Vec<OverlayItemModel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayKind {
    SearchResults,
    DefinitionResults,
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayItemModel {
    pub id: String,
    pub label: String,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusModel {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticSelectionModel {
    pub pane: PaneFocus,
    pub start: TextAnchor,
    pub end: TextAnchor,
}

impl AppModel {
    pub(crate) fn from_state(state: &AppState) -> Self {
        Self {
            context: state.context.clone(),
            file_list: file_list_model(state),
            diff: diff_panel_model(state),
            focus: state.pane_focus,
            prompt: None,
            overlays: Vec::new(),
            status: None,
            selection: None,
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

fn file_list_model(state: &AppState) -> FileListModel {
    let unreviewed_count = state.unreviewed_count();
    let selected_file_id = state
        .selected_file_entry()
        .map(|entry| entry.change.path.clone());

    let unreviewed_rows = state
        .files
        .iter()
        .take(unreviewed_count)
        .enumerate()
        .map(|(index, entry)| file_row_model(index, state.selected_file, entry))
        .collect();
    let reviewed_rows = state
        .files
        .iter()
        .enumerate()
        .skip(unreviewed_count)
        .map(|(index, entry)| file_row_model(index, state.selected_file, entry))
        .collect();

    FileListModel {
        sections: vec![
            FileListSectionModel {
                kind: FileListSectionKind::Unreviewed,
                rows: unreviewed_rows,
            },
            FileListSectionModel {
                kind: FileListSectionKind::Reviewed,
                rows: reviewed_rows,
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
    entry: &crate::model::FileEntry,
) -> FileListRowModel {
    FileListRowModel {
        file_index: index,
        file_id: entry.change.path.clone(),
        path: entry.change.path.clone(),
        old_path: entry.change.old_path.clone(),
        change_kind: entry.change.kind,
        review_status: ReviewStatusModel::from(&entry.status),
        selected: index == selected_file,
    }
}

fn diff_panel_model(state: &AppState) -> DiffPanelModel {
    let selected = state.selected_file_entry();
    let hunks = selected
        .map(|entry| {
            entry
                .diff
                .hunks
                .iter()
                .map(|hunk| DiffHunkModel {
                    header: hunk.header.clone(),
                    old_start: hunk.old_start,
                    old_lines: hunk.old_lines,
                    new_start: hunk.new_start,
                    new_lines: hunk.new_lines,
                    lines: hunk
                        .lines
                        .iter()
                        .map(|line| DiffLineModel {
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

    DiffPanelModel {
        selected_file_index: selected.map(|_| state.selected_file),
        file_id: selected.map(|entry| entry.change.path.clone()),
        path: selected.map(|entry| entry.change.path.clone()),
        review_status: selected.map(|entry| ReviewStatusModel::from(&entry.status)),
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
        head_blame: state.head_blame.iter().map(BlameLineModel::from).collect(),
        base_blame: state.base_blame.iter().map(BlameLineModel::from).collect(),
        scroll: state.diff_scroll,
        cursor: TextAnchor {
            line: state.diff_line_cursor,
            column: state.diff_col_cursor,
        },
        search_query: state.diff_search_query.clone(),
        search_highlights: state
            .diff_search_matches
            .iter()
            .map(|(line, start, end)| TextRangeModel {
                line: *line,
                column_start: *start,
                column_end: *end,
            })
            .collect(),
        current_search_highlight: state
            .diff_search_matches
            .get(state.diff_search_current)
            .map(|_| state.diff_search_current),
    }
}

impl From<&ReviewStatus> for ReviewStatusModel {
    fn from(status: &ReviewStatus) -> Self {
        match status {
            ReviewStatus::Unreviewed => Self::Unreviewed,
            ReviewStatus::Reviewed {
                at,
                reviewed_commit,
            } => Self::Reviewed {
                at: at.clone(),
                reviewed_commit: reviewed_commit.clone(),
            },
            ReviewStatus::Changed {
                at,
                reviewed_commit,
            } => Self::Changed {
                at: at.clone(),
                reviewed_commit: reviewed_commit.clone(),
            },
        }
    }
}

impl From<&crate::git::BlameLine> for BlameLineModel {
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
    use crate::model::{
        ChangeKind, DiffContent, DiffHunk, DiffLine, FileChange, FileEntry, LineKind,
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

    fn file(path: &str, status: ReviewStatus, hunks: Vec<DiffHunk>) -> FileEntry {
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

    fn hunk() -> DiffHunk {
        DiffHunk {
            old_start: 1,
            old_lines: 2,
            new_start: 1,
            new_lines: 2,
            header: "@@ -1,2 +1,2 @@".to_string(),
            lines: vec![
                DiffLine {
                    kind: LineKind::Context,
                    content: "fn main() {".to_string(),
                    old_lineno: Some(1),
                    new_lineno: Some(1),
                },
                DiffLine {
                    kind: LineKind::Addition,
                    content: "    run();".to_string(),
                    old_lineno: None,
                    new_lineno: Some(2),
                },
            ],
        }
    }

    fn reviewed() -> ReviewStatus {
        ReviewStatus::Reviewed {
            at: "2026-01-01T00:00:00Z".to_string(),
            reviewed_commit: Some("abc".to_string()),
        }
    }

    fn changed() -> ReviewStatus {
        ReviewStatus::Changed {
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
                file("a.rs", ReviewStatus::Unreviewed, Vec::new()),
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
        assert_eq!(model.file_list.sections.len(), 2);
        assert_eq!(
            model.file_list.sections[0].kind,
            FileListSectionKind::Unreviewed
        );
        assert_eq!(
            model.file_list.sections[1].kind,
            FileListSectionKind::Reviewed
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
        assert_eq!(model.file_list.sections[0].rows[1].file_index, 1);
        assert!(matches!(
            model.file_list.sections[0].rows[1].review_status,
            ReviewStatusModel::Changed { .. }
        ));
        assert_eq!(model.file_list.sections[1].rows[0].path, "c.rs");
    }

    #[test]
    fn model_projects_diff_content_cursor_and_search_highlights() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![file("src/main.rs", ReviewStatus::Unreviewed, vec![hunk()])],
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
                TextRangeModel {
                    line: 3,
                    column_start: 10,
                    column_end: 13,
                },
                TextRangeModel {
                    line: 8,
                    column_start: 2,
                    column_end: 5,
                },
            ]
        );
        assert_eq!(model.diff.current_search_highlight, Some(1));
    }

    #[test]
    fn model_has_semantic_slots_for_ui_concepts() {
        let app = App::new(
            Config::default(),
            test_context(),
            vec![file("src/lib.rs", ReviewStatus::Unreviewed, Vec::new())],
        );

        let model = app.model();

        assert_eq!(model.focus, PaneFocus::FileList);
        assert!(model.prompt.is_none());
        assert!(model.overlays.is_empty());
        assert!(model.status.is_none());
        assert!(model.selection.is_none());
    }
}
