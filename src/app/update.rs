//! App-owned update step for core interaction effects.
//!
//! Input adapters feed `InputEvent`s to core interaction. The resulting
//! `CoreEffect`s are applied here to mutate the current app state, enqueue
//! async work for the app loop, or report presentation updates to the UI.

use super::{AppState, JumpLocation};
use crate::core::command::{Command, CommandParse};
use crate::core::navigation::{self, Direction, FileNavigationScope};
use crate::core::search as core_search;
use crate::core::{
    CoreEffect, DiffCursorEffect, DiffSearchEffect, InteractionContext, PaneEffect, PaneId,
};
use crate::review_types::{ContentMode, PaneFocus, RenderVariant, ReviewStatus};

pub trait AppViewport {
    fn hunk_start_rows(&self) -> &[usize];
    fn hunk_end_rows(&self) -> &[usize];
    fn hunk_first_change_rows(&self) -> &[usize];
    fn diff_gutter_cols(&self) -> usize;
    fn diff_content_height(&self) -> usize;
    fn diff_view_height(&self) -> usize;
    fn diff_rendered_text(&self) -> &[String];

    fn max_diff_scroll(&self) -> usize {
        self.diff_content_height().saturating_sub(1)
    }

    fn diff_content_start_col(&self) -> usize {
        if self.diff_gutter_cols() > 0 {
            self.diff_gutter_cols() + 3
        } else {
            0
        }
    }

    fn line_content_trimmed(&self, row: usize) -> &str {
        let line = match self.diff_rendered_text().get(row) {
            Some(line) => line.as_str(),
            None => return "",
        };
        let start = self.diff_content_start_col().min(line.len());
        line[start..].trim_end()
    }

    fn current_line_text_len(&self, state: &AppState) -> usize {
        self.line_content_trimmed(state.diff_line_cursor)
            .chars()
            .count()
    }
}

#[derive(Debug, Default)]
pub struct AppOutput {
    pub handled: bool,
    pub status: Option<StatusUpdate>,
    pending_review_toggle: bool,
    pub should_suspend: bool,
    pub should_quit: bool,
    pub pending_command: Option<Command>,
    pub save_layout: bool,
    pub show_comments: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusUpdate {
    Set(String),
    Clear,
}

impl AppOutput {
    fn handled() -> Self {
        Self {
            handled: true,
            ..Self::default()
        }
    }

    fn set_status(&mut self, message: impl Into<String>) {
        self.status = Some(StatusUpdate::Set(message.into()));
    }

    fn clear_status(&mut self) {
        self.status = Some(StatusUpdate::Clear);
    }

    fn request_review_toggle(&mut self) {
        self.pending_review_toggle = true;
    }

    pub(super) fn take_pending_review_toggle(&mut self) -> bool {
        let pending = self.pending_review_toggle;
        self.pending_review_toggle = false;
        pending
    }

    fn request_suspend(&mut self) {
        self.should_suspend = true;
    }

    fn request_quit(&mut self) {
        self.should_quit = true;
    }

    fn request_command(&mut self, command: Command) {
        self.pending_command = Some(command);
    }

    fn request_layout_save(&mut self) {
        self.save_layout = true;
    }

