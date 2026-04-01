//! Input handling and key dispatch.
//!
//! Key events are dispatched based on the current pane focus. Global keys
//! (like `q` to quit, `Tab` to switch focus) work from any pane.
//! Pane-specific keys are handled by dedicated functions.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::AppState;
use crate::model::{ContentMode, PaneFocus, RenderVariant, ReviewStatus};

/// How long the "Press Ctrl-C again" prompt stays active.
const CTRL_C_TIMEOUT: Duration = Duration::from_secs(3);

/// Handle a key press event by mutating the application state.
pub fn handle_key_event(state: &mut AppState, key: KeyEvent) {
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
        _ => {}
    }

    // Any other key clears transient status messages.
    state.status_message = None;

    // --- Diff scrolling: always controls the diff pane regardless of focus ---
    match (key.code, key.modifiers) {
        (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
            state.diff_scroll = state.diff_scroll.saturating_add(1);
            state.clamp_diff_scroll();
            return;
        }
        (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
            state.diff_scroll = state.diff_scroll.saturating_sub(1);
            return;
        }
        (KeyCode::Char(' '), _) => {
            state.diff_scroll = state.diff_scroll.saturating_add(state.diff_view_height);
            state.clamp_diff_scroll();
            return;
        }
        (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
            state.diff_scroll = state.diff_scroll.saturating_sub(state.diff_view_height);
            return;
        }
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
            state.diff_scroll = state.diff_scroll.saturating_add(state.diff_view_height / 2);
            state.clamp_diff_scroll();
            return;
        }
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
            state.diff_scroll = state.diff_scroll.saturating_sub(state.diff_view_height / 2);
            return;
        }
        (KeyCode::Char('g'), KeyModifiers::NONE) => {
            state.diff_scroll = 0;
            return;
        }
        (KeyCode::Char('G'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
            state.diff_scroll = state.max_diff_scroll();
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
            state.diff_scroll = approx_line.saturating_sub(1);
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
            state.diff_scroll = base_line.saturating_sub(1);
        }
        (ContentMode::FullFile, RenderVariant::BaseVersion) => {
            state.content_mode = ContentMode::Diff;
            state.render_variant = RenderVariant::Inline;
            // Map old-file line back to approximate display row in diff view.
            let old_line = state.diff_scroll + 1;
            let new_line = map_old_to_new_line(state, old_line);
            state.diff_scroll = new_line.saturating_sub(1);
        }
        _ => {
            state.content_mode = ContentMode::Diff;
            state.render_variant = RenderVariant::Inline;
        }
    }
}

/// Estimate the new-file line number at the current scroll position.
fn estimate_current_line(state: &AppState) -> usize {
    match state.content_mode {
        ContentMode::Diff => {
            // In inline diff mode, display rows include deletion lines
            // (which don't have new-file line numbers). The approximate
            // new-file line is scroll position adjusted for deletions
            // in hunks above the scroll position.
            let scroll = state.diff_scroll;
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
            state.diff_scroll + 1
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

/// Jump to the next hunk (Ctrl-i).
fn jump_to_next_hunk(state: &mut AppState) {
    let current = state.diff_scroll;
    if let Some(&row) = state.hunk_start_rows.iter().find(|&&r| r > current) {
        state.diff_scroll = row;
        state.clamp_diff_scroll();
    }
}

/// Jump to the previous hunk (Ctrl-o).
fn jump_to_prev_hunk(state: &mut AppState) {
    let current = state.diff_scroll;
    if let Some(&row) = state.hunk_start_rows.iter().rev().find(|&&r| r < current) {
        state.diff_scroll = row;
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
