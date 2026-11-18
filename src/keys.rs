//! Input handling and key dispatch.
//!
//! Key events are dispatched based on the current pane focus. Global keys
//! (like `q` to quit, `Tab` to switch focus) work from any pane.
//! Pane-specific keys are handled by dedicated functions.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::AppState;
use crate::model::PaneFocus;

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
                state.diff_scroll = 0;
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
                state.diff_scroll = 0;
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

/// File list pane keys. Full implementation in Stage 7.
fn handle_file_list_key(state: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if !state.files.is_empty() {
                state.selected_file = (state.selected_file + 1).min(state.files.len() - 1);
                state.diff_scroll = 0;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            state.selected_file = state.selected_file.saturating_sub(1);
            state.diff_scroll = 0;
        }
        KeyCode::Char('g') => {
            state.selected_file = 0;
            state.diff_scroll = 0;
        }
        KeyCode::Char('G') => {
            if !state.files.is_empty() {
                state.selected_file = state.files.len() - 1;
                state.diff_scroll = 0;
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

/// Diff pane keys. Full implementation in Stage 8.
fn handle_diff_key(state: &mut AppState, key: KeyEvent) {
    match (key.code, key.modifiers) {
        (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
            state.diff_scroll = state.diff_scroll.saturating_add(1);
            state.clamp_diff_scroll();
        }
        (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
            state.diff_scroll = state.diff_scroll.saturating_sub(1);
        }
        (KeyCode::Char(' '), KeyModifiers::NONE) => {
            // Page down.
            state.diff_scroll = state.diff_scroll.saturating_add(state.diff_view_height);
            state.clamp_diff_scroll();
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