    fn set_comments_visible(&mut self, show: bool) {
        self.show_comments = Some(show);
    }
}

pub(super) fn interaction_context(state: &AppState) -> InteractionContext {
    InteractionContext {
        focused_pane: Some(match state.pane_focus {
            PaneFocus::FileList => PaneId::FileList,
            PaneFocus::Diff => PaneId::Diff,
        }),
        diff_search_active: state.diff_search_query.is_some(),
        diff_search_has_matches: state.diff_search_query.is_some()
            && !state.diff_search_matches.is_empty(),
        ..InteractionContext::default()
    }
}

pub(super) fn prompt_submit_context(
    state: &AppState,
    view: &impl AppViewport,
) -> InteractionContext {
    InteractionContext {
        fallback_word: extract_word_at_cursor(state, view),
        ..InteractionContext::default()
    }
}

pub(super) fn apply_core_effects(
    state: &mut AppState,
    view: &impl AppViewport,
    effects: Vec<CoreEffect>,
) -> AppOutput {
    let mut update = AppOutput::default();
    for effect in effects {
        update.handled = true;
        match effect {
            CoreEffect::RequestPrompt(_) => {}
            CoreEffect::Status(status) => {
                update.set_status(status.text);
            }
            CoreEffect::ClearPrompt { .. } => {}
            CoreEffect::Command(command) => {
                apply_command(state, &mut update, command);
            }
            CoreEffect::DiffSearch(DiffSearchEffect::Submit { query }) => {
                apply_diff_search(state, view, &mut update, query);
            }
            CoreEffect::DiffSearch(DiffSearchEffect::NextMatch) => {
                navigate_diff_search_match(state, view, &mut update, Direction::Next);
            }
            CoreEffect::DiffSearch(DiffSearchEffect::PreviousMatch) => {
                navigate_diff_search_match(state, view, &mut update, Direction::Prev);
            }
            CoreEffect::DiffSearch(DiffSearchEffect::Clear) => {
                clear_diff_search(state);
            }
            CoreEffect::DiffCursor(effect) => {
                apply_diff_cursor_effect(state, view, effect);
            }
            CoreEffect::SearchResults(_) | CoreEffect::DefinitionResults(_) => {}
            CoreEffect::Pane(effect) => {
                apply_pane_effect(state, effect);
            }
            CoreEffect::Quit => {
                update.request_quit();
            }
            CoreEffect::Suspend => {
                update.request_suspend();
            }
            CoreEffect::ShowHelp | CoreEffect::DismissHelp => {}
            CoreEffect::ReviewToggle => {
                toggle_review(state, &mut update);
                update.clear_status();
            }
            CoreEffect::NavigateFile(direction) => {
                navigate_file(state, direction);
                update.clear_status();
            }
            CoreEffect::JumpHunk(Direction::Next) => {
                jump_to_next_hunk(state, view);
                update.clear_status();
            }
            CoreEffect::JumpHunk(Direction::Prev) => {
                jump_to_prev_hunk(state, view);
                update.clear_status();
            }
            CoreEffect::GoToDefinition => {
                request_go_to_definition(state, view, &mut update);
            }
            CoreEffect::PopJumpStack => {
                pop_jump_stack(state, view, &mut update);
            }
            CoreEffect::TogglePaneFocus | CoreEffect::TogglePaneVisibility(_) => {}
            CoreEffect::ToggleInlineDiff => {
                toggle_inline_diff(state, &mut update);
            }
            CoreEffect::CycleViewMode => {
                update.clear_status();
                cycle_view_mode(state, view);
            }
            CoreEffect::CycleDiffAlgorithm => {
                cycle_diff_algorithm(state, &mut update);
            }
            CoreEffect::ToggleDiffBase => {
                toggle_diff_base(state, &mut update);
            }
            CoreEffect::ConnectionState(_) | CoreEffect::TransientError(_) => {}
        }
    }
    update
}

/// Apply a parsed command emitted by the core interaction engine.
fn apply_command(state: &mut AppState, update: &mut AppOutput, command: CommandParse) {
    match command {
        CommandParse::Empty => {}
        CommandParse::NeedsArgument { usage } | CommandParse::NeedsWord { usage } => {
            update.set_status(usage);
        }
        CommandParse::Parsed(Command::Quit) => {
            update.request_quit();
        }
        CommandParse::Parsed(
            cmd @ (Command::SearchAll { .. }
            | Command::SearchDiff { .. }
            | Command::FindDefinition { .. }),
        ) => {
            update.request_command(cmd);
        }
        CommandParse::Parsed(Command::SetBlame(true)) => {
            state.show_blame = true;
            state.load_blame();
            update.set_status("Blame: shown");
        }
        CommandParse::Parsed(Command::SetBlame(false)) => {
            state.show_blame = false;
            state.load_blame();
            update.set_status("Blame: hidden");
        }
        CommandParse::Parsed(Command::SetComments(show)) => {
            update.set_comments_visible(show);
            let status = if show {
                "Comments: shown"
            } else {
                "Comments: hidden"
            };
            update.set_status(status);
        }
        CommandParse::Parsed(Command::SetWhitespaceIgnored(false)) => {
            state.ignore_whitespace = false;
            state.reload_current_diff();
            update.set_status("Whitespace: shown");
        }
        CommandParse::Parsed(Command::SetWhitespaceIgnored(true)) => {
            state.ignore_whitespace = true;
            state.reload_current_diff();
            update.set_status("Whitespace: ignored");
        }
        CommandParse::Parsed(Command::ViewFile { .. }) => {
            update.set_status("Unsupported command from prompt");
        }
        CommandParse::Parsed(Command::Unknown { name }) => {
            if name == "set" || name.starts_with("set ") {
                update.set_status(
                    "Unknown option. Use: blame, noblame, comments, nocomments, whitespace, nowhitespace",
                );
            } else {
                update.set_status(format!("Unknown command: {name}"));
            }
        }
    }
}

fn apply_diff_search(
    state: &mut AppState,
    view: &impl AppViewport,
    update: &mut AppOutput,
    query: String,
) {
    if query.is_empty() {
        state.diff_search_query = None;
        state.diff_search_matches.clear();
        state.diff_search_current = 0;
    } else {
        state.diff_search_query = Some(query);
        if let Some(err) = recompute_diff_search_matches(state, view) {
            state.diff_search_query = None;
            update.set_status(err);
        } else if state.diff_search_matches.is_empty() {
            update.set_status("No matches");
        } else {
            diff_search_jump_to_current(state, view);
            let total = state.diff_search_matches.len();
            let cur = state.diff_search_current + 1;
            update.set_status(format!("{cur}/{total}"));
        }
    }
}

fn navigate_diff_search_match(
    state: &mut AppState,
    view: &impl AppViewport,
    update: &mut AppOutput,
    direction: Direction,
) {
    if state.diff_search_query.is_none() || state.diff_search_matches.is_empty() {
        return;
    }

    let cursor = state.diff_line_cursor;
    let len = state.diff_search_matches.len();
    let idx = match direction {
        Direction::Next => state
            .diff_search_matches
            .iter()
            .position(|(row, _, _)| *row > cursor)
            .unwrap_or(0),
        Direction::Prev => state
            .diff_search_matches
            .iter()
            .rposition(|(row, _, _)| *row < cursor)
            .unwrap_or(len - 1),
    };

    state.diff_search_current = idx;
    let (row, _, _) = state.diff_search_matches[idx];
    state.diff_line_cursor = row;
    clamp_cursor_and_scroll(state, view);
    let cur = idx + 1;
    update.set_status(format!("{cur}/{len}"));
}

fn clear_diff_search(state: &mut AppState) {
    state.diff_search_query = None;
    state.diff_search_matches.clear();
    state.diff_search_current = 0;
}

fn apply_diff_cursor_effect(
    state: &mut AppState,
    view: &impl AppViewport,
    effect: DiffCursorEffect,
) {
    match effect {
        DiffCursorEffect::MoveTo { line, column } => {
            state.diff_line_cursor = line;
            state.diff_col_cursor = column;
            clamp_cursor_and_scroll(state, view);
            clamp_col_cursor(state, view);
        }
        DiffCursorEffect::LineDown => {
            state.diff_line_cursor = state.diff_line_cursor.saturating_add(1);
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
        }
        DiffCursorEffect::LineUp => {
            state.diff_line_cursor = state.diff_line_cursor.saturating_sub(1);
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
        }
        DiffCursorEffect::PageDown => {
            let delta = view.diff_view_height();
            state.diff_line_cursor = state.diff_line_cursor.saturating_add(delta);
            state.diff_scroll = state.diff_scroll.saturating_add(delta);
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
        }
        DiffCursorEffect::PageUp => {
            let delta = view.diff_view_height();
            state.diff_line_cursor = state.diff_line_cursor.saturating_sub(delta);
            state.diff_scroll = state.diff_scroll.saturating_sub(delta);
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
        }
        DiffCursorEffect::HalfPageDown => {
            let delta = view.diff_view_height() / 2;
            state.diff_line_cursor = state.diff_line_cursor.saturating_add(delta);
            state.diff_scroll = state.diff_scroll.saturating_add(delta);
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
        }
        DiffCursorEffect::HalfPageUp => {
            let delta = view.diff_view_height() / 2;
            state.diff_line_cursor = state.diff_line_cursor.saturating_sub(delta);
            state.diff_scroll = state.diff_scroll.saturating_sub(delta);
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
        }
        DiffCursorEffect::ScrollDown => {
            state.diff_scroll = state.diff_scroll.saturating_add(1);
            clamp_diff_scroll(state, view);
            if state.diff_line_cursor < state.diff_scroll {
                state.diff_line_cursor = state.diff_scroll;
                state.diff_col_cursor = 0;
            }
        }
        DiffCursorEffect::ScrollUp => {
            state.diff_scroll = state.diff_scroll.saturating_sub(1);
            if view.diff_view_height() > 0
                && state.diff_line_cursor >= state.diff_scroll + view.diff_view_height()
            {
                state.diff_line_cursor = state.diff_scroll + view.diff_view_height() - 1;
                state.diff_col_cursor = 0;
            }
        }
        DiffCursorEffect::WheelDown => {
            state.diff_scroll = state.diff_scroll.saturating_add(3);
            clamp_diff_scroll(state, view);
            if state.diff_line_cursor < state.diff_scroll {
                state.diff_line_cursor = state.diff_scroll;
            }
        }
        DiffCursorEffect::WheelUp => {
            state.diff_scroll = state.diff_scroll.saturating_sub(3);
            let bottom = state
                .diff_scroll
                .saturating_add(view.diff_view_height().saturating_sub(1));
            if state.diff_line_cursor > bottom {
                state.diff_line_cursor = bottom;
            }
        }
        DiffCursorEffect::Top => {
            state.diff_line_cursor = 0;
            state.diff_scroll = 0;
            state.diff_col_cursor = 0;
        }
        DiffCursorEffect::Bottom => {
            state.diff_line_cursor = view.max_diff_scroll();
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
        }
        DiffCursorEffect::ViewTop => {
            state.diff_line_cursor = state.diff_scroll;
            state.diff_col_cursor = 0;
        }
        DiffCursorEffect::ViewMiddle => {
            let mid = view.diff_view_height() / 2;
            state.diff_line_cursor = state.diff_scroll + mid;
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
        }
        DiffCursorEffect::ViewBottom => {
            let bottom = view.diff_view_height().saturating_sub(1);
            state.diff_line_cursor = state.diff_scroll + bottom;
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
        }
        DiffCursorEffect::CharLeft => {
            state.diff_col_cursor = state.diff_col_cursor.saturating_sub(1);
        }
        DiffCursorEffect::CharRight => {
            let max = view.current_line_text_len(state).saturating_sub(1);
            if state.diff_col_cursor < max {
                state.diff_col_cursor += 1;
            }
        }
        DiffCursorEffect::LineStart => {
            state.diff_col_cursor = 0;
        }
        DiffCursorEffect::LineEnd => {
            state.diff_col_cursor = view.current_line_text_len(state).saturating_sub(1);
        }
        DiffCursorEffect::WordForward => {
            word_forward(state, view);
        }
        DiffCursorEffect::WordBackward => {
            word_backward(state, view);
        }
        DiffCursorEffect::BigWordForward => {
            bigword_forward(state, view);
        }
        DiffCursorEffect::BigWordBackward => {
            bigword_backward(state, view);
        }
    }
}

fn apply_pane_effect(state: &mut AppState, effect: PaneEffect) {
    match effect {
        PaneEffect::ActivateFileListSelection => {}
        PaneEffect::ActivateDiffSelection => {
            if let Some(entry) = state.selected_file_entry() {
                if matches!(entry.status, ReviewStatus::Reviewed { .. })
                    && !state.reviewed_diff_expanded
                {
                    state.reviewed_diff_expanded = true;
                    state.diff_scroll = 0;
                }
            }
        }
        PaneEffect::SelectFile { file_index } => {
            if state.files.get(file_index).is_some() {
                if file_index != state.selected_file {
                    state.selected_file = file_index;
                    state.on_file_changed();
                }
            }
        }
    }
}

pub(super) fn navigate_to_search_match(
    state: &mut AppState,
    view: &impl AppViewport,
    m: &crate::review_types::SearchMatch,
) -> AppOutput {
    let mut update = AppOutput::handled();
    let target = core_search::resolve_search_target(&state.files, m);
    navigate_to_location_target(state, view, &mut update, target);
    update
}

pub(super) fn navigate_to_definition(
    state: &mut AppState,
    view: &impl AppViewport,
    def: &crate::review_types::DefinitionLocation,
) -> AppOutput {
    let mut update = AppOutput::handled();
    let target = core_search::resolve_definition_target(&state.files, def);
    navigate_to_location_target(state, view, &mut update, target);
    update
}

fn navigate_to_location_target(
    state: &mut AppState,
    view: &impl AppViewport,
    update: &mut AppOutput,
    target: core_search::LocationTarget,
) {
    match target {
        core_search::LocationTarget::InDiff {
            file_index,
            line_number,
        } => {
            push_jump_stack(state);
            state.selected_file = file_index;
            on_file_changed(state);
            state.diff_line_cursor = (line_number as usize).saturating_sub(1);
            clamp_cursor_and_scroll(state, view);
        }
        core_search::LocationTarget::External {
            file_path,
            line_number,
        } => {
            push_jump_stack(state);
            update.request_command(Command::ViewFile {
                path: file_path,
                line_number,
            });
        }
    }
}

fn push_jump_stack(state: &mut AppState) {
    state.jump_stack.push(JumpLocation {
        file_index: state.selected_file,
        diff_scroll: state.diff_scroll,
        diff_line_cursor: state.diff_line_cursor,
        content_mode: state.content_mode,
        render_variant: state.render_variant,
    });
}

fn toggle_inline_diff(state: &mut AppState, update: &mut AppOutput) {
    if state.content_mode == ContentMode::Diff {
        state.render_variant = match state.render_variant {
            RenderVariant::Inline => RenderVariant::SideBySide,
            other => other,
        };
    }
    update.clear_status();
}

fn cycle_diff_algorithm(state: &mut AppState, update: &mut AppOutput) {
    state.diff_algorithm = state.diff_algorithm.next();
    state.reload_current_diff();
    update.set_status(format!("Diff algorithm: {}", state.diff_algorithm.label()));
    update.request_layout_save();
}

fn toggle_diff_base(state: &mut AppState, update: &mut AppOutput) {
    let has_reviewed_commit = state.selected_file_entry().is_some_and(|e| {
        matches!(
            &e.status,
            ReviewStatus::Reviewed {
                reviewed_commit: Some(_),
                ..
            } | ReviewStatus::Changed {
                reviewed_commit: Some(_),
                ..
            }
        )
    });
    if has_reviewed_commit {
        state.show_merge_base = !state.show_merge_base;
        state.reload_current_diff();
        let label = if state.show_merge_base {
            "Diff base: merge base"
        } else {
            "Diff base: since review"
        };
        update.set_status(label);
    } else {
        update.set_status("File not yet reviewed");
    }
}

/// Pop the jump stack and restore the previous location.
fn pop_jump_stack(state: &mut AppState, view: &impl AppViewport, update: &mut AppOutput) {
    if let Some(loc) = state.jump_stack.pop() {
        if loc.file_index != state.selected_file && loc.file_index < state.files.len() {
            state.selected_file = loc.file_index;
            on_file_changed(state);
        }
        state.diff_scroll = loc.diff_scroll;
        state.diff_line_cursor = loc.diff_line_cursor;
        state.content_mode = loc.content_mode;
        state.render_variant = loc.render_variant;
        clamp_cursor_and_scroll(state, view);
        update.set_status(format!("Jump stack: {} remaining", state.jump_stack.len()));
    } else {
        update.set_status("Jump stack empty");
    }
}

/// Request go-to-definition for the word under the cursor.
fn request_go_to_definition(state: &mut AppState, view: &impl AppViewport, update: &mut AppOutput) {
    let word = extract_word_at_cursor(state, view);
    if let Some(word) = word {
        if word.is_empty() {
            update.set_status("No word under cursor");
        } else {
            update.request_command(Command::FindDefinition { symbol: word });
        }
    } else {
        update.set_status("No word under cursor");
    }
}

/// Extract the identifier-like word under the current diff column cursor.
fn extract_word_at_cursor(state: &AppState, view: &impl AppViewport) -> Option<String> {
    let content = content_for_word_extraction(state, view)?;
    word_at_char_offset(content, state.diff_col_cursor)
}

fn content_for_word_extraction<'a>(
    state: &AppState,
    view: &'a impl AppViewport,
) -> Option<&'a str> {
    let line = view.diff_rendered_text().get(state.diff_line_cursor)?;
    let gutter = view.diff_gutter_cols();
    let content = if gutter < line.len() {
        &line[gutter..]
    } else {
        line.as_str()
    };
    let content = if content.len() >= 3 {
        let prefix = &content[..3];
        if prefix == "+ "
            || prefix == "- "
            || prefix == "  "
            || prefix.starts_with(" + ")
            || prefix.starts_with(" - ")
        {
            content[3..].trim_start()
        } else {
            content.trim()
        }
    } else {
        content.trim()
    };

    Some(content)
}

