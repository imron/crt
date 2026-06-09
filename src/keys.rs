//! Input handling and key dispatch.
//!
//! Key events are normalized into core input where possible, with local
//! prompt editing retained by the TUI adapter.

use crossterm::event::{
    KeyCode, KeyEvent as CrosstermKeyEvent, KeyEventKind as CrosstermKeyEventKind, KeyModifiers,
};

use crate::app::{AppState, InputMode};
use crate::app_update;
use crate::core::{
    InputEvent, InputModifiers, Key as CoreKey, KeyEvent as CoreKeyEvent,
    KeyEventKind as CoreKeyEventKind,
};

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
        .handle_input(event, &app_update::interaction_context(state));
    app_update::apply_core_effects(state, effects)
}

fn submit_active_prompt(state: &mut AppState, value: String) -> bool {
    let Some(id) = state.active_core_prompt else {
        return false;
    };
    let effects = state.core_interaction.handle_input(
        InputEvent::PromptSubmit { id, value },
        &app_update::prompt_submit_context(state),
    );
    app_update::apply_core_effects(state, effects)
}

fn cancel_active_prompt(state: &mut AppState) -> bool {
    let Some(id) = state.active_core_prompt else {
        return false;
    };
    let effects = state
        .core_interaction
        .handle_input(InputEvent::PromptCancel { id }, &Default::default());
    app_update::apply_core_effects(state, effects)
}

/// Handle a key press event by mutating the application state.
pub fn handle_key_event(state: &mut AppState, key: CrosstermKeyEvent) {
    if state.input_mode == InputMode::Command {
        handle_command_input(state, key);
        return;
    }

    if state.input_mode == InputMode::DiffSearch {
        handle_diff_search_input(state, key);
        return;
    }

    if state.search_results.is_some() {
        dispatch_core_input(state, key);
        return;
    }

    if state.definition_results.is_some() {
        dispatch_core_input(state, key);
        return;
    }

    if state.show_help {
        dispatch_core_input(state, key);
        return;
    }

    if dispatch_core_input(state, key) {
        return;
    }

    state.status_message = None;
}

/// Handle keystrokes while in command mode.
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
                app_update::apply_unscoped_command_prompt(state, cmd);
            }
        }
        KeyCode::Backspace => {
            if state.command_cursor > 0 {
                state.command_cursor -= 1;
                state.command_input.remove(state.command_cursor);
            } else if !cancel_active_prompt(state) {
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

/// Handle keystrokes while in diff search mode.
fn handle_diff_search_input(state: &mut AppState, key: CrosstermKeyEvent) {
    match key.code {
        KeyCode::Esc => {
            if !cancel_active_prompt(state) {
                state.input_mode = InputMode::Normal;
                state.diff_search_input.clear();
                state.diff_search_cursor = 0;
            }
        }
        KeyCode::Enter => {
            let query = state.diff_search_input.clone();
            if !submit_active_prompt(state, query.clone()) {
                app_update::apply_unscoped_diff_search_prompt(state, query);
            }
        }
        KeyCode::Backspace => {
            if state.diff_search_cursor > 0 {
                state.diff_search_cursor -= 1;
                state.diff_search_input.remove(state.diff_search_cursor);
            } else if !cancel_active_prompt(state) {
                state.input_mode = InputMode::Normal;
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
            state.diff_search_cursor = state.diff_search_cursor.min(state.diff_search_input.len());
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
