//! Input handling and key dispatch.
//!
//! Key events are dispatched based on the current pane focus. Global keys
//! (like `q` to quit, `Tab` to switch focus) work from any pane.
//! Pane-specific keys are handled by dedicated functions.

use std::time::{Duration, Instant};

use crossterm::event::{
    KeyCode, KeyEvent as CrosstermKeyEvent, KeyEventKind as CrosstermKeyEventKind, KeyModifiers,
};

use crate::app::{AppState, InputMode};
use crate::core::command::{self, Command, CommandParse};
use crate::core::navigation::{self, Direction, FileNavigationScope};
use crate::core::search as core_search;
use crate::core::{
    CoreEffect, DiffSearchEffect, InputEvent, InputModifiers, InteractionContext, Key as CoreKey,
    KeyEvent as CoreKeyEvent, KeyEventKind as CoreKeyEventKind, PromptKind,
};
use crate::model::{ContentMode, PaneFocus, RenderVariant, ReviewStatus};

/// How long the "Press Ctrl-C again" prompt stays active.
const CTRL_C_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) fn input_event_from_key(key: CrosstermKeyEvent) -> Option<InputEvent> {
    Some(InputEvent::Key(CoreKeyEvent {
        kind: match key.kind {
            CrosstermKeyEventKind::Press => CoreKeyEventKind::Press,
            CrosstermKeyEventKind::Repeat => CoreKeyEventKind::Repeat,
            CrosstermKeyEventKind::Release => CoreKeyEventKind::Release,
        },
        key: match key.code {
            KeyCode::Backspace => CoreKey::Backspace,
            KeyCode::Enter => CoreKey::Enter,
            KeyCode::Left => CoreKey::Left,
            KeyCode::Right => CoreKey::Right,
            KeyCode::Up => CoreKey::Up,
            KeyCode::Down => CoreKey::Down,
            KeyCode::Home => CoreKey::Home,
            KeyCode::End => CoreKey::End,
            KeyCode::PageUp => CoreKey::PageUp,
            KeyCode::PageDown => CoreKey::PageDown,
            KeyCode::Tab | KeyCode::BackTab => CoreKey::Tab,
            KeyCode::Delete => CoreKey::Delete,
            KeyCode::Esc => CoreKey::Escape,
            KeyCode::Char(c) => CoreKey::Char(c),
            KeyCode::F(n) => CoreKey::Function(n),
            _ => return None,
        },
        modifiers: InputModifiers {
            ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
            alt: key.modifiers.contains(KeyModifiers::ALT),
            shift: key.modifiers.contains(KeyModifiers::SHIFT),
        },
    }))
}

fn dispatch_core_input(state: &mut AppState, key: CrosstermKeyEvent) -> bool {
    let Some(event) = input_event_from_key(key) else {
        return false;
    };
    let effects = state
        .core_interaction
        .handle_input(event, &InteractionContext::default());
    apply_core_effects(state, effects)
}

fn submit_active_prompt(state: &mut AppState, value: String) -> bool {
    let Some(id) = state.active_core_prompt else {
        return false;
    };
    let fallback_word = extract_word_at_cursor(state);
    let effects = state.core_interaction.handle_input(
        InputEvent::PromptSubmit { id, value },
        &InteractionContext {
            fallback_word,
            ..InteractionContext::default()
        },
    );
    apply_core_effects(state, effects)
}

fn cancel_active_prompt(state: &mut AppState) -> bool {
    let Some(id) = state.active_core_prompt else {
        return false;
    };
    let effects = state.core_interaction.handle_input(
        InputEvent::PromptCancel { id },
        &InteractionContext::default(),
    );
    apply_core_effects(state, effects)
}

fn apply_core_effects(state: &mut AppState, effects: Vec<CoreEffect>) -> bool {
    let mut handled = false;
    for effect in effects {
        handled = true;
        match effect {
            CoreEffect::RequestPrompt(prompt) => match prompt.kind {
                PromptKind::CommandLine => {
                    state.active_core_prompt = Some(prompt.id);
                    state.input_mode = InputMode::Command;
                    state.command_input = prompt.initial_value;
                    state.command_cursor = state.command_input.len();
                }
                PromptKind::Search => {
                    state.active_core_prompt = Some(prompt.id);
                    state.input_mode = InputMode::DiffSearch;
                    state.diff_search_input = prompt.initial_value;
                    state.diff_search_cursor = state.diff_search_input.len();
                }
                PromptKind::Custom(_) => {}
            },
            CoreEffect::Status(status) => {
                state.status_message = Some((status.text, Instant::now()));
            }
            CoreEffect::ClearPrompt { id } => {
                if state.active_core_prompt == Some(id) {
                    state.active_core_prompt = None;
                }
                state.input_mode = InputMode::Normal;
                state.command_input.clear();
                state.command_cursor = 0;
                state.diff_search_input.clear();
                state.diff_search_cursor = 0;
            }
            CoreEffect::Command(command) => {
                apply_command(state, command);
            }
            CoreEffect::DiffSearch(DiffSearchEffect::Submit { query }) => {
                apply_diff_search(state, query);
            }
            CoreEffect::ReviewToggle => {
                toggle_review(state);
                state.status_message = None;
            }
            CoreEffect::NavigateFile(direction) => {
                navigate_file(state, direction);
                state.status_message = None;
            }
            CoreEffect::JumpHunk(Direction::Next) => {
                jump_to_next_hunk(state);
                state.status_message = None;
            }
            CoreEffect::JumpHunk(Direction::Prev) => {
                jump_to_prev_hunk(state);
                state.status_message = None;
            }
            CoreEffect::GoToDefinition => {
                request_go_to_definition(state);
            }
            CoreEffect::PopJumpStack => {
                pop_jump_stack(state);
            }
            CoreEffect::Render(_)
            | CoreEffect::ConnectionState(_)
            | CoreEffect::TransientError(_) => {}
        }
    }
    handled
}

