//! Input handling and key dispatch.
//!
//! Key events are dispatched based on the current pane focus. Global keys
//! (like `q` to quit, `Tab` to switch focus) work from any pane.
//! Pane-specific keys are handled by dedicated functions.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{AppState, InputMode};
use crate::model::{ContentMode, PaneFocus, RenderVariant, ReviewStatus};

/// How long the "Press Ctrl-C again" prompt stays active.
const CTRL_C_TIMEOUT: Duration = Duration::from_secs(3);

/// Handle a key press event by mutating the application state.
pub fn handle_key_event(state: &mut AppState, key: KeyEvent) {
    // --- Command mode input ---
    if state.input_mode == InputMode::Command {
        handle_command_input(state, key);
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

    // --- Ctrl-W prefix combos (vim-style window navigation) ---
    if state.pending_ctrl_w {
        state.pending_ctrl_w = false;
        state.status_message = None;
        match key.code {
            KeyCode::Char('h') => {
                if state.show_file_list {
                    state.pane_focus = PaneFocus::FileList;
                }
            }
            KeyCode::Char('l') => {
                if state.show_diff_pane {
                    state.pane_focus = PaneFocus::Diff;
                }
            }
            _ => {}
        }
        return;
    }

    // ---------------------------------------------------------------------------
    // Command mode
    // ---------------------------------------------------------------------------

    /// Handle keystrokes while in command mode (`:` prompt active).
    fn handle_command_input(state: &mut AppState, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                state.input_mode = InputMode::Normal;
                state.command_input.clear();
                state.command_cursor = 0;
            }
            KeyCode::Enter => {
                let cmd = state.command_input.clone();
                state.input_mode = InputMode::Normal;
                state.command_input.clear();
                state.command_cursor = 0;
                execute_command(state, &cmd);
            }
            KeyCode::Backspace => {
                if state.command_cursor > 0 {
                    state.command_cursor -= 1;
                    state.command_input.remove(state.command_cursor);
                } else {
                    // Backspace on empty input exits command mode.
                    state.input_mode = InputMode::Normal;
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

    /// Parse and execute a command string (from the `:` prompt).
    fn execute_command(state: &mut AppState, cmd: &str) {
        let cmd = cmd.trim();
        if cmd.is_empty() {
            return;
        }

        // Split into command name and arguments.
        let (name, args) = match cmd.split_once(char::is_whitespace) {
            Some((n, a)) => (n, a.trim()),
            None => (cmd, ""),
        };

        match name {
            "q" | "quit" => {
                state.should_quit = true;
            }
            "gr" => {
                if args.is_empty() {
                    state.status_message = Some(("Usage: :gr <regex>".to_string(), Instant::now()));
                } else {
                    // Set pending command for the async event loop to process.
                    state.pending_command = Some(format!("gr {args}"));
                }
            }
            "grd" => {
                if args.is_empty() {
                    state.status_message =
                        Some(("Usage: :grd <regex>".to_string(), Instant::now()));
                } else {
                    state.pending_command = Some(format!("grd {args}"));
                }
            }
            "gd" => {
                if args.is_empty() {
                    // No argument: use word under cursor.
                    let word = extract_word_at_cursor(state);
                    match word {
                        Some(w) if !w.is_empty() => {
                            state.pending_command = Some(format!("find_definition {w}"));
                        }
                        _ => {
                            state.status_message =
                                Some(("Usage: :gd <symbol>".to_string(), Instant::now()));
                        }
                    }
                } else {
                    state.pending_command = Some(format!("find_definition {args}"));
                }
            }
            _ => {
                state.status_message = Some((format!("Unknown command: {name}"), Instant::now()));
            }
        }
    }

    // ---------------------------------------------------------------------------
    // Search results overlay
    // ---------------------------------------------------------------------------

    /// Handle keystrokes when the search results overlay is visible.
    fn handle_search_results_key(state: &mut AppState, key: KeyEvent) {
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
    fn handle_definition_results_key(state: &mut AppState, key: KeyEvent) {
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
        // Try to find the file in the diff file list.
        if let Some(idx) = state
            .files
            .iter()
            .position(|f| f.change.path == m.file_path)
        {
            push_jump_stack(state);
            state.selected_file = idx;
            on_file_changed(state);
            // Place cursor on the matching line.
            state.diff_line_cursor = (m.line_number as usize).saturating_sub(1);
            state.clamp_cursor_and_scroll();
        } else {
            // File not in diff — show it read-only via pending command.
            push_jump_stack(state);
            state.pending_command = Some(format!("view_file {} {}", m.file_path, m.line_number));
        }
    }

    /// Navigate to a definition location.
    fn navigate_to_definition(state: &mut AppState, def: &crate::model::DefinitionLocation) {
        if let Some(idx) = state
            .files
            .iter()
            .position(|f| f.change.path == def.file_path)
        {
            push_jump_stack(state);
            state.selected_file = idx;
            on_file_changed(state);
            state.diff_line_cursor = (def.line_number as usize).saturating_sub(1);
            state.clamp_cursor_and_scroll();
        } else {
            push_jump_stack(state);
            state.pending_command =
                Some(format!("view_file {} {}", def.file_path, def.line_number));
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
    /// Sets `pending_command` for the async event loop.
    fn request_go_to_definition(state: &mut AppState) {
        // Extract the word at the approximate cursor position in the diff view.
        // The "cursor" is at diff_scroll line, first non-whitespace token after
        // the gutter. For simplicity, extract from the first content token of
        // the current scroll line.
        let word = extract_word_at_cursor(state);
        if let Some(word) = word {
            if word.is_empty() {
                state.status_message = Some(("No word under cursor".to_string(), Instant::now()));
            } else {
                state.pending_command = Some(format!("find_definition {word}"));
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

        // Extract the first identifier: sequence of alphanumeric + underscore.
        let start = content.find(|c: char| c.is_alphanumeric() || c == '_')?;
        let word: String = content[start..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if word.is_empty() {
            None
        } else {
            Some(word)
        }
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
        (KeyCode::Char('w'), KeyModifiers::CONTROL) => {
            state.pending_ctrl_w = true;
            state.status_message = Some(("Ctrl-W ...".to_string(), Instant::now()));
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
            state.input_mode = InputMode::Command;
            state.command_input.clear();
            state.command_cursor = 0;
            return;
        }
        // `/` is reserved for file-local diff search (Stage 12).
        // Not yet implemented — ignore for now.
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
            navigate_file(state, Direction::Next);
            state.status_message = None;
            return;
        }
        (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
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
            // Next hunk.
            jump_to_next_hunk(state);
            state.status_message = None;
            return;
        }
        (KeyCode::Char('['), KeyModifiers::NONE) => {
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
            toggle_review(state);
            state.status_message = None;
            return;
        }
        (KeyCode::Char('b'), KeyModifiers::NONE) => {
            state.show_blame = !state.show_blame;
            state.load_blame();
            let label = if state.show_blame {
                "Blame: shown"
            } else {
                "Blame: hidden"
            };
            state.status_message = Some((label.to_string(), Instant::now()));
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
        (KeyCode::Char('w'), KeyModifiers::NONE) => {
            state.ignore_whitespace = !state.ignore_whitespace;
            state.reload_current_diff();
            let label = if state.ignore_whitespace {
                "Whitespace: ignored"
            } else {
                "Whitespace: shown"
            };
            state.status_message = Some((label.to_string(), Instant::now()));
            return;
        }
        (KeyCode::Char(']'), KeyModifiers::CONTROL) => {
            // Go-to-definition: extract word under cursor and request definition.
            request_go_to_definition(state);
            return;
        }
        (KeyCode::Char('t'), KeyModifiers::CONTROL) => {
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
            state.clamp_cursor_and_scroll();
            return;
        }
        (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
            state.diff_line_cursor = state.diff_line_cursor.saturating_sub(1);
            state.clamp_cursor_and_scroll();
            return;
        }
        (KeyCode::Char(' '), _) => {
            let delta = state.diff_view_height;
            state.diff_line_cursor = state.diff_line_cursor.saturating_add(delta);
            state.diff_scroll = state.diff_scroll.saturating_add(delta);
            state.clamp_cursor_and_scroll();
            return;
        }
        (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
            let delta = state.diff_view_height;
            state.diff_line_cursor = state.diff_line_cursor.saturating_sub(delta);
            state.diff_scroll = state.diff_scroll.saturating_sub(delta);
            state.clamp_cursor_and_scroll();
            return;
        }
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
            let delta = state.diff_view_height / 2;
            state.diff_line_cursor = state.diff_line_cursor.saturating_add(delta);
            state.diff_scroll = state.diff_scroll.saturating_add(delta);
            state.clamp_cursor_and_scroll();
            return;
        }
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
            let delta = state.diff_view_height / 2;
            state.diff_line_cursor = state.diff_line_cursor.saturating_sub(delta);
            state.diff_scroll = state.diff_scroll.saturating_sub(delta);
            state.clamp_cursor_and_scroll();
            return;
        }
        (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
            // Scroll viewport down one line, keeping cursor on screen.
            state.diff_scroll = state.diff_scroll.saturating_add(1);
            state.clamp_diff_scroll();
            if state.diff_line_cursor < state.diff_scroll {
                state.diff_line_cursor = state.diff_scroll;
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
            }
            return;
        }
        (KeyCode::Char('g'), KeyModifiers::NONE) => {
            state.diff_line_cursor = 0;
            state.diff_scroll = 0;
            return;
        }
        (KeyCode::Char('G'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
            state.diff_line_cursor = state.max_diff_scroll();
            state.diff_scroll = state.max_diff_scroll();
            return;
        }
        (KeyCode::Char('H'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
            // Move cursor to top of visible viewport.
            state.diff_line_cursor = state.diff_scroll;
            return;
        }
        (KeyCode::Char('M'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
            // Move cursor to middle of visible viewport.
            let mid = state.diff_view_height / 2;
            state.diff_line_cursor = state.diff_scroll + mid;
            state.clamp_cursor_and_scroll();
            return;
        }
        (KeyCode::Char('L'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
            // Move cursor to bottom of visible viewport.
            let bottom = state.diff_view_height.saturating_sub(1);
            state.diff_line_cursor = state.diff_scroll + bottom;
            state.clamp_cursor_and_scroll();
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
            let base_line = map_new_to_old_line(state, approx_line);
            let row = base_line.saturating_sub(1);
            state.diff_line_cursor = row;
            state.diff_scroll = row;
        }
        (ContentMode::FullFile, RenderVariant::BaseVersion) => {
            state.content_mode = ContentMode::Diff;
            state.render_variant = RenderVariant::Inline;
            // Map old-file line back to approximate display row in diff view.
            let old_line = state.diff_line_cursor + 1;
            let new_line = map_old_to_new_line(state, old_line);
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

/// Map a new-file line number to the corresponding old-file line number
/// using the diff hunk data.
fn map_new_to_old_line(state: &AppState, new_line: usize) -> usize {
    let entry = match state.selected_file_entry() {
        Some(e) => e,
        None => return new_line,
    };
    // Walk through hunks to compute the offset between old and new line numbers.
    let mut offset: i64 = 0; // old_line = new_line + offset
    for hunk in &entry.diff.hunks {
        if (hunk.new_start as usize) > new_line {
            break;
        }
        // Each hunk changes the offset by (old_lines - new_lines).
        offset = (hunk.old_start as i64 + hunk.old_lines as i64)
            - (hunk.new_start as i64 + hunk.new_lines as i64);
    }
    (new_line as i64 + offset).max(1) as usize
}

/// Map an old-file line number to the corresponding new-file line number.
fn map_old_to_new_line(state: &AppState, old_line: usize) -> usize {
    let entry = match state.selected_file_entry() {
        Some(e) => e,
        None => return old_line,
    };
    let mut offset: i64 = 0; // new_line = old_line + offset
    for hunk in &entry.diff.hunks {
        if (hunk.old_start as usize) > old_line {
            break;
        }
        offset = (hunk.new_start as i64 + hunk.new_lines as i64)
            - (hunk.old_start as i64 + hunk.old_lines as i64);
    }
    (old_line as i64 + offset).max(1) as usize
}

/// Jump to the next hunk — moves the cursor to the first changed line.
fn jump_to_next_hunk(state: &mut AppState) {
    let current = state.diff_line_cursor;
    if let Some(&row) = state.hunk_first_change_rows.iter().find(|&&r| r > current) {
        state.diff_line_cursor = row;
        state.clamp_cursor_and_scroll();
    }
}

/// Jump to the previous hunk — moves the cursor to the first changed line.
fn jump_to_prev_hunk(state: &mut AppState) {
    let current = state.diff_line_cursor;
    if let Some(&row) = state.hunk_first_change_rows.iter().rev().find(|&&r| r < current) {
        state.diff_line_cursor = row;
        state.clamp_cursor_and_scroll();
    }
}

enum Direction {
    Next,
    Prev,
}

/// Navigate to the next/previous file, scoped by section.
///
/// - In the diff pane: cycles only through unreviewed files.
/// - In the file list: cycles within the current section (unreviewed
///   or reviewed) based on where the cursor currently is.
fn navigate_file(state: &mut AppState, dir: Direction) {
    if state.files.is_empty() {
        return;
    }

    let unreviewed_count = state.unreviewed_count();
    let total = state.files.len();

    // Determine the range of indices to cycle within.
    let (range_start, range_end) = if state.pane_focus == PaneFocus::Diff {
        // Diff pane: always cycle unreviewed only.
        if unreviewed_count == 0 {
            (0, total)
        } else {
            (0, unreviewed_count)
        }
    } else {
        // File list: scope to whichever section the cursor is in.
        if state.selected_file < unreviewed_count {
            (0, unreviewed_count.max(1))
        } else {
            (unreviewed_count, total)
        }
    };

    let range_len = range_end - range_start;
    if range_len == 0 {
        return;
    }

    // Current position within the range.
    let pos = state.selected_file.saturating_sub(range_start);

    let new_pos = match dir {
        Direction::Next => (pos + 1) % range_len,
        Direction::Prev => {
            if pos == 0 {
                range_len - 1
            } else {
                pos - 1
            }
        }
    };

    let new_idx = range_start + new_pos;
    if new_idx != state.selected_file {
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
fn handle_file_list_key(state: &mut AppState, key: KeyEvent) {
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
fn handle_diff_key(state: &mut AppState, key: KeyEvent) {
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
