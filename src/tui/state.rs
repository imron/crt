//! TUI-owned presentation state.

use std::time::{Duration, Instant};

use super::render::diff_view::DiffCache;
use crate::app::document::DocumentKey;
use crate::app::model::AppModel;
use crate::app::{AppOutput, AppState, AppViewport, FileListSectionFocus, StatusUpdate};
use crate::core::{AppTarget, ConnectionState, PaneId, PointerSemanticHit, PromptId, TextAnchor};
use crate::review_types::PaneFocus;
use ratatui::layout::Rect;

/// Status message timeout.
pub const STATUS_MSG_TIMEOUT: Duration = Duration::from_secs(3);

/// Current terminal input mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputMode {
    /// Normal mode — keys are dispatched as commands.
    Normal,
    /// Command mode — `:` prompt is active, collecting user input.
    Command,
    /// Diff search mode — `/` prompt is active, collecting search query.
    DiffSearch,
    /// Comment composer mode — multi-line review comment input is active.
    Comment,
}

/// An in-progress or completed text selection via mouse drag.
#[derive(Debug, Clone)]
pub struct MouseSelection {
    /// Which pane the selection is confined to.
    pub pane: PaneFocus,
    /// Start position in pane semantic text coordinates.
    pub start: TextAnchor,
    /// Current end position in pane semantic text coordinates.
    pub end: TextAnchor,
    /// Set when selection was created by double-click word selection.
    /// The Up event should not re-extract text (it was already copied).
    pub word_selected: bool,
}

impl MouseSelection {
    /// Normalize so start is before end (handles upward/leftward drags).
    pub fn normalized(&self) -> (TextAnchor, TextAnchor) {
        if self.start.line < self.end.line
            || (self.start.line == self.end.line && self.start.column <= self.end.column)
        {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        }
    }
}

/// Last semantic content click, used for double-click detection.
#[derive(Debug, Clone)]
pub struct LastPointerClick {
    pub when: Instant,
    pub pane: PaneFocus,
    pub anchor: TextAnchor,
}

pub struct TuiState {
    /// Whether inline comments are visible in the diff pane.
    pub show_comments: bool,
    /// TUI-local document renderer toggle.
    pub use_document_diff_view: bool,
    pub diff_cache: Option<DiffCache>,
    /// Display row indices where each hunk starts in the rendered TUI diff.
    pub hunk_start_rows: Vec<usize>,
    /// Display row indices where each hunk ends in the rendered TUI diff.
    pub hunk_end_rows: Vec<usize>,
    /// Display row of the first actual change (+/-) in each rendered hunk.
    pub hunk_first_change_rows: Vec<usize>,
    /// Number of columns occupied by line-number gutters in the TUI diff pane.
    pub diff_gutter_cols: usize,
    /// Column where semantic diff text starts in the rendered TUI diff pane.
    pub diff_content_start_col: usize,
    /// Total rendered line count in the diff pane.
    pub diff_content_height: usize,
    /// Visible rendered line count in the diff pane.
    pub diff_view_height: usize,
    /// Plain text of rendered diff lines, used for search and clipboard.
    pub diff_rendered_text: Vec<String>,
    /// Document key for plain text produced by the document renderer.
    pub document_diff_rendered_text_key: Option<DocumentKey>,
    /// Plain text of rendered file-list lines, used for clipboard.
    pub file_list_rendered_text: Vec<String>,
    /// Mapping from rendered file-list rows to file indices.
    pub file_list_row_to_file: Vec<Option<usize>>,
    /// Mapping from rendered file-list rows to unresolved comment ids.
    pub file_list_row_to_comment: Vec<Option<i64>>,
    /// Screen area of the file list pane.
    pub file_list_area: Rect,
    /// Screen area of the diff pane.
    pub diff_area: Rect,
    /// Screen area of the comments panel.
    pub comments_area: Rect,
    /// Active mouse text selection, if any.
    pub mouse_selection: Option<MouseSelection>,
    /// Mouse down anchor used to start a drag selection only after the pointer
    /// actually moves.
    pub mouse_down_anchor: Option<(PaneFocus, TextAnchor)>,
    /// Last semantic content click, for double-click detection.
    pub last_click: Option<LastPointerClick>,
    /// True while the user is dragging the file list / diff pane border.
    pub dragging_border: bool,
    /// Whether the help overlay is visible.
    pub show_help: bool,
    /// Transient status bar message (e.g. "Press Ctrl-C again to quit").
    pub status_message: Option<(String, Instant)>,
    /// Set to true to suspend the process (Ctrl-Z).
    pub should_suspend: bool,
    /// Set to true to exit the event loop.
    pub should_quit: bool,
    /// TUI scroll offset for the search results overlay.
    pub search_results_scroll: usize,
    /// Current input mode (Normal vs Command).
    pub input_mode: InputMode,
    /// Command-mode input buffer (the text after `:`).
    pub command_input: String,
    /// Cursor position within `command_input`.
    pub command_cursor: usize,
    /// In-progress diff search input (while typing in `/` prompt).
    pub diff_search_input: String,
    /// Cursor position within `diff_search_input`.
    pub diff_search_cursor: usize,
    /// In-progress review comment body.
    pub comment_input: String,
    /// Title shown on the comment composer.
    pub comment_title: String,
    /// Cursor position within `comment_input`.
    pub comment_cursor: usize,
    /// First visible line in the comment composer.
    pub comment_scroll: usize,
    /// Active core prompt requested by the interaction engine, if any.
    pub active_core_prompt: Option<PromptId>,
}