/// Apply a parsed command emitted by the core interaction engine.
fn apply_command(state: &mut AppState, command: CommandParse) {
    match command {
        CommandParse::Empty => {}
        CommandParse::NeedsArgument { usage } | CommandParse::NeedsWord { usage } => {
            state.status_message = Some((usage, Instant::now()));
        }
        CommandParse::Parsed(Command::Quit) => {
            state.should_quit = true;
        }
        CommandParse::Parsed(
            cmd @ (Command::SearchAll { .. }
            | Command::SearchDiff { .. }
            | Command::FindDefinition { .. }),
        ) => {
            state.pending_command = Some(cmd);
        }
        CommandParse::Parsed(Command::SetBlame(true)) => {
            state.show_blame = true;
            state.load_blame();
            state.status_message = Some(("Blame: shown".to_string(), Instant::now()));
        }
        CommandParse::Parsed(Command::SetBlame(false)) => {
            state.show_blame = false;
            state.load_blame();
            state.status_message = Some(("Blame: hidden".to_string(), Instant::now()));
        }
        CommandParse::Parsed(Command::SetComments(show)) => {
            state.show_comments = show;
            let status = if show {
                "Comments: shown"
            } else {
                "Comments: hidden"
            };
            state.status_message = Some((status.to_string(), Instant::now()));
        }
        CommandParse::Parsed(Command::SetWhitespaceIgnored(false)) => {
            state.ignore_whitespace = false;
            state.reload_current_diff();
            state.status_message = Some(("Whitespace: shown".to_string(), Instant::now()));
        }
        CommandParse::Parsed(Command::SetWhitespaceIgnored(true)) => {
            state.ignore_whitespace = true;
            state.reload_current_diff();
            state.status_message = Some(("Whitespace: ignored".to_string(), Instant::now()));
        }
        CommandParse::Parsed(Command::ViewFile { .. }) => {
            state.status_message = Some((
                "Unsupported command from prompt".to_string(),
                Instant::now(),
            ));
        }
        CommandParse::Parsed(Command::Unknown { name }) => {
            if name == "set" || name.starts_with("set ") {
                state.status_message = Some((
                    "Unknown option. Use: blame, noblame, comments, nocomments, whitespace, nowhitespace"
                        .to_string(),
                    Instant::now(),
                ));
            } else {
                state.status_message = Some((format!("Unknown command: {name}"), Instant::now()));
            }
        }
    }
}

fn apply_diff_search(state: &mut AppState, query: String) {
    if query.is_empty() {
        state.diff_search_query = None;
        state.diff_search_matches.clear();
        state.diff_search_current = 0;
    } else {
        state.diff_search_query = Some(query);
        if let Some(err) = state.recompute_diff_search_matches() {
            state.diff_search_query = None;
            state.status_message = Some((err, Instant::now()));
        } else if state.diff_search_matches.is_empty() {
            state.status_message = Some(("No matches".to_string(), Instant::now()));
        } else {
            state.diff_search_jump_to_current();
            let total = state.diff_search_matches.len();
            let cur = state.diff_search_current + 1;
            state.status_message = Some((format!("{cur}/{total}"), Instant::now()));
        }
    }
}

/// Pop the jump stack and restore the previous location.
fn pop_jump_stack(state: &mut AppState) {
    if let Some(loc) = state.jump_stack.pop() {
        if loc.file_index != state.selected_file && loc.file_index < state.files.len() {
            state.selected_file = loc.file_index;
            on_file_changed(state);
        }
        state.diff_scroll = loc.diff_scroll;
        state.diff_line_cursor = loc.diff_line_cursor;
        state.content_mode = loc.content_mode;
        state.render_variant = loc.render_variant;
        state.clamp_cursor_and_scroll();
        state.status_message = Some((
            format!("Jump stack: {} remaining", state.jump_stack.len()),
            Instant::now(),
        ));
    } else {
        state.status_message = Some(("Jump stack empty".to_string(), Instant::now()));
    }
}

/// Request go-to-definition for the word under the cursor.
fn request_go_to_definition(state: &mut AppState) {
    let word = extract_word_at_cursor(state);
    if let Some(word) = word {
        if word.is_empty() {
            state.status_message = Some(("No word under cursor".to_string(), Instant::now()));
        } else {
            state.pending_command = Some(Command::FindDefinition { symbol: word });
        }
    } else {
        state.status_message = Some(("No word under cursor".to_string(), Instant::now()));
    }
}

/// Extract the first identifier-like word from the current cursor line.
fn extract_word_at_cursor(state: &AppState) -> Option<String> {
    let line = state.diff_rendered_text.get(state.diff_line_cursor)?;
    // Skip gutter columns to get to actual content.
    let gutter = state.diff_gutter_cols;
    let content = if gutter < line.len() {
        &line[gutter..]
    } else {
        line.as_str()
    };
    // Skip the prefix marker ("+ ", "- ", "  ") if present.
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

    let start = content.find(|c: char| c.is_alphanumeric() || c == '_')?;
    let word: String = content[start..]
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if word.is_empty() { None } else { Some(word) }
}

