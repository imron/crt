//! TUI-owned presentation state.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::render::diff_view::DiffCache;
use crate::app_update::{AppUpdate, StatusUpdate};
use crate::core::PromptId;
use crate::core::TextAnchor;
use crate::core::command::Command;
use crate::model::PaneFocus;

/// Current terminal input mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputMode {
    /// Normal mode — keys are dispatched as commands.
    Normal,
    /// Command mode — `:` prompt is active, collecting user input.
    Command,
    /// Diff search mode — `/` prompt is active, collecting search query.
    DiffSearch,
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

#[derive(Default)]
pub struct TuiState {
    /// Current file list pane width (configurable, resizable by drag).
    pub file_list_width: u16,
    /// Path to the config file (for persisting TUI layout changes).
    pub config_path: Option<PathBuf>,
    pub diff_cache: Option<DiffCache>,
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
    /// Set when the runtime should toggle the selected file review state.
    pub pending_review_toggle: bool,
    /// Set to true to suspend the process (Ctrl-Z).
    pub should_suspend: bool,
    /// Set to true to exit the event loop.
    pub should_quit: bool,
    /// Set to true when the terminal regains focus.
    pub pending_refresh: bool,
    /// Pending command to execute asynchronously in the TUI runtime.
    pub pending_command: Option<Command>,
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
    /// Active core prompt requested by the interaction engine, if any.
    pub active_core_prompt: Option<PromptId>,
}

impl Default for InputMode {
    fn default() -> Self {
        Self::Normal
    }
}

impl TuiState {
    pub fn new(file_list_width: u16, config_path: Option<PathBuf>) -> Self {
        Self {
            file_list_width,
            config_path,
            ..Self::default()
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
            None => {}
        }
    }

    pub fn apply_app_update(&mut self, update: AppUpdate) {
        self.apply_status_update(update.status);
        self.pending_review_toggle |= update.pending_review_toggle;
        self.should_suspend |= update.should_suspend;
        self.should_quit |= update.should_quit;
        if let Some(command) = update.pending_command {
            self.pending_command = Some(command);
        }
    }

    pub fn has_status_message(&self) -> bool {
        self.status_message.is_some()
    }

    pub fn expire_status_message(&mut self, timeout: Duration) {
        if self
            .status_message
            .as_ref()
            .is_some_and(|(_, when)| when.elapsed() > timeout)
        {
            self.clear_status_message();
        }
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

    pub fn clear_prompt(&mut self) {
        self.active_core_prompt = None;
        self.input_mode = InputMode::Normal;
        self.command_input.clear();
        self.command_cursor = 0;
        self.diff_search_input.clear();
        self.diff_search_cursor = 0;
    }
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
    fn layout_config_is_tui_owned() {
        let state = TuiState::new(48, Some(PathBuf::from("/tmp/crt.toml")));

        assert_eq!(state.file_list_width, 48);
        assert_eq!(state.config_path, Some(PathBuf::from("/tmp/crt.toml")));
    }

    #[test]
    fn clear_prompt_resets_all_prompt_buffers() {
        let mut state = TuiState::default();
        state.open_diff_search_prompt(PromptId(8), "needle".to_string());

        state.clear_prompt();

        assert_eq!(state.active_core_prompt, None);
        assert_eq!(state.input_mode, InputMode::Normal);
        assert!(state.command_input.is_empty());
        assert_eq!(state.command_cursor, 0);
        assert!(state.diff_search_input.is_empty());
        assert_eq!(state.diff_search_cursor, 0);
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
    fn app_updates_set_tui_runtime_intents() {
        let mut state = TuiState::default();

        state.apply_app_update(AppUpdate {
            handled: true,
            status: Some(StatusUpdate::Set("Searching...".to_string())),
            pending_review_toggle: true,
            should_suspend: true,
            should_quit: true,
            pending_command: Some(Command::SearchAll {
                pattern: "needle".to_string(),
            }),
            save_layout: false,
        });

        assert!(state.has_status_message());
        assert!(state.pending_review_toggle);
        assert!(state.should_suspend);
        assert!(state.should_quit);
        assert!(matches!(
            state.pending_command,
            Some(Command::SearchAll { ref pattern }) if pattern == "needle"
        ));
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