impl Default for InputMode {
    fn default() -> Self {
        Self::Normal
    }
}

impl Default for TuiState {
    fn default() -> Self {
        Self {
            show_comments: false,
            use_document_diff_view: false,
            diff_cache: None,
            hunk_start_rows: Vec::new(),
            hunk_end_rows: Vec::new(),
            hunk_first_change_rows: Vec::new(),
            diff_gutter_cols: 0,
            diff_content_start_col: 0,
            diff_content_height: 0,
            diff_view_height: 0,
            diff_rendered_text: Vec::new(),
            document_diff_rendered_text_key: None,
            file_list_rendered_text: Vec::new(),
            file_list_row_to_file: Vec::new(),
            file_list_row_to_comment: Vec::new(),
            file_list_area: Rect::default(),
            diff_area: Rect::default(),
            comments_area: Rect::default(),
            mouse_selection: None,
            mouse_down_anchor: None,
            last_click: None,
            dragging_border: false,
            show_help: false,
            status_message: None,
            should_suspend: false,
            should_quit: false,
            search_results_scroll: 0,
            input_mode: InputMode::default(),
            command_input: String::new(),
            command_cursor: 0,
            diff_search_input: String::new(),
            diff_search_cursor: 0,
            comment_input: String::new(),
            comment_title: "Comment".to_string(),
            comment_cursor: 0,
            comment_scroll: 0,
            active_core_prompt: None,
        }
    }
}