/// Handle a key press event by mutating the application state.
pub fn handle_key_event(state: &mut AppState, key: CrosstermKeyEvent) {
    // --- Command mode input ---
    if state.input_mode == InputMode::Command {
        handle_command_input(state, key);
        return;
    }

    // --- Diff search input ---
    if state.input_mode == InputMode::DiffSearch {
        handle_diff_search_input(state, key);
        return;
    }

    // --- Search results overlay catches keys ---
    if state.search_results.is_some() {
        handle_search_results_key(state, key);
        return;
    }

    // --- Definition results overlay catches keys ---
    if state.definition_results.is_some() {
        handle_definition_results_key(state, key);
        return;
    }

    // --- Help overlay catches most keys to dismiss ---
    if state.show_help {
        match key.code {
            KeyCode::Char('?') | KeyCode::Char('q') | KeyCode::Esc => {
                state.show_help = false;
            }
            _ => {}
        }
        return;
    }

    // ---------------------------------------------------------------------------
    // Command mode
    // ---------------------------------------------------------------------------

    /// Handle keystrokes while in command mode (`:` prompt active).
    fn handle_command_input(state: &mut AppState, key: CrosstermKeyEvent) {
        match key.code {
            KeyCode::Esc => {
                if !cancel_active_prompt(state) {
                    state.input_mode = InputMode::Normal;
                    state.command_input.clear();
                    state.command_cursor = 0;
                }
            }
            KeyCode::Enter => {
                let cmd = state.command_input.clone();
                if !submit_active_prompt(state, cmd.clone()) {
                    state.input_mode = InputMode::Normal;
                    state.command_input.clear();
                    state.command_cursor = 0;
                    apply_command(state, command::parse_command(&cmd, None));
                }
            }
            KeyCode::Backspace => {
                if state.command_cursor > 0 {
                    state.command_cursor -= 1;
                    state.command_input.remove(state.command_cursor);
                } else {
                    // Backspace on empty input exits command mode.
                    if !cancel_active_prompt(state) {
                        state.input_mode = InputMode::Normal;
                    }
                }
            }
            KeyCode::Delete => {
                if state.command_cursor < state.command_input.len() {
                    state.command_input.remove(state.command_cursor);
                }
            }
            KeyCode::Left => {
                state.command_cursor = state.command_cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                state.command_cursor = state
                    .command_cursor
                    .min(state.command_input.len())
                    .saturating_add(1);
                state.command_cursor = state.command_cursor.min(state.command_input.len());
            }
            KeyCode::Home => {
                state.command_cursor = 0;
            }
            KeyCode::End => {
                state.command_cursor = state.command_input.len();
            }
            KeyCode::Char(c) => {
                state.command_input.insert(state.command_cursor, c);
                state.command_cursor += 1;
            }
            _ => {}
        }
    }

    /// Handle keystrokes while in diff search mode (`/` prompt active).
    fn handle_diff_search_input(state: &mut AppState, key: CrosstermKeyEvent) {
        match key.code {
            KeyCode::Esc => {
                if !cancel_active_prompt(state) {
                    state.input_mode = InputMode::Normal;
                    state.diff_search_input.clear();
                    state.diff_search_cursor = 0;
                    // Keep existing query/matches (Escape just closes the prompt).
                }
            }
            KeyCode::Enter => {
                let query = state.diff_search_input.clone();
                if !submit_active_prompt(state, query.clone()) {
                    state.input_mode = InputMode::Normal;
                    state.diff_search_input.clear();
                    state.diff_search_cursor = 0;
                    apply_diff_search(state, query);
                }
            }
            KeyCode::Backspace => {
                if state.diff_search_cursor > 0 {
                    state.diff_search_cursor -= 1;
                    state.diff_search_input.remove(state.diff_search_cursor);
                } else {
                    // Backspace on empty input exits search mode.
                    if !cancel_active_prompt(state) {
                        state.input_mode = InputMode::Normal;
                    }
                }
            }
            KeyCode::Delete => {
                if state.diff_search_cursor < state.diff_search_input.len() {
                    state.diff_search_input.remove(state.diff_search_cursor);
                }
            }
            KeyCode::Left => {
                state.diff_search_cursor = state.diff_search_cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                state.diff_search_cursor = state
                    .diff_search_cursor
                    .min(state.diff_search_input.len())
                    .saturating_add(1);
                state.diff_search_cursor =
                    state.diff_search_cursor.min(state.diff_search_input.len());
            }
            KeyCode::Home => {
                state.diff_search_cursor = 0;
            }
            KeyCode::End => {
                state.diff_search_cursor = state.diff_search_input.len();
            }
            KeyCode::Char(c) => {
                state.diff_search_input.insert(state.diff_search_cursor, c);
                state.diff_search_cursor += 1;
            }
            _ => {}
        }
    }

    // ---------------------------------------------------------------------------
    // Search results overlay
    // ---------------------------------------------------------------------------

    /// Handle keystrokes when the search results overlay is visible.
    fn handle_search_results_key(state: &mut AppState, key: CrosstermKeyEvent) {
        let results = state.search_results.as_mut().unwrap();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                state.search_results = None;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if !results.matches.is_empty() {
                    results.selected = (results.selected + 1).min(results.matches.len() - 1);
                    // Auto-scroll to keep selection visible.
                    if results.selected >= results.scroll + 20 {
                        results.scroll = results.selected.saturating_sub(19);
                    }
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                results.selected = results.selected.saturating_sub(1);
                if results.selected < results.scroll {
                    results.scroll = results.selected;
                }
            }
            KeyCode::Char('g') => {
                results.selected = 0;
                results.scroll = 0;
            }
            KeyCode::Char('G') => {
                if !results.matches.is_empty() {
                    results.selected = results.matches.len() - 1;
                    results.scroll = results.selected.saturating_sub(19);
                }
            }
            KeyCode::Enter => {
                if let Some(m) = results.matches.get(results.selected).cloned() {
                    state.search_results = None;
                    navigate_to_search_match(state, &m);
                }
            }
            _ => {}
        }
    }

    /// Handle keystrokes when the definition results overlay is visible.
    fn handle_definition_results_key(state: &mut AppState, key: CrosstermKeyEvent) {
        let results = state.definition_results.as_mut().unwrap();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                state.definition_results = None;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if !results.definitions.is_empty() {
                    results.selected = (results.selected + 1).min(results.definitions.len() - 1);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                results.selected = results.selected.saturating_sub(1);
            }
            KeyCode::Enter => {
                if let Some(def) = results.definitions.get(results.selected).cloned() {
                    state.definition_results = None;
                    navigate_to_definition(state, &def);
                }
            }
            _ => {}
        }
    }

    // ---------------------------------------------------------------------------
    // Navigation helpers
    // ---------------------------------------------------------------------------

    /// Navigate to a search match result.
    fn navigate_to_search_match(state: &mut AppState, m: &crate::model::SearchMatch) {
        let target = core_search::resolve_search_target(&state.files, m);
        navigate_to_location_target(state, target);
    }

    /// Navigate to a definition location.
    fn navigate_to_definition(state: &mut AppState, def: &crate::model::DefinitionLocation) {
        let target = core_search::resolve_definition_target(&state.files, def);
        navigate_to_location_target(state, target);
    }

    fn navigate_to_location_target(state: &mut AppState, target: core_search::LocationTarget) {
        match target {
            core_search::LocationTarget::InDiff {
                file_index,
                line_number,
            } => {
                push_jump_stack(state);
                state.selected_file = file_index;
                on_file_changed(state);
                state.diff_line_cursor = (line_number as usize).saturating_sub(1);
                state.clamp_cursor_and_scroll();
            }
            core_search::LocationTarget::External {
                file_path,
                line_number,
            } => {
                push_jump_stack(state);
                state.pending_command = Some(Command::ViewFile {
                    path: file_path,
                    line_number,
                });
            }
        }
    }

    /// Push current location onto the jump stack.
    fn push_jump_stack(state: &mut AppState) {
        use crate::app::JumpLocation;
        state.jump_stack.push(JumpLocation {
            file_index: state.selected_file,
            diff_scroll: state.diff_scroll,
            diff_line_cursor: state.diff_line_cursor,
            content_mode: state.content_mode,
            render_variant: state.render_variant,
        });
    }

    // --- Global keys (work from any pane) ---
    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            // Double-press Ctrl-C to quit.
            let is_repeat = state.status_message.as_ref().is_some_and(|(msg, when)| {
                msg.contains("Ctrl-C") && when.elapsed() < CTRL_C_TIMEOUT
            });
            if is_repeat {
                state.should_quit = true;
            } else {
                state.status_message =
                    Some(("Press Ctrl-C again to quit".to_string(), Instant::now()));
            }
            return;
        }
        (KeyCode::Char('z'), KeyModifiers::CONTROL) => {
            state.should_suspend = true;
            return;
        }
        (KeyCode::Char('?'), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
            state.show_help = true;
            return;
        }
        (KeyCode::Char('q'), KeyModifiers::NONE) => {
            state.should_quit = true;
            return;
        }
        (KeyCode::Char(':'), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
            if dispatch_core_input(state, key) {
                return;
            }
            state.input_mode = InputMode::Command;
            state.command_input.clear();
            state.command_cursor = 0;
            return;
        }
        (KeyCode::Char('/'), KeyModifiers::NONE) => {
            if dispatch_core_input(state, key) {
                return;
            }
            state.input_mode = InputMode::DiffSearch;
            state.diff_search_input.clear();
            state.diff_search_cursor = 0;
            return;
        }
        (KeyCode::Char('n'), KeyModifiers::NONE) => {
            if state.diff_search_query.is_some() && !state.diff_search_matches.is_empty() {
                // Next match: find first match strictly after current cursor.
                let cursor = state.diff_line_cursor;
                let len = state.diff_search_matches.len();
                let idx = state
                    .diff_search_matches
                    .iter()
                    .position(|(row, _, _)| *row > cursor)
                    .unwrap_or(0); // wrap to first match
                state.diff_search_current = idx;
                let (row, _, _) = state.diff_search_matches[idx];
                state.diff_line_cursor = row;
                state.clamp_cursor_and_scroll();
                let cur = idx + 1;
                state.status_message = Some((format!("{cur}/{len}"), Instant::now()));
                return;
            }
            // Fall through if no active search — `n` might be used elsewhere.
        }
        (KeyCode::Char('N'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
            if state.diff_search_query.is_some() && !state.diff_search_matches.is_empty() {
                // Previous match: find last match strictly before current cursor.
                let cursor = state.diff_line_cursor;
                let len = state.diff_search_matches.len();
                let idx = state
                    .diff_search_matches
                    .iter()
                    .rposition(|(row, _, _)| *row < cursor)
                    .unwrap_or(len - 1); // wrap to last match
                state.diff_search_current = idx;
                let (row, _, _) = state.diff_search_matches[idx];
                state.diff_line_cursor = row;
                state.clamp_cursor_and_scroll();
                let cur = idx + 1;
                state.status_message = Some((format!("{cur}/{len}"), Instant::now()));
                return;
            }
            // Fall through if no active search.
        }
        (KeyCode::Esc, KeyModifiers::NONE) => {
            // Escape clears search highlights.
            if state.diff_search_query.is_some() {
                state.diff_search_query = None;
                state.diff_search_matches.clear();
                state.diff_search_current = 0;
                return;
            }
        }
        (KeyCode::Tab, KeyModifiers::NONE | KeyModifiers::SHIFT) => {
            // Only toggle between visible panes.
            if state.show_file_list && state.show_diff_pane {
                state.pane_focus = match state.pane_focus {
                    PaneFocus::FileList => PaneFocus::Diff,
                    PaneFocus::Diff => PaneFocus::FileList,
                };
            }
            state.status_message = None;
            return;
        }
        (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
            if dispatch_core_input(state, key) {
                return;
            }
            navigate_file(state, Direction::Next);
            state.status_message = None;
            return;
        }
        (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
            if dispatch_core_input(state, key) {
                return;
            }
            navigate_file(state, Direction::Prev);
            state.status_message = None;
            return;
        }
        (KeyCode::Char('1'), KeyModifiers::NONE) => {
            toggle_pane_visibility(state, PaneFocus::FileList);
            state.status_message = None;
            return;
        }
        (KeyCode::Char('2'), KeyModifiers::NONE) => {
            toggle_pane_visibility(state, PaneFocus::Diff);
            state.status_message = None;
            return;
        }
        (KeyCode::Char(']'), KeyModifiers::NONE) => {
            if dispatch_core_input(state, key) {
                return;
            }
            // Next hunk.
            jump_to_next_hunk(state);
            state.status_message = None;
            return;
        }
        (KeyCode::Char('['), KeyModifiers::NONE) => {
            if dispatch_core_input(state, key) {
                return;
            }
            // Previous hunk.
            jump_to_prev_hunk(state);
            state.status_message = None;
            return;
        }
        (KeyCode::Char('i'), KeyModifiers::NONE) => {
            // Toggle inline / side-by-side in diff mode.
            if state.content_mode == ContentMode::Diff {
                state.render_variant = match state.render_variant {
                    RenderVariant::Inline => RenderVariant::SideBySide,
                    RenderVariant::SideBySide => RenderVariant::Inline,
                    other => other,
                };
            }
            state.status_message = None;
            return;
        }
        (KeyCode::Char('s'), KeyModifiers::NONE) => {
            // Cycle: Diff → Head → Base → Diff.
            state.status_message = None;
            cycle_view_mode(state);
            return;
        }
        (KeyCode::Char('r'), KeyModifiers::NONE) => {
            if dispatch_core_input(state, key) {
                return;
            }
            toggle_review(state);
            state.status_message = None;
            return;
        }

        (KeyCode::Char('d'), KeyModifiers::NONE) => {
            state.diff_algorithm = state.diff_algorithm.next();
            state.reload_current_diff();
            state.status_message = Some((
                format!("Diff algorithm: {}", state.diff_algorithm.label()),
                Instant::now(),
            ));
            // Persist to config.
            if let Some(path) = &state.config_path {
                let layout = crate::config::LayoutConfig {
                    file_list_width: state.file_list_width,
                    diff_algorithm: Some(state.diff_algorithm),
                };
                crate::config::save_layout(path, &layout);
            }
            return;
        }

        (KeyCode::Char('m'), KeyModifiers::NONE) => {
            // Toggle diff base between merge base and reviewed commit.
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
                state.diff_cache = None;
                let label = if state.show_merge_base {
                    "Diff base: merge base"
                } else {
                    "Diff base: since review"
                };
                state.status_message = Some((label.to_string(), Instant::now()));
            } else {
                state.status_message = Some(("File not yet reviewed".to_string(), Instant::now()));
            }
            return;
        }

        (KeyCode::Char(']'), KeyModifiers::CONTROL) => {
            if dispatch_core_input(state, key) {
                return;
            }
            // Go-to-definition: extract word under cursor and request definition.
            request_go_to_definition(state);
            return;
        }
        (KeyCode::Char('t'), KeyModifiers::CONTROL) => {
            if dispatch_core_input(state, key) {
                return;
            }
            // Pop the jump stack — return to previous location.
            pop_jump_stack(state);
            return;
        }
        _ => {}
    }

    // Any other key clears transient status messages.
    state.status_message = None;

    // --- Diff cursor and scrolling: always controls the diff pane regardless of focus ---
    match (key.code, key.modifiers) {
        (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
            state.diff_line_cursor = state.diff_line_cursor.saturating_add(1);
            state.diff_col_cursor = 0;
            state.clamp_cursor_and_scroll();
            return;
        }
        (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
            state.diff_line_cursor = state.diff_line_cursor.saturating_sub(1);
            state.diff_col_cursor = 0;
            state.clamp_cursor_and_scroll();
            return;
        }
        (KeyCode::Char(' '), _) => {
            let delta = state.diff_view_height;
            state.diff_line_cursor = state.diff_line_cursor.saturating_add(delta);
            state.diff_scroll = state.diff_scroll.saturating_add(delta);
            state.diff_col_cursor = 0;
            state.clamp_cursor_and_scroll();
            return;
        }
        (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
            let delta = state.diff_view_height;
            state.diff_line_cursor = state.diff_line_cursor.saturating_sub(delta);
            state.diff_scroll = state.diff_scroll.saturating_sub(delta);
            state.diff_col_cursor = 0;
            state.clamp_cursor_and_scroll();
            return;
        }
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
            let delta = state.diff_view_height / 2;
            state.diff_line_cursor = state.diff_line_cursor.saturating_add(delta);
            state.diff_scroll = state.diff_scroll.saturating_add(delta);
            state.diff_col_cursor = 0;
            state.clamp_cursor_and_scroll();
            return;
        }
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
            let delta = state.diff_view_height / 2;
            state.diff_line_cursor = state.diff_line_cursor.saturating_sub(delta);
            state.diff_scroll = state.diff_scroll.saturating_sub(delta);
            state.diff_col_cursor = 0;
            state.clamp_cursor_and_scroll();
            return;
        }
        (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
            // Scroll viewport down one line, keeping cursor on screen.
            state.diff_scroll = state.diff_scroll.saturating_add(1);
            state.clamp_diff_scroll();
            if state.diff_line_cursor < state.diff_scroll {
                state.diff_line_cursor = state.diff_scroll;
                state.diff_col_cursor = 0;
            }
            return;
        }
        (KeyCode::Char('y'), KeyModifiers::CONTROL) => {
            // Scroll viewport up one line, keeping cursor on screen.
            state.diff_scroll = state.diff_scroll.saturating_sub(1);
            if state.diff_view_height > 0
                && state.diff_line_cursor >= state.diff_scroll + state.diff_view_height
            {
                state.diff_line_cursor = state.diff_scroll + state.diff_view_height - 1;
                state.diff_col_cursor = 0;
            }
            return;
        }
        (KeyCode::Char('g'), KeyModifiers::NONE) => {
            state.diff_line_cursor = 0;
            state.diff_scroll = 0;
            state.diff_col_cursor = 0;
            return;
        }
        (KeyCode::Char('G'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
            state.diff_line_cursor = state.max_diff_scroll();
            state.diff_col_cursor = 0;
            state.clamp_cursor_and_scroll();
            return;
        }
        (KeyCode::Char('H'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
            // Move cursor to top of visible viewport.
            state.diff_line_cursor = state.diff_scroll;
            state.diff_col_cursor = 0;
            return;
        }
        (KeyCode::Char('M'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
            // Move cursor to middle of visible viewport.
            let mid = state.diff_view_height / 2;
            state.diff_line_cursor = state.diff_scroll + mid;
            state.diff_col_cursor = 0;
            state.clamp_cursor_and_scroll();
            return;
        }
        (KeyCode::Char('L'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
            // Move cursor to bottom of visible viewport.
            let bottom = state.diff_view_height.saturating_sub(1);
            state.diff_line_cursor = state.diff_scroll + bottom;
            state.diff_col_cursor = 0;
            state.clamp_cursor_and_scroll();
            return;
        }
        // --- Horizontal cursor movement ---
        (KeyCode::Char('h') | KeyCode::Left, KeyModifiers::NONE) => {
            state.diff_col_cursor = state.diff_col_cursor.saturating_sub(1);
            return;
        }
        (KeyCode::Char('l') | KeyCode::Right, KeyModifiers::NONE) => {
            let max = state.current_line_text_len().saturating_sub(1);
            if state.diff_col_cursor < max {
                state.diff_col_cursor += 1;
            }
            return;
        }
        (KeyCode::Char('0'), KeyModifiers::NONE) => {
            state.diff_col_cursor = 0;
            return;
        }
        (KeyCode::Char('$'), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
            state.diff_col_cursor = state.current_line_text_len().saturating_sub(1);
            return;
        }
        (KeyCode::Char('w'), KeyModifiers::NONE) => {
            word_forward(state);
            return;
        }
        (KeyCode::Char('b'), KeyModifiers::NONE) => {
            word_backward(state);
            return;
        }
        (KeyCode::Char('W'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
            bigword_forward(state);
            return;
        }
        (KeyCode::Char('B'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
            bigword_backward(state);
            return;
        }
        _ => {}
    }

    // --- Pane-specific keys ---
    match state.pane_focus {
        PaneFocus::FileList => handle_file_list_key(state, key),
        PaneFocus::Diff => handle_diff_key(state, key),
    }
}

/// Toggle visibility of a pane. At least one pane must remain visible.
fn toggle_pane_visibility(state: &mut AppState, pane: PaneFocus) {
    match pane {
        PaneFocus::FileList => {
            if state.show_file_list {
                // Only hide if the other pane is visible.
                if state.show_diff_pane {
                    state.show_file_list = false;
                    state.pane_focus = PaneFocus::Diff;
                }
            } else {
                state.show_file_list = true;
            }
        }
        PaneFocus::Diff => {
            if state.show_diff_pane {
                if state.show_file_list {
                    state.show_diff_pane = false;
                    state.pane_focus = PaneFocus::FileList;
                }
            } else {
                state.show_diff_pane = true;
            }
        }
    }
}

/// Reset diff-related state when the selected file changes.
fn on_file_changed(state: &mut AppState) {
    state.on_file_changed();
}

/// Cycle view mode: Diff → Head → Base → Diff.
/// Preserves approximate scroll position by finding the closest line
/// in the target view that corresponds to the current position.
fn cycle_view_mode(state: &mut AppState) {
    // Determine which new-file line is currently at the top of the viewport.
    // In diff mode, a display row maps to a new-file line via the hunk data.
    // For simplicity, use the approximate relationship: for gaps between
    // hunks, display_row ≈ new_lineno. For hunk regions, the line number
    // from the diff content is used.
    let approx_line = estimate_current_line(state);

    match (&state.content_mode, &state.render_variant) {
        (ContentMode::Diff, _) => {
            state.content_mode = ContentMode::FullFile;
            state.render_variant = RenderVariant::HeadVersion;
            // In HEAD view, display row ≈ lineno - 1.
            let row = approx_line.saturating_sub(1);
            state.diff_line_cursor = row;
            state.diff_scroll = row;
        }
        (ContentMode::FullFile, RenderVariant::HeadVersion) => {
            if state.base_content.is_none() {
                state.load_base_content();
            }
            state.render_variant = RenderVariant::BaseVersion;
            // In base view, the line numbers differ from HEAD due to
            // additions/deletions. Use the old-file line that corresponds
            // to the current new-file line via the diff mapping.
            let base_line =
                navigation::map_new_to_old_line(state.selected_file_entry(), approx_line);
            let row = base_line.saturating_sub(1);
            state.diff_line_cursor = row;
            state.diff_scroll = row;
        }
        (ContentMode::FullFile, RenderVariant::BaseVersion) => {
            state.content_mode = ContentMode::Diff;
            state.render_variant = RenderVariant::Inline;
            // Map old-file line back to approximate display row in diff view.
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

/// Estimate the new-file line number at the current cursor position.
fn estimate_current_line(state: &AppState) -> usize {
    match state.content_mode {
        ContentMode::Diff => {
            // In inline diff mode, display rows include deletion lines
            // (which don't have new-file line numbers). The approximate
            // new-file line is cursor position adjusted for deletions
            // in hunks above the cursor position.
            let scroll = state.diff_line_cursor;
            if let Some(entry) = state.selected_file_entry() {
                let mut deletions_above = 0usize;
                for (i, &start) in state.hunk_start_rows.iter().enumerate() {
                    let end = state.hunk_end_rows.get(i).copied().unwrap_or(start);
                    if start > scroll {
                        break;
                    }
                    // Count deletion lines in this hunk that are above scroll.
                    let hunk = &entry.diff.hunks[i.min(entry.diff.hunks.len() - 1)];
                    let hunk_scroll_end = scroll.min(end);
                    for (j, line) in hunk.lines.iter().enumerate() {
                        if start + j > hunk_scroll_end {
                            break;
                        }
                        if line.kind == crate::model::LineKind::Deletion && start + j <= scroll {
                            deletions_above += 1;
                        }
                    }
                }
                scroll.saturating_sub(deletions_above) + 1
            } else {
                scroll + 1
            }
        }
        ContentMode::FullFile => {
            // In full-file mode, display row = lineno - 1.
            state.diff_line_cursor + 1
        }
    }
}

/// Jump to the next hunk — moves the cursor to the first changed line,
/// then scrolls to show as much of the hunk as possible.
fn jump_to_next_hunk(state: &mut AppState) {
    if let Some(jump) = navigation::jump_to_next_hunk(
        state.diff_line_cursor,
        state.diff_scroll,
        state.diff_view_height,
        &state.hunk_first_change_rows,
        &state.hunk_end_rows,
    ) {
        state.diff_line_cursor = jump.cursor;
        state.diff_scroll = jump.scroll;
        state.diff_col_cursor = 0;
        state.clamp_cursor_and_scroll();
    }
}

/// Jump to the previous hunk — moves the cursor to the first changed line,
/// then scrolls to show as much of the hunk as possible.
fn jump_to_prev_hunk(state: &mut AppState) {
    if let Some(jump) = navigation::jump_to_prev_hunk(
        state.diff_line_cursor,
        state.diff_scroll,
        state.diff_view_height,
        &state.hunk_first_change_rows,
        &state.hunk_end_rows,
    ) {
        state.diff_line_cursor = jump.cursor;
        state.diff_scroll = jump.scroll;
        state.diff_col_cursor = 0;
        state.clamp_cursor_and_scroll();
    }
}

// ---------------------------------------------------------------------------
// Word motion helpers (vim-style w/b)
// ---------------------------------------------------------------------------

/// Classify a character for word boundary detection.
fn char_class(c: char) -> u8 {
    if c.is_alphanumeric() || c == '_' {
        0 // word
    } else if c.is_whitespace() {
        1 // whitespace
    } else {
        2 // punctuation / symbol
    }
}

/// Move the column cursor to the start of the next word (vim `w`).
/// Wraps to the next line if at end of current line's text.
fn word_forward(state: &mut AppState) {
    let content = state.line_content_trimmed(state.diff_line_cursor);
    let chars: Vec<char> = content.chars().collect();
    let text_len = chars.len();

    // At or past end of text content — wrap to next line.
    if text_len == 0 || state.diff_col_cursor >= text_len.saturating_sub(1) {
        let max_line = state.diff_content_height.saturating_sub(1);
        if state.diff_line_cursor < max_line {
            state.diff_line_cursor += 1;
            state.diff_col_cursor = 0;
            state.clamp_cursor_and_scroll();
            // Skip to first non-whitespace on the new line.
            let new_content = state.line_content_trimmed(state.diff_line_cursor);
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
    // Skip current word class.
    while pos < text_len && char_class(chars[pos]) == start_class {
        pos += 1;
    }
    // Skip whitespace.
    while pos < text_len && chars[pos].is_whitespace() {
        pos += 1;
    }
    // If we ran past the end of text, wrap to next line.
    if pos >= text_len {
        let max_line = state.diff_content_height.saturating_sub(1);
        if state.diff_line_cursor < max_line {
            state.diff_line_cursor += 1;
            state.diff_col_cursor = 0;
            state.clamp_cursor_and_scroll();
            let new_content = state.line_content_trimmed(state.diff_line_cursor);
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

/// Move the column cursor to the start of the previous word (vim `b`).
/// Wraps to the previous line if at start of current line.
fn word_backward(state: &mut AppState) {
    // At start of line — wrap to previous line's last word.
    if state.diff_col_cursor == 0 {
        if state.diff_line_cursor > 0 {
            state.diff_line_cursor -= 1;
            state.clamp_cursor_and_scroll();
            let content = state.line_content_trimmed(state.diff_line_cursor);
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

    let content = state.line_content_trimmed(state.diff_line_cursor);
    let chars: Vec<char> = content.chars().collect();
    if chars.is_empty() {
        return;
    }
    let mut pos = state.diff_col_cursor;
    // Step back one char.
    pos -= 1;
    // Skip whitespace.
    while pos > 0 && chars[pos].is_whitespace() {
        pos -= 1;
    }
    // Find the start of the current word class.
    let target_class = char_class(chars[pos]);
    while pos > 0 && char_class(chars[pos - 1]) == target_class {
        pos -= 1;
    }
    state.diff_col_cursor = pos;
}

/// Move the column cursor to the start of the next WORD (vim `W`).
/// WORDs are separated by whitespace only — punctuation is not a boundary.
/// Wraps to the next line if at end of current line's text.
fn bigword_forward(state: &mut AppState) {
    let content = state.line_content_trimmed(state.diff_line_cursor);
    let chars: Vec<char> = content.chars().collect();
    let text_len = chars.len();

    if text_len == 0 || state.diff_col_cursor >= text_len.saturating_sub(1) {
        // Wrap to next line.
        let max_line = state.diff_content_height.saturating_sub(1);
        if state.diff_line_cursor < max_line {
            state.diff_line_cursor += 1;
            state.diff_col_cursor = 0;
            state.clamp_cursor_and_scroll();
            let new_content = state.line_content_trimmed(state.diff_line_cursor);
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
    // Skip non-whitespace.
    while pos < text_len && !chars[pos].is_whitespace() {
        pos += 1;
    }
    // Skip whitespace.
    while pos < text_len && chars[pos].is_whitespace() {
        pos += 1;
    }
    if pos >= text_len {
        // Wrap to next line.
        let max_line = state.diff_content_height.saturating_sub(1);
        if state.diff_line_cursor < max_line {
            state.diff_line_cursor += 1;
            state.diff_col_cursor = 0;
            state.clamp_cursor_and_scroll();
            let new_content = state.line_content_trimmed(state.diff_line_cursor);
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

/// Move the column cursor to the start of the previous WORD (vim `B`).
/// WORDs are separated by whitespace only. Wraps to the previous line.
fn bigword_backward(state: &mut AppState) {
    if state.diff_col_cursor == 0 {
        if state.diff_line_cursor > 0 {
            state.diff_line_cursor -= 1;
            state.clamp_cursor_and_scroll();
            let content = state.line_content_trimmed(state.diff_line_cursor);
            let chars: Vec<char> = content.chars().collect();
            if chars.is_empty() {
                state.diff_col_cursor = 0;
            } else {
                let mut pos = chars.len() - 1;
                // Skip trailing whitespace.
                while pos > 0 && chars[pos].is_whitespace() {
                    pos -= 1;
                }
                // Find start of WORD (skip non-whitespace backward).
                while pos > 0 && !chars[pos - 1].is_whitespace() {
                    pos -= 1;
                }
                state.diff_col_cursor = pos;
            }
        }
        return;
    }

    let content = state.line_content_trimmed(state.diff_line_cursor);
    let chars: Vec<char> = content.chars().collect();
    if chars.is_empty() {
        return;
    }
    let mut pos = state.diff_col_cursor - 1;
    // Skip whitespace.
    while pos > 0 && chars[pos].is_whitespace() {
        pos -= 1;
    }
    // Find start of WORD (skip non-whitespace backward).
    while pos > 0 && !chars[pos - 1].is_whitespace() {
        pos -= 1;
    }
    state.diff_col_cursor = pos;
}

/// Navigate to the next/previous file, scoped by section.
///
/// - In the diff pane: cycles only through unreviewed files.
/// - In the file list: cycles within the current section (unreviewed
///   or reviewed) based on where the cursor currently is.
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

/// Toggle review status of the currently selected file.
/// Sets a flag for the async event loop to process.
fn toggle_review(state: &mut AppState) {
    if state.files.get(state.selected_file).is_some() {
        state.pending_review_toggle = true;
    }
}

/// File list pane keys.
fn handle_file_list_key(state: &mut AppState, key: CrosstermKeyEvent) {
    match key.code {
        KeyCode::Enter => {
            if state.show_diff_pane {
                state.pane_focus = PaneFocus::Diff;
            }
        }
        _ => {}
    }
}

/// Diff pane keys.
fn handle_diff_key(state: &mut AppState, key: CrosstermKeyEvent) {
    match key.code {
        KeyCode::Enter => {
            // Expand reviewed file diff or no-op.
            if let Some(entry) = state.selected_file_entry() {
                if matches!(entry.status, ReviewStatus::Reviewed { .. })
                    && !state.reviewed_diff_expanded
                {
                    state.reviewed_diff_expanded = true;
                    state.diff_scroll = 0;
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent as CrosstermKeyEvent;

    #[test]
    fn translates_printable_key_to_core_input_event() {
        let event = input_event_from_key(CrosstermKeyEvent::new(
            KeyCode::Char(':'),
            KeyModifiers::SHIFT,
        ))
        .unwrap();

        assert_eq!(
            event,
            InputEvent::Key(CoreKeyEvent {
                kind: CoreKeyEventKind::Press,
                key: CoreKey::Char(':'),
                modifiers: InputModifiers {
                    shift: true,
                    ..Default::default()
                },
            })
        );
    }

    #[test]
    fn translates_control_modifier_to_core_input_event() {
        let event = input_event_from_key(CrosstermKeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        ))
        .unwrap();

        assert_eq!(
            event,
            InputEvent::Key(CoreKeyEvent {
                kind: CoreKeyEventKind::Press,
                key: CoreKey::Char('c'),
                modifiers: InputModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            })
        );
    }

    #[test]
    fn ignores_crossterm_keys_without_core_equivalent() {
        assert!(
            input_event_from_key(CrosstermKeyEvent::new(KeyCode::Null, KeyModifiers::NONE,))
                .is_none()
        );
    }
}