fn word_at_char_offset(content: &str, column: usize) -> Option<String> {
    let chars: Vec<char> = content.chars().collect();
    let ch = *chars.get(column)?;
    if !is_identifier_char(ch) {
        return None;
    }

    let mut start = column;
    while start > 0 && is_identifier_char(chars[start - 1]) {
        start -= 1;
    }

    let mut end = column;
    while end + 1 < chars.len() && is_identifier_char(chars[end + 1]) {
        end += 1;
    }

    Some(chars[start..=end].iter().collect())
}

fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn clamp_diff_scroll(state: &mut AppState, view: &impl AppViewport) {
    state.diff_scroll = state.diff_scroll.min(view.max_diff_scroll());
}

fn clamp_cursor_and_scroll(state: &mut AppState, view: &impl AppViewport) {
    let max = view.max_diff_scroll();
    state.diff_line_cursor = state.diff_line_cursor.min(max);
    if state.diff_line_cursor < state.diff_scroll {
        state.diff_scroll = state.diff_line_cursor;
    }
    if view.diff_view_height() > 0
        && state.diff_line_cursor >= state.diff_scroll + view.diff_view_height()
    {
        state.diff_scroll = state
            .diff_line_cursor
            .saturating_sub(view.diff_view_height() - 1);
    }
    clamp_diff_scroll(state, view);
}