impl TuiState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Maximum diff scroll offset for the last rendered line.
    pub fn max_diff_scroll(&self) -> usize {
        self.diff_content_height.saturating_sub(1)
    }

    pub fn current_hunk_index_at(&self, cursor_line: usize) -> Option<usize> {
        self.hunk_start_rows
            .iter()
            .enumerate()
            .rev()
            .find(|(_, start)| **start <= cursor_line)
            .map(|(idx, _)| idx)
    }

    pub fn clamp_diff_scroll(&self, state: &mut AppState) {
        state.diff_scroll = state.diff_scroll.min(self.max_diff_scroll());
    }

    pub fn clamp_cursor_and_scroll(&self, state: &mut AppState) {
        let max = self.max_diff_scroll();
        state.diff_line_cursor = state.diff_line_cursor.min(max);
        if state.diff_line_cursor < state.diff_scroll {
            state.diff_scroll = state.diff_line_cursor;
        }
        if self.diff_view_height > 0
            && state.diff_line_cursor >= state.diff_scroll + self.diff_view_height
        {
            state.diff_scroll = state
                .diff_line_cursor
                .saturating_sub(self.diff_view_height - 1);
        }
        self.clamp_diff_scroll(state);
    }

    pub fn clamp_file_list_scroll(&self, state: &mut AppState) {
        let cursor_row =
            if state.file_list_section_focus == FileListSectionFocus::UnresolvedComments {
                state.selected_comment_id.and_then(|comment_id| {
                    self.file_list_row_to_comment
                        .iter()
                        .position(|row_comment_id| *row_comment_id == Some(comment_id))
                })
            } else {
                None
            }
            .or_else(|| {
                self.file_list_row_to_file
                    .iter()
                    .position(|file_idx| *file_idx == Some(state.selected_file))
            });
        let Some(cursor_row) = cursor_row else {
            return;
        };
        let inner_height = self.file_list_area.height.saturating_sub(2) as usize;
        if cursor_row < state.file_list_scroll {
            state.file_list_scroll = cursor_row;
        } else if cursor_row >= state.file_list_scroll + inner_height {
            state.file_list_scroll = cursor_row.saturating_sub(inner_height) + 1;
        }
    }

    pub fn comments_panel_height(area_height: u16) -> u16 {
        (area_height / 3).clamp(6, 12)
    }

    pub fn project_comments_panel_open(&mut self) {
        if self.comments_area != Rect::default() || self.diff_area.height < 8 {
            return;
        }

        let panel_height = Self::comments_panel_height(self.diff_area.height);
        let diff_height = self.diff_area.height.saturating_sub(panel_height);
        self.comments_area = Rect::new(
            self.diff_area.x,
            self.diff_area.y.saturating_add(diff_height),
            self.diff_area.width,
            panel_height,
        );
        self.diff_area.height = diff_height;
        self.diff_view_height = diff_height.saturating_sub(2) as usize;
    }

    pub fn diff_content_start_col(&self) -> usize {
        if self.diff_content_start_col > 0 {
            self.diff_content_start_col
        } else if self.diff_gutter_cols > 0 {
            self.diff_gutter_cols + 3
        } else {
            0
        }
    }

    pub fn pane_at(&self, col: u16, row: u16) -> Option<PaneFocus> {
        if self.file_list_area.contains((col, row).into()) {
            Some(PaneFocus::FileList)
        } else if self.comments_area.contains((col, row).into()) {
            Some(PaneFocus::Comments)
        } else if self.diff_area.contains((col, row).into()) {
            Some(PaneFocus::Diff)
        } else {
            None
        }
    }

    pub fn area_for_pane(&self, pane: PaneFocus) -> Rect {
        match pane {
            PaneFocus::FileList => self.file_list_area,
            PaneFocus::Diff => self.diff_area,
            PaneFocus::Comments => self.comments_area,
        }
    }

    pub fn pointer_semantic_hit(
        &self,
        model: &AppModel,
        pane: PaneFocus,
        column: u16,
        row: u16,
    ) -> Option<PointerSemanticHit> {
        let pane_id = match pane {
            PaneFocus::FileList => PaneId::FileList,
            PaneFocus::Diff => PaneId::Diff,
            PaneFocus::Comments => PaneId::Comments,
        };
        let text_anchor = self.pointer_text_anchor_for_pane(model, pane, column, row, false);
        let target = self.pointer_target(pane, text_anchor);
        Some(PointerSemanticHit {
            pane_id,
            target,
            region_id: self.pointer_region_id(model, pane, text_anchor),
            text_anchor,
        })
    }

    pub fn pointer_text_anchor_for_pane(
        &self,
        model: &AppModel,
        pane: PaneFocus,
        column: u16,
        row: u16,
        clamp: bool,
    ) -> Option<TextAnchor> {
        let area = self.area_for_pane(pane);
        let inner_top = area.y.saturating_add(1);
        let inner_left = area.x.saturating_add(1);
        let inner_right = area.right().saturating_sub(1);
        let inner_bottom = area.bottom().saturating_sub(1);

        if inner_top >= inner_bottom {
            return None;
        }

        let row = if clamp {
            row.clamp(inner_top, inner_bottom.saturating_sub(1))
        } else if row >= inner_top && row < inner_bottom {
            row
        } else {
            return None;
        };

        let column = if clamp && inner_left < inner_right {
            column.clamp(inner_left, inner_right.saturating_sub(1))
        } else {
            column
        };

        match pane {
            PaneFocus::FileList => Some(TextAnchor {
                line: model
                    .file_list
                    .scroll
                    .saturating_add((row - inner_top) as usize),
                column: column.saturating_sub(inner_left) as usize,
            }),
            PaneFocus::Diff => Some(TextAnchor {
                line: model.diff.scroll.saturating_add((row - inner_top) as usize),
                column: (column as usize)
                    .saturating_sub(inner_left as usize + self.diff_content_start_col()),
            }),
            PaneFocus::Comments => Some(TextAnchor {
                line: (row - inner_top) as usize,
                column: column.saturating_sub(inner_left) as usize,
            }),
        }
    }

    fn pointer_target(&self, pane: PaneFocus, text_anchor: Option<TextAnchor>) -> AppTarget {
        match (pane, text_anchor) {
            (PaneFocus::FileList, Some(anchor)) => self
                .file_list_row_to_comment
                .get(anchor.line)
                .and_then(|comment_id| *comment_id)
                .map(|id| AppTarget::Comment { id })
                .or_else(|| {
                    self.file_list_row_to_file
                        .get(anchor.line)
                        .and_then(|file_idx| *file_idx)
                        .map(|index| AppTarget::File { index })
                })
                .unwrap_or(AppTarget::Pane {
                    pane_id: PaneId::FileList,
                }),
            (PaneFocus::Diff, Some(anchor)) => AppTarget::DiffText { anchor },
            (PaneFocus::Comments, _) => AppTarget::Pane {
                pane_id: PaneId::Comments,
            },
            (PaneFocus::FileList, None) => AppTarget::Pane {
                pane_id: PaneId::FileList,
            },
            (PaneFocus::Diff, None) => AppTarget::Pane {
                pane_id: PaneId::Diff,
            },
        }
    }

    fn pointer_region_id(
        &self,
        model: &AppModel,
        pane: PaneFocus,
        text_anchor: Option<TextAnchor>,
    ) -> Option<String> {
        let anchor = text_anchor?;
        match pane {
            PaneFocus::FileList => match self.file_list_row_to_comment.get(anchor.line) {
                Some(Some(comment_id)) => Some(format!("comment:{comment_id}")),
                _ => match self.file_list_row_to_file.get(anchor.line) {
                    Some(Some(file_idx)) => {
                        model_file_path(model, *file_idx).map(|path| format!("file:{path}"))
                    }
                    _ => Some(format!("file-list-row:{}", anchor.line)),
                },
            },
            PaneFocus::Diff => Some(format!("diff-line:{}", anchor.line)),
            PaneFocus::Comments => Some(format!("comment-row:{}", anchor.line)),
        }
    }

    pub fn set_status_message(&mut self, message: impl Into<String>) {
        self.status_message = Some((message.into(), Instant::now()));
    }

    pub fn clear_status_message(&mut self) {
        self.status_message = None;
    }

    pub fn apply_status_update(&mut self, update: Option<StatusUpdate>) {
        match update {
            Some(StatusUpdate::Set(message)) => {
                self.set_status_message(message);
            }
            Some(StatusUpdate::Clear) => {
                self.clear_status_message();
            }
            Some(StatusUpdate::ConnectionState(state)) => {
                self.apply_connection_state(state);
            }
            None => {}
        }
    }

    fn apply_connection_state(&mut self, state: ConnectionState) {
        match state {
            ConnectionState::Connected => {}
            ConnectionState::Reconnecting => {
                self.clear_prompt();
                self.set_status_message("Reconnecting...");
            }
            ConnectionState::Reconnected => {
                self.clear_prompt();
                self.set_status_message("Reconnected");
            }
            ConnectionState::Disconnected => {
                self.clear_prompt();
                self.set_status_message("Disconnected");
            }
        }
    }

    pub fn apply_app_output(&mut self, output: AppOutput) {
        self.apply_status_update(output.status);
        self.should_suspend |= output.should_suspend;
        self.should_quit |= output.should_quit;
        if let Some(show_comments) = output.show_comments {
            self.show_comments = show_comments;
        }
    }

    #[cfg(test)]
    fn has_status_message(&self) -> bool {
        self.status_message.is_some()
    }

    pub fn expire_status_message(&mut self, timeout: Duration) -> bool {
        if self
            .status_message
            .as_ref()
            .is_some_and(|(_, when)| when.elapsed() > timeout)
        {
            self.clear_status_message();
            return true;
        }
        false
    }

    pub fn quit_confirmation_active(&self, timeout: Duration) -> bool {
        self.status_message
            .as_ref()
            .is_some_and(|(message, when)| message.contains("Ctrl-C") && when.elapsed() < timeout)
    }

    pub fn open_command_prompt(&mut self, id: PromptId, initial_value: String) {
        self.active_core_prompt = Some(id);
        self.input_mode = InputMode::Command;
        self.command_input = initial_value;
        self.command_cursor = self.command_input.len();
    }

    pub fn open_diff_search_prompt(&mut self, id: PromptId, initial_value: String) {
        self.active_core_prompt = Some(id);
        self.input_mode = InputMode::DiffSearch;
        self.diff_search_input = initial_value;
        self.diff_search_cursor = self.diff_search_input.len();
    }

    #[cfg(test)]
    pub fn open_comment_prompt(&mut self, id: PromptId, initial_value: String) {
        self.open_comment_prompt_with_title(id, "Comment".to_string(), initial_value);
    }

    pub fn open_comment_prompt_with_title(
        &mut self,
        id: PromptId,
        title: String,
        initial_value: String,
    ) {
        self.active_core_prompt = Some(id);
        self.input_mode = InputMode::Comment;
        self.comment_title = title;
        self.comment_input = initial_value;
        self.comment_cursor = self.comment_input.len();
        self.comment_scroll = 0;
    }

    pub fn clear_prompt(&mut self) {
        self.active_core_prompt = None;
        self.input_mode = InputMode::Normal;
        self.command_input.clear();
        self.command_cursor = 0;
        self.diff_search_input.clear();
        self.diff_search_cursor = 0;
        self.comment_input.clear();
        self.comment_title = "Comment".to_string();
        self.comment_cursor = 0;
        self.comment_scroll = 0;
    }
}

