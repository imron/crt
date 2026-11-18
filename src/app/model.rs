//! UI-agnostic application model.
//!
//! `AppModel` describes the conceptual review UI that any frontend can render.
//! It intentionally excludes terminal cells, pixels, ratatui layout, and other
//! backend-specific geometry.

use crate::app::{AppState, CommentAnchorCapture, VisualSelectionMode};
use crate::config::DiffAlgorithm;
use crate::core::TextAnchor;
use crate::review_types::{
    self, ChangeKind, ConnectionContext, ContentMode, LineKind, PaneFocus, RenderVariant,
};

#[derive(Debug, Clone)]
pub struct AppModel {
    pub revision: u64,
    pub context: ConnectionContext,
    pub layout: AppLayout,
    pub file_list: FileList,
    pub diff: DiffPanel,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileListSection {
    pub kind: FileListSectionKind,
    pub rows: Vec<FileListRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileListRow {
    pub file_index: usize,
    pub file_id: String,
    pub path: String,
    pub old_path: Option<String>,
    pub change_kind: ChangeKind,
    pub review_status: ReviewStatus,
    pub selected: bool,
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
    entry: &crate::review_types::FileEntry,
) -> FileListRow {
    FileListRow {
        file_index: index,
        file_id: entry.change.path.clone(),
        path: entry.change.path.clone(),
        old_path: entry.change.old_path.clone(),
        change_kind: entry.change.kind,
        review_status: ReviewStatus::from(&entry.status),
        selected: index == selected_file,
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
    }
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
    use crate::review_types::{self, ChangeKind, DiffContent, FileChange, FileEntry, LineKind};

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
            ReviewStatus::Changed { .. }
        ));
        assert_eq!(model.file_list.sections[1].rows[0].path, "c.rs");
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