fn clamp_col_cursor(state: &mut AppState, view: &impl AppViewport) {
    let max = view.current_line_text_len(state).saturating_sub(1);
    state.diff_col_cursor = state.diff_col_cursor.min(max);
}

fn recompute_diff_search_matches(state: &mut AppState, view: &impl AppViewport) -> Option<String> {
    state.diff_search_matches.clear();
    state.diff_search_current = 0;
    let query = match &state.diff_search_query {
        Some(q) if !q.is_empty() => q.clone(),
        _ => return None,
    };
    let re = match regex::RegexBuilder::new(&query)
        .case_insensitive(true)
        .build()
    {
        Ok(re) => re,
        Err(e) => {
            let msg = e.to_string();
            let short = msg
                .lines()
                .next()
                .unwrap_or(&msg)
                .trim_start_matches("regex parse error:")
                .trim();
            return Some(format!("Invalid regex: {short}"));
        }
    };
    for (row, line) in view.diff_rendered_text().iter().enumerate() {
        let search_start = view.diff_gutter_cols().min(line.len());
        let content = &line[search_start..];
        for m in re.find_iter(content) {
            if m.start() == m.end() {
                continue;
            }
            let abs_start = search_start + m.start();
            let abs_end = search_start + m.end();
            state.diff_search_matches.push((row, abs_start, abs_end));
        }
    }
    None
}