impl AppViewport for TuiState {
    fn hunk_start_rows(&self) -> &[usize] {
        &self.hunk_start_rows
    }

    fn hunk_end_rows(&self) -> &[usize] {
        &self.hunk_end_rows
    }

    fn hunk_first_change_rows(&self) -> &[usize] {
        &self.hunk_first_change_rows
    }

    fn diff_gutter_cols(&self) -> usize {
        self.diff_gutter_cols
    }

    fn diff_content_start_col(&self) -> usize {
        TuiState::diff_content_start_col(self)
    }

    fn diff_content_height(&self) -> usize {
        self.diff_content_height
    }

    fn diff_view_height(&self) -> usize {
        self.diff_view_height
    }

    fn diff_rendered_text(&self) -> &[String] {
        &self.diff_rendered_text
    }
}

fn model_file_path(model: &AppModel, file_index: usize) -> Option<&str> {
    model
        .file_list
        .sections
        .iter()
        .flat_map(|section| section.rows.iter())
        .find(|row| row.file_index == Some(file_index))
        .map(|row| row.path.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_prompt_tracks_buffer_and_cursor_locally() {
        let mut state = TuiState::default();

        state.open_command_prompt(PromptId(7), "gd symbol".to_string());

        assert_eq!(state.active_core_prompt, Some(PromptId(7)));
        assert_eq!(state.input_mode, InputMode::Command);
        assert_eq!(state.command_input, "gd symbol");
        assert_eq!(state.command_cursor, "gd symbol".len());
    }

    #[test]
    fn tui_state_keeps_terminal_presentation_state() {
        let state = TuiState::new();

        assert!(!state.show_comments);
        assert_eq!(state.file_list_area, Rect::default());
        assert_eq!(state.diff_area, Rect::default());
    }

    #[test]
    fn clear_prompt_resets_all_prompt_buffers() {
        let mut state = TuiState::default();
        state.open_comment_prompt_with_title(
            PromptId(8),
            "Edit Comment".to_string(),
            "needle".to_string(),
        );

        state.clear_prompt();

        assert_eq!(state.active_core_prompt, None);
        assert_eq!(state.input_mode, InputMode::Normal);
        assert!(state.command_input.is_empty());
        assert_eq!(state.command_cursor, 0);
        assert!(state.diff_search_input.is_empty());
        assert_eq!(state.diff_search_cursor, 0);
        assert!(state.comment_input.is_empty());
        assert_eq!(state.comment_title, "Comment");
        assert_eq!(state.comment_cursor, 0);
    }

    #[test]
    fn status_updates_are_tui_owned() {
        let mut state = TuiState::default();

        state.apply_status_update(Some(StatusUpdate::Set(
            "Press Ctrl-C again to quit".to_string(),
        )));

        assert!(state.has_status_message());
        assert!(state.quit_confirmation_active(Duration::from_secs(3)));

        state.apply_status_update(Some(StatusUpdate::Clear));

        assert!(!state.has_status_message());
    }

    #[test]
    fn connection_state_updates_clear_active_prompt() {
        let mut state = TuiState::default();
        state.open_command_prompt(PromptId(2), "search old".to_string());

        state.apply_status_update(Some(StatusUpdate::ConnectionState(
            ConnectionState::Reconnecting,
        )));

        assert_eq!(state.active_core_prompt, None);
        assert_eq!(state.input_mode, InputMode::Normal);
        assert!(state.command_input.is_empty());
        assert!(state.has_status_message());
    }

    #[test]
    fn app_outputs_set_tui_runtime_intents() {
        let mut state = TuiState::default();
        let mut output = AppOutput::default();
        output.handled = true;
        output.status = Some(StatusUpdate::Set("Searching...".to_string()));
        output.should_suspend = true;
        output.should_quit = true;
        output.show_comments = Some(true);

        state.apply_app_output(output);

        assert!(state.has_status_message());
        assert!(state.should_suspend);
        assert!(state.should_quit);
        assert!(state.show_comments);
    }

    #[test]
    fn mouse_selection_normalizes_drag_direction() {
        let forward = MouseSelection {
            pane: PaneFocus::Diff,
            start: TextAnchor { line: 2, column: 5 },
            end: TextAnchor {
                line: 4,
                column: 10,
            },
            word_selected: false,
        };
        assert_eq!(
            forward.normalized(),
            (
                TextAnchor { line: 2, column: 5 },
                TextAnchor {
                    line: 4,
                    column: 10,
                }
            )
        );

        let backward = MouseSelection {
            pane: PaneFocus::Diff,
            start: TextAnchor {
                line: 4,
                column: 10,
            },
            end: TextAnchor { line: 2, column: 5 },
            word_selected: false,
        };
        assert_eq!(
            backward.normalized(),
            (
                TextAnchor { line: 2, column: 5 },
                TextAnchor {
                    line: 4,
                    column: 10,
                }
            )
        );
    }
}
