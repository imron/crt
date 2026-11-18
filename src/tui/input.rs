//! Input handling and key dispatch.
//!
//! Key events are normalized into core input where possible, with local
//! prompt editing retained by the TUI adapter.

use std::time::Duration;

use crossterm::event::{
    KeyCode, KeyEvent as CrosstermKeyEvent, KeyEventKind as CrosstermKeyEventKind, KeyModifiers,
};

use super::effects::apply_core_effects;
use super::state::{InputMode, TuiState};
use crate::app::AppState;
use crate::app_update;
use crate::core::{
    InputEvent, InputModifiers, InteractionContext, Key as CoreKey, KeyEvent as CoreKeyEvent,
    KeyEventKind as CoreKeyEventKind,
};

/// How long the "Press Ctrl-C again" prompt stays active.
const CTRL_C_TIMEOUT: Duration = Duration::from_secs(3);

fn input_event_from_key(key: CrosstermKeyEvent) -> Option<InputEvent> {
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

fn dispatch_core_input(
    state: &mut AppState,
    tui_state: &mut TuiState,
    key: CrosstermKeyEvent,
) -> bool {
    let Some(event) = input_event_from_key(key) else {
        return false;
    };
    let effects = state
        .core_interaction
        .handle_input(event, &interaction_context(state, tui_state));
    apply_core_effects(state, tui_state, effects)
}

fn interaction_context(state: &AppState, tui_state: &TuiState) -> InteractionContext {
    InteractionContext {
        help_visible: tui_state.show_help,
        quit_confirmation_active: tui_state.quit_confirmation_active(CTRL_C_TIMEOUT),
        ..app_update::interaction_context(state)
    }
}

fn submit_active_prompt(state: &mut AppState, tui_state: &mut TuiState, value: String) -> bool {
    let Some(id) = tui_state.active_core_prompt else {
        return false;
    };
    let effects = state.core_interaction.handle_input(
        InputEvent::PromptSubmit { id, value },
        &app_update::prompt_submit_context(state),
    );
    apply_core_effects(state, tui_state, effects)
}

fn cancel_active_prompt(state: &mut AppState, tui_state: &mut TuiState) -> bool {
    let Some(id) = tui_state.active_core_prompt else {
        return false;
    };
    let effects = state
        .core_interaction
        .handle_input(InputEvent::PromptCancel { id }, &Default::default());
    apply_core_effects(state, tui_state, effects)
}

/// Handle a key press event by mutating the application state.
pub fn handle_key_event(state: &mut AppState, tui_state: &mut TuiState, key: CrosstermKeyEvent) {
    if tui_state.input_mode == InputMode::Command {
        handle_command_input(state, tui_state, key);
        return;
    }

    if tui_state.input_mode == InputMode::DiffSearch {
        handle_diff_search_input(state, tui_state, key);
        return;
    }

    if state.search_results.is_some() {
        dispatch_core_input(state, tui_state, key);
        return;
    }

    if state.definition_results.is_some() {
        dispatch_core_input(state, tui_state, key);
        return;
    }

    if tui_state.show_help {
        dispatch_core_input(state, tui_state, key);
        return;
    }

    if dispatch_core_input(state, tui_state, key) {
        return;
    }

    tui_state.clear_status_message();
}

/// Handle keystrokes while in command mode.
fn handle_command_input(state: &mut AppState, tui_state: &mut TuiState, key: CrosstermKeyEvent) {
    match key.code {
        KeyCode::Esc => {
            if !cancel_active_prompt(state, tui_state) {
                tui_state.clear_prompt();
            }
        }
        KeyCode::Enter => {
            let cmd = tui_state.command_input.clone();
            if !submit_active_prompt(state, tui_state, cmd.clone()) {
                tui_state.clear_prompt();
                let update = app_update::apply_unscoped_command_prompt(state, cmd);
                tui_state.apply_status_update(update.status);
            }
        }
        KeyCode::Backspace => {
            if tui_state.command_cursor > 0 {
                tui_state.command_cursor -= 1;
                tui_state.command_input.remove(tui_state.command_cursor);
            } else if !cancel_active_prompt(state, tui_state) {
                tui_state.clear_prompt();
            }
        }
        KeyCode::Delete => {
            if tui_state.command_cursor < tui_state.command_input.len() {
                tui_state.command_input.remove(tui_state.command_cursor);
            }
        }
        KeyCode::Left => {
            tui_state.command_cursor = tui_state.command_cursor.saturating_sub(1);
        }
        KeyCode::Right => {
            tui_state.command_cursor = tui_state
                .command_cursor
                .min(tui_state.command_input.len())
                .saturating_add(1);
            tui_state.command_cursor = tui_state.command_cursor.min(tui_state.command_input.len());
        }
        KeyCode::Home => {
            tui_state.command_cursor = 0;
        }
        KeyCode::End => {
            tui_state.command_cursor = tui_state.command_input.len();
        }
        KeyCode::Char(c) => {
            tui_state.command_input.insert(tui_state.command_cursor, c);
            tui_state.command_cursor += 1;
        }
        _ => {}
    }
}

/// Handle keystrokes while in diff search mode.
fn handle_diff_search_input(
    state: &mut AppState,
    tui_state: &mut TuiState,
    key: CrosstermKeyEvent,
) {
    match key.code {
        KeyCode::Esc => {
            if !cancel_active_prompt(state, tui_state) {
                tui_state.clear_prompt();
            }
        }
        KeyCode::Enter => {
            let query = tui_state.diff_search_input.clone();
            if !submit_active_prompt(state, tui_state, query.clone()) {
                tui_state.clear_prompt();
                let update = app_update::apply_unscoped_diff_search_prompt(state, query);
                tui_state.apply_status_update(update.status);
            }
        }
        KeyCode::Backspace => {
            if tui_state.diff_search_cursor > 0 {
                tui_state.diff_search_cursor -= 1;
                tui_state
                    .diff_search_input
                    .remove(tui_state.diff_search_cursor);
            } else if !cancel_active_prompt(state, tui_state) {
                tui_state.clear_prompt();
            }
        }
        KeyCode::Delete => {
            if tui_state.diff_search_cursor < tui_state.diff_search_input.len() {
                tui_state
                    .diff_search_input
                    .remove(tui_state.diff_search_cursor);
            }
        }
        KeyCode::Left => {
            tui_state.diff_search_cursor = tui_state.diff_search_cursor.saturating_sub(1);
        }
        KeyCode::Right => {
            tui_state.diff_search_cursor = tui_state
                .diff_search_cursor
                .min(tui_state.diff_search_input.len())
                .saturating_add(1);
            tui_state.diff_search_cursor = tui_state
                .diff_search_cursor
                .min(tui_state.diff_search_input.len());
        }
        KeyCode::Home => {
            tui_state.diff_search_cursor = 0;
        }
        KeyCode::End => {
            tui_state.diff_search_cursor = tui_state.diff_search_input.len();
        }
        KeyCode::Char(c) => {
            tui_state
                .diff_search_input
                .insert(tui_state.diff_search_cursor, c);
            tui_state.diff_search_cursor += 1;
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