fn diff_search_jump_to_current(state: &mut AppState, view: &impl AppViewport) {
    if state.diff_search_matches.is_empty() {
        return;
    }
    let idx = state
        .diff_search_matches
        .iter()
        .position(|(row, _, _)| *row >= state.diff_line_cursor)
        .unwrap_or(0);
    state.diff_search_current = idx;
    let (row, _, _) = state.diff_search_matches[idx];
    state.diff_line_cursor = row;
    clamp_cursor_and_scroll(state, view);
}

fn on_file_changed(state: &mut AppState) {
    state.on_file_changed();
}

fn cycle_view_mode(state: &mut AppState, view: &impl AppViewport) {
    let approx_line = estimate_current_line(state, view);

    match (&state.content_mode, &state.render_variant) {
        (ContentMode::Diff, _) => {
            state.content_mode = ContentMode::FullFile;
            state.render_variant = RenderVariant::HeadVersion;
            let row = approx_line.saturating_sub(1);
            state.diff_line_cursor = row;
            state.diff_scroll = row;
        }
        (ContentMode::FullFile, RenderVariant::HeadVersion) => {
            if state.base_content.is_none() {
                state.load_base_content();
            }
            state.render_variant = RenderVariant::BaseVersion;
            let base_line =
                navigation::map_new_to_old_line(state.selected_file_entry(), approx_line);
            let row = base_line.saturating_sub(1);
            state.diff_line_cursor = row;
            state.diff_scroll = row;
        }
        (ContentMode::FullFile, RenderVariant::BaseVersion) => {
            state.content_mode = ContentMode::Diff;
            state.render_variant = RenderVariant::Inline;
            let old_line = state.diff_line_cursor + 1;
            let new_line = navigation::map_old_to_new_line(state.selected_file_entry(), old_line);
            let row = new_line.saturating_sub(1);
            state.diff_line_cursor = row;
            state.diff_scroll = row;
        }
        _ => {
            state.content_mode = ContentMode::Diff;
            state.render_variant = RenderVariant::Inline;
        }
    }
}

