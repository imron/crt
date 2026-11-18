//! TUI-owned presentation state.

use crate::core::PromptId;
use crate::tui::render::diff_view::DiffCache;

/// Current terminal input mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InputMode {
    /// Normal mode — keys are dispatched as commands.
    Normal,
    /// Command mode — `:` prompt is active, collecting user input.
    Command,
    /// Diff search mode — `/` prompt is active, collecting search query.
    DiffSearch,
}

#[derive(Default)]
pub(crate) struct TuiState {
    pub diff_cache: Option<DiffCache>,
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
}
