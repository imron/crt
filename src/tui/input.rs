//! Input handling and key dispatch.
//!
//! Key events are normalized into core input where possible, with local
//! prompt editing retained by the TUI adapter.

use crossterm::event::{
    KeyCode, KeyEvent as CrosstermKeyEvent, KeyEventKind as CrosstermKeyEventKind, KeyModifiers,
    MouseButton as CrosstermMouseButton, MouseEvent as CrosstermMouseEvent,
    MouseEventKind as CrosstermMouseEventKind,
};

use super::state::{InputMode, TuiState};
use crate::app::model::AppModel;
use crate::core::{
    InputEvent, InputModifiers, Key as CoreKey, KeyEvent as CoreKeyEvent,
    KeyEventKind as CoreKeyEventKind, MouseButton as CoreMouseButton, MouseEvent as CoreMouseEvent,
    MouseEventKind as CoreMouseEventKind,
};

#[derive(Debug, Clone, PartialEq)]
pub enum CoreInputDispatch {
    Interaction(InputEvent),
    PromptSubmit(InputEvent),
    PromptCancel(InputEvent),
}

#[derive(Debug, Clone, PartialEq)]
pub enum KeyInputResult {
    Core(CoreInputDispatch),
    Local,
    Unhandled,
}

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

pub fn input_event_from_mouse(
    model: &AppModel,
    tui_state: &TuiState,
    mouse: &CrosstermMouseEvent,
) -> Option<InputEvent> {
    let (kind, button) = match mouse.kind {
        CrosstermMouseEventKind::Down(button) => {
            (CoreMouseEventKind::Down, Some(core_mouse_button(button)?))
        }
        CrosstermMouseEventKind::Up(button) => {
            (CoreMouseEventKind::Up, Some(core_mouse_button(button)?))
        }
        CrosstermMouseEventKind::Drag(button) => {
            (CoreMouseEventKind::Drag, Some(core_mouse_button(button)?))
        }
        CrosstermMouseEventKind::Moved => (CoreMouseEventKind::Move, None),
        CrosstermMouseEventKind::ScrollDown => (CoreMouseEventKind::ScrollDown, None),
        CrosstermMouseEventKind::ScrollUp => (CoreMouseEventKind::ScrollUp, None),
        _ => return None,
    };

    let pane = tui_state.pane_at(mouse.column, mouse.row);
    let local_pos = pane.map(|pane| {
        let area = tui_state.area_for_pane(pane);
        (
            mouse.column.saturating_sub(area.x),
            mouse.row.saturating_sub(area.y),
        )
    });

    Some(InputEvent::Mouse(CoreMouseEvent {
        kind,
        button,
        local_pos,
        semantic_hit: pane
            .and_then(|pane| tui_state.pointer_semantic_hit(model, pane, mouse.column, mouse.row)),
        modifiers: InputModifiers {
            ctrl: mouse.modifiers.contains(KeyModifiers::CONTROL),
            alt: mouse.modifiers.contains(KeyModifiers::ALT),
            shift: mouse.modifiers.contains(KeyModifiers::SHIFT),
        },
    }))
}

fn core_mouse_button(button: CrosstermMouseButton) -> Option<CoreMouseButton> {
    match button {
        CrosstermMouseButton::Left => Some(CoreMouseButton::Left),
        CrosstermMouseButton::Right => Some(CoreMouseButton::Right),
        CrosstermMouseButton::Middle => Some(CoreMouseButton::Middle),
    }
}

fn dispatch_core_input(key: CrosstermKeyEvent) -> KeyInputResult {
    input_event_from_key(key)
        .map(|event| KeyInputResult::Core(CoreInputDispatch::Interaction(event)))
        .unwrap_or(KeyInputResult::Unhandled)
}

fn submit_active_prompt(tui_state: &mut TuiState, value: String) -> Option<CoreInputDispatch> {
    let Some(id) = tui_state.active_core_prompt else {
        return None;
    };
    Some(CoreInputDispatch::PromptSubmit(InputEvent::PromptSubmit {
        id,
        value,
    }))
}