fn estimate_current_line(state: &AppState, view: &impl AppViewport) -> usize {
    match state.content_mode {
        ContentMode::Diff => {
            let scroll = state.diff_line_cursor;
            if let Some(entry) = state.selected_file_entry() {
                let mut deletions_above = 0usize;
                for (i, &start) in view.hunk_start_rows().iter().enumerate() {
                    let end = view.hunk_end_rows().get(i).copied().unwrap_or(start);
                    if start > scroll {
                        break;
                    }
                    let hunk = &entry.diff.hunks[i.min(entry.diff.hunks.len() - 1)];
                    let hunk_scroll_end = scroll.min(end);
                    for (j, line) in hunk.lines.iter().enumerate() {
                        if start + j > hunk_scroll_end {
                            break;
                        }
                        if line.kind == crate::review_types::LineKind::Deletion
                            && start + j <= scroll
                        {
                            deletions_above += 1;
                        }
                    }
                }
                scroll.saturating_sub(deletions_above) + 1
            } else {
                scroll + 1
            }
        }
        ContentMode::FullFile => state.diff_line_cursor + 1,
    }
}

fn jump_to_next_hunk(state: &mut AppState, view: &impl AppViewport) {
    if let Some(jump) = navigation::jump_to_next_hunk(
        state.diff_line_cursor,
        state.diff_scroll,
        view.diff_view_height(),
        view.hunk_first_change_rows(),
        view.hunk_end_rows(),
    ) {
        state.diff_line_cursor = jump.cursor;
        state.diff_scroll = jump.scroll;
        state.diff_col_cursor = 0;
        clamp_cursor_and_scroll(state, view);
    }
}

