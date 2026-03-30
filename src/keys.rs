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
        (KeyCode::Tab, _) => {
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
            // Next file.
            if !state.files.is_empty() {
                state.selected_file = (state.selected_file + 1) % state.files.len();
                on_file_changed(state);
            }
            state.status_message = None;
            return;
        }
        (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
            // Previous file.
            if !state.files.is_empty() {
                state.selected_file = if state.selected_file == 0 {
                    state.files.len() - 1
                } else {
                    state.selected_file - 1
                };
                on_file_changed(state);
            }
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
        (KeyCode::Char('i'), KeyModifiers::CONTROL) => {
            // Next hunk.
            jump_to_next_hunk(state);
            state.status_message = None;
            return;
        }
        (KeyCode::Char('o'), KeyModifiers::CONTROL) => {
            // Previous hunk.
            jump_to_prev_hunk(state);
            state.status_message = None;
            return;
        }
        (KeyCode::Char('s'), KeyModifiers::NONE) => {
            // Cycle: Diff → Head → Base → Diff.
            state.status_message = None;
            cycle_view_mode(state);
            return;
        }
        _ => {}
    }

    // Any other key clears transient status messages.
    state.status_message = None;

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
    state.reviewed_diff_expanded = false;
    state.hunk_start_rows.clear();
    state.hunk_end_rows.clear();
    state.load_head_content();

    // Reload base content if we're currently in base view.
    if state.content_mode == ContentMode::FullFile
        && state.render_variant == RenderVariant::BaseVersion
    {
        state.load_base_content();
    } else {
        state.base_content = None;
    }

    // Scroll to the first hunk (approximate: new_start - 1 unchanged lines before it).
    state.diff_scroll = state
        .selected_file_entry()
        .and_then(|e| e.diff.hunks.first())
        .map(|h| (h.new_start as usize).saturating_sub(1))
        .unwrap_or(0);
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

/// File list pane keys.
fn handle_file_list_key(state: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if !state.files.is_empty() {
                let new = (state.selected_file + 1).min(state.files.len() - 1);
                if new != state.selected_file {
                    state.selected_file = new;
                    on_file_changed(state);
                }
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            let new = state.selected_file.saturating_sub(1);
            if new != state.selected_file {
                state.selected_file = new;
                on_file_changed(state);
            }
        }
        KeyCode::Char('g') => {
            if state.selected_file != 0 {
                state.selected_file = 0;
                on_file_changed(state);
            }
        }
        KeyCode::Char('G') => {
            if !state.files.is_empty() {
                let new = state.files.len() - 1;
                if new != state.selected_file {
                    state.selected_file = new;
                    on_file_changed(state);
                }
            }
        }
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
    match (key.code, key.modifiers) {
        (KeyCode::Enter, _) => {
            // Expand reviewed file diff or no-op.
            if let Some(entry) = state.selected_file_entry() {
                if matches!(entry.status, ReviewStatus::Reviewed { .. })
                    && !state.reviewed_diff_expanded
                {
                    state.reviewed_diff_expanded = true;
                    state.diff_scroll = 0;
                }
            }
            return;
        }
        (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
            state.diff_scroll = state.diff_scroll.saturating_add(1);
            state.clamp_diff_scroll();
        }
        (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
            state.diff_scroll = state.diff_scroll.saturating_sub(1);
        }
        (KeyCode::Char(' '), _) => {
            // Page down.
            state.diff_scroll = state.diff_scroll.saturating_add(state.diff_view_height);
            state.clamp_diff_scroll();
        }
        (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
            // Ctrl-b: page up (fallback for terminals that don't send Shift-Space).
            state.diff_scroll = state.diff_scroll.saturating_sub(state.diff_view_height);
        }
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
            // Half page down.
            state.diff_scroll = state.diff_scroll.saturating_add(state.diff_view_height / 2);
            state.clamp_diff_scroll();
        }
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
            // Half page up.
            state.diff_scroll = state.diff_scroll.saturating_sub(state.diff_view_height / 2);
        }
        (KeyCode::Char('g'), KeyModifiers::NONE) => {
            state.diff_scroll = 0;
        }
        (KeyCode::Char('G'), KeyModifiers::SHIFT | KeyModifiers::NONE) => {
            state.diff_scroll = state.max_diff_scroll();
        }
        _ => {}
    }
}