fn cancel_active_prompt(tui_state: &mut TuiState) -> Option<CoreInputDispatch> {
    let Some(id) = tui_state.active_core_prompt else {
        return None;
    };
    Some(CoreInputDispatch::PromptCancel(InputEvent::PromptCancel {
        id,
    }))
}

/// Handle a key press event by updating local TUI input state and emitting core input.
pub fn handle_key_event(tui_state: &mut TuiState, key: CrosstermKeyEvent) -> KeyInputResult {
    if tui_state.input_mode == InputMode::Command {
        return handle_command_input(tui_state, key);
    }

    if tui_state.input_mode == InputMode::DiffSearch {
        return handle_diff_search_input(tui_state, key);
    }

    if tui_state.show_help {
        return dispatch_core_input(key);
    }

    dispatch_core_input(key)
}

/// Handle keystrokes while in command mode.
fn handle_command_input(tui_state: &mut TuiState, key: CrosstermKeyEvent) -> KeyInputResult {
    match key.code {
        KeyCode::Esc => {
            if let Some(dispatch) = cancel_active_prompt(tui_state) {
                return KeyInputResult::Core(dispatch);
            }
            tui_state.clear_prompt();
        }
        KeyCode::Enter => {
            let cmd = tui_state.command_input.clone();
            if let Some(dispatch) = submit_active_prompt(tui_state, cmd) {
                return KeyInputResult::Core(dispatch);
            }
            tui_state.clear_prompt();
        }
        KeyCode::Backspace => {
            if tui_state.command_cursor > 0 {
                tui_state.command_cursor -= 1;
                tui_state.command_input.remove(tui_state.command_cursor);
            } else if let Some(dispatch) = cancel_active_prompt(tui_state) {
                return KeyInputResult::Core(dispatch);
            } else {
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
    KeyInputResult::Local
}

/// Handle keystrokes while in diff search mode.
fn handle_diff_search_input(tui_state: &mut TuiState, key: CrosstermKeyEvent) -> KeyInputResult {
    match key.code {
        KeyCode::Esc => {
            if let Some(dispatch) = cancel_active_prompt(tui_state) {
                return KeyInputResult::Core(dispatch);
            }
            tui_state.clear_prompt();
        }
        KeyCode::Enter => {
            let query = tui_state.diff_search_input.clone();
            if let Some(dispatch) = submit_active_prompt(tui_state, query) {
                return KeyInputResult::Core(dispatch);
            }
            tui_state.clear_prompt();
        }
        KeyCode::Backspace => {
            if tui_state.diff_search_cursor > 0 {
                tui_state.diff_search_cursor -= 1;
                tui_state
                    .diff_search_input
                    .remove(tui_state.diff_search_cursor);
            } else if let Some(dispatch) = cancel_active_prompt(tui_state) {
                return KeyInputResult::Core(dispatch);
            } else {
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
    KeyInputResult::Local
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppState;
    use crate::app::model::AppModel;
    use crate::config::DiffAlgorithm;
    use crate::core::{AppTarget, PromptId};
    use crate::core::{
        MouseButton as CoreMouseButton, MouseEvent as CoreMouseEvent,
        MouseEventKind as CoreMouseEventKind, PaneId, PointerSemanticHit, TextAnchor,
    };
    use crate::review_types::{
        ChangeKind, ConnectionContext, DiffContent, FileChange, FileEntry, ReviewStatus,
    };
    use crossterm::event::KeyEvent as CrosstermKeyEvent;
    use ratatui::layout::Rect;

    fn test_context() -> ConnectionContext {
        ConnectionContext {
            repo_root: "/repo".to_string(),
            worktree: "/repo".to_string(),
            base_ref: "main".to_string(),
            head_ref: "feature".to_string(),
            merge_base: "abc123".to_string(),
        }
    }

    fn test_file(path: &str) -> FileEntry {
        FileEntry {
            change: FileChange {
                path: path.to_string(),
                old_path: None,
                kind: ChangeKind::Modified,
            },
            status: ReviewStatus::Unreviewed,
            diff: DiffContent {
                hunks: Vec::new(),
                is_binary: false,
                diff_hash: format!("hash-{path}"),
            },
        }
    }

    fn test_state() -> AppState {
        AppState::new(
            DiffAlgorithm::Myers,
            test_context(),
            vec![test_file("src/lib.rs")],
        )
    }

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

    #[test]
    fn handle_key_event_emits_core_input_without_dispatching_it() {
        let mut tui_state = TuiState::default();

        let result = handle_key_event(
            &mut tui_state,
            CrosstermKeyEvent::new(KeyCode::Char(':'), KeyModifiers::SHIFT),
        );

        assert_eq!(
            result,
            KeyInputResult::Core(CoreInputDispatch::Interaction(InputEvent::Key(
                CoreKeyEvent {
                    kind: CoreKeyEventKind::Press,
                    key: CoreKey::Char(':'),
                    modifiers: InputModifiers {
                        shift: true,
                        ..Default::default()
                    },
                }
            )))
        );
        assert_eq!(tui_state.input_mode, InputMode::Normal);
    }

    #[test]
    fn command_prompt_submit_emits_prompt_input() {
        let mut tui_state = TuiState::default();
        tui_state.open_command_prompt(PromptId(7), "gd symbol".to_string());

        let result = handle_key_event(
            &mut tui_state,
            CrosstermKeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );

        assert_eq!(
            result,
            KeyInputResult::Core(CoreInputDispatch::PromptSubmit(InputEvent::PromptSubmit {
                id: PromptId(7),
                value: "gd symbol".to_string(),
            }))
        );
    }

    #[test]
    fn mouse_input_event_maps_file_list_hit() {
        let state = test_state();
        let model = AppModel::from_state(&state);
        let tui_state = TuiState {
            file_list_area: Rect::new(0, 0, 30, 10),
            file_list_row_to_file: vec![Some(0)],
            ..TuiState::default()
        };

        let event = input_event_from_mouse(
            &model,
            &tui_state,
            &CrosstermMouseEvent {
                kind: CrosstermMouseEventKind::Down(CrosstermMouseButton::Left),
                column: 5,
                row: 1,
                modifiers: KeyModifiers::CONTROL,
            },
        )
        .expect("expected mouse input");

        assert_eq!(
            event,
            InputEvent::Mouse(CoreMouseEvent {
                kind: CoreMouseEventKind::Down,
                button: Some(CoreMouseButton::Left),
                local_pos: Some((5, 1)),
                semantic_hit: Some(PointerSemanticHit {
                    pane_id: PaneId::FileList,
                    target: AppTarget::File { index: 0 },
                    region_id: Some("file:src/lib.rs".to_string()),
                    text_anchor: Some(TextAnchor { line: 0, column: 4 }),
                }),
                modifiers: InputModifiers {
                    ctrl: true,
                    alt: false,
                    shift: false,
                },
            })
        );
    }

    #[test]
    fn mouse_input_event_maps_diff_hit_to_content_anchor() {
        let mut state = test_state();
        state.diff_scroll = 10;
        let model = AppModel::from_state(&state);
        let tui_state = TuiState {
            show_file_list: false,
            diff_area: Rect::new(0, 0, 80, 20),
            diff_gutter_cols: 4,
            ..TuiState::default()
        };

        let event = input_event_from_mouse(
            &model,
            &tui_state,
            &CrosstermMouseEvent {
                kind: CrosstermMouseEventKind::ScrollDown,
                column: 12,
                row: 3,
                modifiers: KeyModifiers::SHIFT,
            },
        )
        .expect("expected mouse input");

        assert_eq!(
            event,
            InputEvent::Mouse(CoreMouseEvent {
                kind: CoreMouseEventKind::ScrollDown,
                button: None,
                local_pos: Some((12, 3)),
                semantic_hit: Some(PointerSemanticHit {
                    pane_id: PaneId::Diff,
                    target: AppTarget::DiffText {
                        anchor: TextAnchor {
                            line: 12,
                            column: 4,
                        },
                    },
                    region_id: Some("diff-line:12".to_string()),
                    text_anchor: Some(TextAnchor {
                        line: 12,
                        column: 4,
                    }),
                }),
                modifiers: InputModifiers {
                    ctrl: false,
                    alt: false,
                    shift: true,
                },
            })
        );
    }
}