fn jump_to_prev_hunk(state: &mut AppState, view: &impl AppViewport) {
    if let Some(jump) = navigation::jump_to_prev_hunk(
        state.diff_line_cursor,
        state.diff_scroll,
        view.diff_view_height(),
        view.hunk_first_change_rows(),
        view.hunk_end_rows(),
    ) {
        state.diff_line_cursor = jump.cursor;
        state.diff_scroll = jump.scroll;
        state.diff_col_cursor = 0;
        clamp_cursor_and_scroll(state, view);
    }
}

fn char_class(c: char) -> u8 {
    if c.is_alphanumeric() || c == '_' {
        0
    } else if c.is_whitespace() {
        1
    } else {
        2
    }
}

fn word_forward(state: &mut AppState, view: &impl AppViewport) {
    let content = view.line_content_trimmed(state.diff_line_cursor);
    let chars: Vec<char> = content.chars().collect();
    let text_len = chars.len();

    if text_len == 0 || state.diff_col_cursor >= text_len.saturating_sub(1) {
        let max_line = view.diff_content_height().saturating_sub(1);
        if state.diff_line_cursor < max_line {
            state.diff_line_cursor += 1;
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
            let new_content = view.line_content_trimmed(state.diff_line_cursor);
            let new_chars: Vec<char> = new_content.chars().collect();
            let mut pos = 0;
            while pos < new_chars.len() && new_chars[pos].is_whitespace() {
                pos += 1;
            }
            state.diff_col_cursor = pos.min(new_chars.len().saturating_sub(1));
        }
        return;
    }

    let mut pos = state.diff_col_cursor;
    let start_class = char_class(chars[pos]);
    while pos < text_len && char_class(chars[pos]) == start_class {
        pos += 1;
    }
    while pos < text_len && chars[pos].is_whitespace() {
        pos += 1;
    }
    if pos >= text_len {
        let max_line = view.diff_content_height().saturating_sub(1);
        if state.diff_line_cursor < max_line {
            state.diff_line_cursor += 1;
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
            let new_content = view.line_content_trimmed(state.diff_line_cursor);
            let new_chars: Vec<char> = new_content.chars().collect();
            let mut p = 0;
            while p < new_chars.len() && new_chars[p].is_whitespace() {
                p += 1;
            }
            state.diff_col_cursor = p.min(new_chars.len().saturating_sub(1));
        } else {
            state.diff_col_cursor = text_len.saturating_sub(1);
        }
        return;
    }
    state.diff_col_cursor = pos;
}

fn word_backward(state: &mut AppState, view: &impl AppViewport) {
    if state.diff_col_cursor == 0 {
        if state.diff_line_cursor > 0 {
            state.diff_line_cursor -= 1;
            clamp_cursor_and_scroll(state, view);
            let content = view.line_content_trimmed(state.diff_line_cursor);
            let chars: Vec<char> = content.chars().collect();
            if chars.is_empty() {
                state.diff_col_cursor = 0;
            } else {
                let end = chars.len() - 1;
                let mut pos = end;
                while pos > 0 && chars[pos].is_whitespace() {
                    pos -= 1;
                }
                let target_class = char_class(chars[pos]);
                while pos > 0 && char_class(chars[pos - 1]) == target_class {
                    pos -= 1;
                }
                state.diff_col_cursor = pos;
            }
        }
        return;
    }

    let content = view.line_content_trimmed(state.diff_line_cursor);
    let chars: Vec<char> = content.chars().collect();
    if chars.is_empty() {
        return;
    }
    let mut pos = state.diff_col_cursor;
    pos -= 1;
    while pos > 0 && chars[pos].is_whitespace() {
        pos -= 1;
    }
    let target_class = char_class(chars[pos]);
    while pos > 0 && char_class(chars[pos - 1]) == target_class {
        pos -= 1;
    }
    state.diff_col_cursor = pos;
}

fn bigword_forward(state: &mut AppState, view: &impl AppViewport) {
    let content = view.line_content_trimmed(state.diff_line_cursor);
    let chars: Vec<char> = content.chars().collect();
    let text_len = chars.len();

    if text_len == 0 || state.diff_col_cursor >= text_len.saturating_sub(1) {
        let max_line = view.diff_content_height().saturating_sub(1);
        if state.diff_line_cursor < max_line {
            state.diff_line_cursor += 1;
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
            let new_content = view.line_content_trimmed(state.diff_line_cursor);
            let new_chars: Vec<char> = new_content.chars().collect();
            let mut pos = 0;
            while pos < new_chars.len() && new_chars[pos].is_whitespace() {
                pos += 1;
            }
            state.diff_col_cursor = pos.min(new_chars.len().saturating_sub(1));
        }
        return;
    }

    let mut pos = state.diff_col_cursor;
    while pos < text_len && !chars[pos].is_whitespace() {
        pos += 1;
    }
    while pos < text_len && chars[pos].is_whitespace() {
        pos += 1;
    }
    if pos >= text_len {
        let max_line = view.diff_content_height().saturating_sub(1);
        if state.diff_line_cursor < max_line {
            state.diff_line_cursor += 1;
            state.diff_col_cursor = 0;
            clamp_cursor_and_scroll(state, view);
            let new_content = view.line_content_trimmed(state.diff_line_cursor);
            let new_chars: Vec<char> = new_content.chars().collect();
            let mut p = 0;
            while p < new_chars.len() && new_chars[p].is_whitespace() {
                p += 1;
            }
            state.diff_col_cursor = p.min(new_chars.len().saturating_sub(1));
        } else {
            state.diff_col_cursor = text_len.saturating_sub(1);
        }
        return;
    }
    state.diff_col_cursor = pos;
}

fn bigword_backward(state: &mut AppState, view: &impl AppViewport) {
    if state.diff_col_cursor == 0 {
        if state.diff_line_cursor > 0 {
            state.diff_line_cursor -= 1;
            clamp_cursor_and_scroll(state, view);
            let content = view.line_content_trimmed(state.diff_line_cursor);
            let chars: Vec<char> = content.chars().collect();
            if chars.is_empty() {
                state.diff_col_cursor = 0;
            } else {
                let mut pos = chars.len() - 1;
                while pos > 0 && chars[pos].is_whitespace() {
                    pos -= 1;
                }
                while pos > 0 && !chars[pos - 1].is_whitespace() {
                    pos -= 1;
                }
                state.diff_col_cursor = pos;
            }
        }
        return;
    }

    let content = view.line_content_trimmed(state.diff_line_cursor);
    let chars: Vec<char> = content.chars().collect();
    if chars.is_empty() {
        return;
    }
    let mut pos = state.diff_col_cursor - 1;
    while pos > 0 && chars[pos].is_whitespace() {
        pos -= 1;
    }
    while pos > 0 && !chars[pos - 1].is_whitespace() {
        pos -= 1;
    }
    state.diff_col_cursor = pos;
}

fn navigate_file(state: &mut AppState, dir: Direction) {
    let scope = if state.pane_focus == PaneFocus::Diff {
        FileNavigationScope::DiffPane
    } else {
        FileNavigationScope::FileListPane
    };

    if let Some(new_idx) = navigation::navigate_file(
        state.selected_file,
        state.files.len(),
        state.unreviewed_count(),
        scope,
        dir,
    ) {
        state.selected_file = new_idx;
        on_file_changed(state);
    }
}

fn toggle_review(state: &AppState, update: &mut AppOutput) {
    if state.files.get(state.selected_file).is_some() {
        update.request_review_toggle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_at_char_offset_selects_identifier_under_cursor() {
        assert_eq!(
            word_at_char_offset("let first = second_value;", 12),
            Some("second_value".to_string())
        );
    }

    #[test]
    fn word_at_char_offset_selects_identifier_from_underscore() {
        assert_eq!(
            word_at_char_offset("call merge_base now", 10),
            Some("merge_base".to_string())
        );
    }

    #[test]
    fn word_at_char_offset_returns_none_on_separator() {
        assert_eq!(word_at_char_offset("foo.bar", 3), None);
    }

    #[test]
    fn word_at_char_offset_uses_character_offsets() {
        assert_eq!(
            word_at_char_offset("let caf\u{e9}_value = 1", 6),
            Some("caf\u{e9}_value".to_string())
        );
    }
}
