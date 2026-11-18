//! Core interaction entrypoint scaffold.

use super::command::{self, CommandParse};
use super::input::{InputEvent, Key, KeyEventKind};
use super::navigation::Direction;
use super::prompt::{PromptId, PromptKind, PromptRequest};
use super::render::PaneId;
use super::render::{RenderUpdate, StatusMessage};

/// Core output effect stream.
pub type CoreEffects = Vec<CoreEffect>;

/// Core-to-UI effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreEffect {
    RequestPrompt(PromptRequest),
    ClearPrompt { id: PromptId },
    Command(CommandParse),
    DiffSearch(DiffSearchEffect),
    ReviewToggle,
    NavigateFile(Direction),
    JumpHunk(Direction),
    GoToDefinition,
    PopJumpStack,
    TogglePaneFocus,
    TogglePaneVisibility(PaneId),
    ToggleInlineDiff,
    CycleViewMode,
    CycleDiffAlgorithm,
    ToggleDiffBase,
    Render(RenderUpdate),
    Status(StatusMessage),
    ConnectionState(ConnectionState),
    TransientError(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffSearchEffect {
    Submit { query: String },
}

/// Connection lifecycle states for UI adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Connected,
    Reconnecting,
    Reconnected,
    Disconnected,
}

/// Context passed from adapter/runtime into the interaction engine.
#[derive(Debug, Clone, Default)]
pub struct InteractionContext {
    /// Optional hint about current connection state.
    pub connection_state: Option<ConnectionState>,
    /// Word under the cursor, supplied by the adapter for commands like `gd`.
    pub fallback_word: Option<String>,
}

/// Stage-20 scaffold interaction engine.
///
/// This intentionally contains minimal behavior; later stages will migrate
/// existing TUI behavior into this engine.
#[derive(Debug, Default)]
pub struct CoreInteractionEngine {
    next_prompt_id: u64,
    active_prompt: Option<ActivePrompt>,
}

#[derive(Debug, Clone)]
struct ActivePrompt {
    id: PromptId,
    kind: PromptKind,
}

impl CoreInteractionEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Single core input entrypoint for UI adapters.
    pub fn handle_input(
        &mut self,
        event: InputEvent,
        _context: &InteractionContext,
    ) -> CoreEffects {
        match event {
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_command_modifier(key.modifiers)
                    && key.key == Key::Char(':') =>
            {
                let id = self.next_prompt(PromptKind::CommandLine);
                vec![CoreEffect::RequestPrompt(PromptRequest {
                    id,
                    kind: PromptKind::CommandLine,
                    title: "Command".to_string(),
                    placeholder: Some("Type a command".to_string()),
                    initial_value: String::new(),
                })]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_command_modifier(key.modifiers)
                    && key.key == Key::Char('/') =>
            {
                let id = self.next_prompt(PromptKind::Search);
                vec![CoreEffect::RequestPrompt(PromptRequest {
                    id,
                    kind: PromptKind::Search,
                    title: "Search".to_string(),
                    placeholder: Some("Type a regex".to_string()),
                    initial_value: String::new(),
                })]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Char('r') =>
            {
                vec![CoreEffect::ReviewToggle]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_control_modifier_only(key.modifiers)
                    && key.key == Key::Char('n') =>
            {
                vec![CoreEffect::NavigateFile(Direction::Next)]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_control_modifier_only(key.modifiers)
                    && key.key == Key::Char('p') =>
            {
                vec![CoreEffect::NavigateFile(Direction::Prev)]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Char(']') =>
            {
                vec![CoreEffect::JumpHunk(Direction::Next)]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Char('[') =>
            {
                vec![CoreEffect::JumpHunk(Direction::Prev)]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_control_modifier_only(key.modifiers)
                    && key.key == Key::Char(']') =>
            {
                vec![CoreEffect::GoToDefinition]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_control_modifier_only(key.modifiers)
                    && key.key == Key::Char('t') =>
            {
                vec![CoreEffect::PopJumpStack]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_command_modifier(key.modifiers)
                    && key.key == Key::Tab =>
            {
                vec![CoreEffect::TogglePaneFocus]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Char('1') =>
            {
                vec![CoreEffect::TogglePaneVisibility(PaneId::FileList)]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Char('2') =>
            {
                vec![CoreEffect::TogglePaneVisibility(PaneId::Diff)]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Char('i') =>
            {
                vec![CoreEffect::ToggleInlineDiff]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Char('s') =>
            {
                vec![CoreEffect::CycleViewMode]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Char('d') =>
            {
                vec![CoreEffect::CycleDiffAlgorithm]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Char('m') =>
            {
                vec![CoreEffect::ToggleDiffBase]
            }
            InputEvent::PromptSubmit { id, value } => {
                let Some(active) = self.take_active_prompt(id) else {
                    return Vec::new();
                };

                match active.kind {
                    PromptKind::CommandLine => vec![
                        CoreEffect::ClearPrompt { id },
                        CoreEffect::Command(command::parse_command(
                            &value,
                            _context.fallback_word.as_deref(),
                        )),
                    ],
                    PromptKind::Search => vec![
                        CoreEffect::ClearPrompt { id },
                        CoreEffect::DiffSearch(DiffSearchEffect::Submit { query: value }),
                    ],
                    PromptKind::Custom(_) => vec![CoreEffect::ClearPrompt { id }],
                }
            }
            InputEvent::PromptCancel { id } => {
                if self.take_active_prompt(id).is_some() {
                    vec![CoreEffect::ClearPrompt { id }]
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        }
    }

    fn next_prompt(&mut self, kind: PromptKind) -> PromptId {
        self.next_prompt_id = self.next_prompt_id.saturating_add(1);
        let id = PromptId(self.next_prompt_id);
        self.active_prompt = Some(ActivePrompt { id, kind });
        id
    }

    fn take_active_prompt(&mut self, id: PromptId) -> Option<ActivePrompt> {
        let active = self.active_prompt.clone()?;
        if active.id != id {
            return None;
        }
        self.active_prompt = None;
        Some(active)
    }
}

fn key_has_no_command_modifier(modifiers: super::input::InputModifiers) -> bool {
    !modifiers.ctrl && !modifiers.alt
}

fn key_has_no_modifier(modifiers: super::input::InputModifiers) -> bool {
    !modifiers.ctrl && !modifiers.alt && !modifiers.shift
}

fn key_has_control_modifier_only(modifiers: super::input::InputModifiers) -> bool {
    modifiers.ctrl && !modifiers.alt && !modifiers.shift
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::input::{InputModifiers, KeyEvent};

    fn key_event(key: Key, modifiers: InputModifiers) -> InputEvent {
        InputEvent::Key(KeyEvent {
            kind: KeyEventKind::Press,
            key,
            modifiers,
        })
    }

    fn requested_prompt_id(effects: &[CoreEffect]) -> PromptId {
        match effects {
            [CoreEffect::RequestPrompt(PromptRequest { id, .. })] => *id,
            _ => panic!("expected prompt request"),
        }
    }

    #[test]
    fn colon_requests_command_prompt() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(Key::Char(':'), InputModifiers::default()),
            &InteractionContext::default(),
        );

        assert!(matches!(
            effects.as_slice(),
            [CoreEffect::RequestPrompt(PromptRequest {
                kind: PromptKind::CommandLine,
                ..
            })]
        ));
    }

    #[test]
    fn slash_requests_search_prompt() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(Key::Char('/'), InputModifiers::default()),
            &InteractionContext::default(),
        );

        assert!(matches!(
            effects.as_slice(),
            [CoreEffect::RequestPrompt(PromptRequest {
                kind: PromptKind::Search,
                ..
            })]
        ));
    }

    #[test]
    fn shifted_colon_still_requests_command_prompt() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char(':'),
                InputModifiers {
                    shift: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert!(matches!(
            effects.as_slice(),
            [CoreEffect::RequestPrompt(PromptRequest {
                kind: PromptKind::CommandLine,
                ..
            })]
        ));
    }

    #[test]
    fn control_colon_does_not_request_prompt() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char(':'),
                InputModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert!(effects.is_empty());
    }

    #[test]
    fn r_requests_review_toggle() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(Key::Char('r'), InputModifiers::default()),
            &InteractionContext::default(),
        );

        assert_eq!(effects, vec![CoreEffect::ReviewToggle]);
    }

    #[test]
    fn control_r_does_not_request_review_toggle() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char('r'),
                InputModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert!(effects.is_empty());
    }

    #[test]
    fn control_n_requests_next_file_navigation() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char('n'),
                InputModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert_eq!(effects, vec![CoreEffect::NavigateFile(Direction::Next)]);
    }

    #[test]
    fn control_p_requests_previous_file_navigation() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char('p'),
                InputModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert_eq!(effects, vec![CoreEffect::NavigateFile(Direction::Prev)]);
    }

    #[test]
    fn shifted_control_n_does_not_request_file_navigation() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char('n'),
                InputModifiers {
                    ctrl: true,
                    shift: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert!(effects.is_empty());
    }

    #[test]
    fn bracket_keys_request_hunk_navigation() {
        let mut engine = CoreInteractionEngine::new();

        let next = engine.handle_input(
            key_event(Key::Char(']'), InputModifiers::default()),
            &InteractionContext::default(),
        );
        let prev = engine.handle_input(
            key_event(Key::Char('['), InputModifiers::default()),
            &InteractionContext::default(),
        );

        assert_eq!(next, vec![CoreEffect::JumpHunk(Direction::Next)]);
        assert_eq!(prev, vec![CoreEffect::JumpHunk(Direction::Prev)]);
    }

    #[test]
    fn control_bracket_requests_definition_navigation() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char(']'),
                InputModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert_eq!(effects, vec![CoreEffect::GoToDefinition]);
    }

    #[test]
    fn control_t_requests_jump_stack_pop() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char('t'),
                InputModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert_eq!(effects, vec![CoreEffect::PopJumpStack]);
    }

    #[test]
    fn shifted_bracket_does_not_request_hunk_navigation() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char(']'),
                InputModifiers {
                    shift: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert!(effects.is_empty());
    }

    #[test]
    fn tab_requests_pane_focus_toggle() {
        let mut engine = CoreInteractionEngine::new();

        let normal = engine.handle_input(
            key_event(Key::Tab, InputModifiers::default()),
            &InteractionContext::default(),
        );
        let shifted = engine.handle_input(
            key_event(
                Key::Tab,
                InputModifiers {
                    shift: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert_eq!(normal, vec![CoreEffect::TogglePaneFocus]);
        assert_eq!(shifted, vec![CoreEffect::TogglePaneFocus]);
    }

    #[test]
    fn number_keys_request_pane_visibility_toggle() {
        let mut engine = CoreInteractionEngine::new();

        let file_list = engine.handle_input(
            key_event(Key::Char('1'), InputModifiers::default()),
            &InteractionContext::default(),
        );
        let diff = engine.handle_input(
            key_event(Key::Char('2'), InputModifiers::default()),
            &InteractionContext::default(),
        );

        assert_eq!(
            file_list,
            vec![CoreEffect::TogglePaneVisibility(PaneId::FileList)]
        );
        assert_eq!(diff, vec![CoreEffect::TogglePaneVisibility(PaneId::Diff)]);
    }

    #[test]
    fn view_keys_request_view_effects() {
        let mut engine = CoreInteractionEngine::new();

        let inline = engine.handle_input(
            key_event(Key::Char('i'), InputModifiers::default()),
            &InteractionContext::default(),
        );
        let view = engine.handle_input(
            key_event(Key::Char('s'), InputModifiers::default()),
            &InteractionContext::default(),
        );
        let algorithm = engine.handle_input(
            key_event(Key::Char('d'), InputModifiers::default()),
            &InteractionContext::default(),
        );
        let base = engine.handle_input(
            key_event(Key::Char('m'), InputModifiers::default()),
            &InteractionContext::default(),
        );

        assert_eq!(inline, vec![CoreEffect::ToggleInlineDiff]);
        assert_eq!(view, vec![CoreEffect::CycleViewMode]);
        assert_eq!(algorithm, vec![CoreEffect::CycleDiffAlgorithm]);
        assert_eq!(base, vec![CoreEffect::ToggleDiffBase]);
    }

    #[test]
    fn shifted_view_key_does_not_request_view_effect() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char('i'),
                InputModifiers {
                    shift: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert!(effects.is_empty());
    }

    #[test]
    fn command_prompt_submit_emits_parsed_command() {
        let mut engine = CoreInteractionEngine::new();
        let effects = engine.handle_input(
            key_event(Key::Char(':'), InputModifiers::default()),
            &InteractionContext::default(),
        );
        let id = requested_prompt_id(&effects);

        let effects = engine.handle_input(
            InputEvent::PromptSubmit {
                id,
                value: "gr needle".to_string(),
            },
            &InteractionContext::default(),
        );

        assert_eq!(
            effects,
            vec![
                CoreEffect::ClearPrompt { id },
                CoreEffect::Command(CommandParse::Parsed(command::Command::SearchAll {
                    pattern: "needle".to_string()
                }))
            ]
        );
    }

    #[test]
    fn command_prompt_submit_uses_context_fallback_word() {
        let mut engine = CoreInteractionEngine::new();
        let effects = engine.handle_input(
            key_event(Key::Char(':'), InputModifiers::default()),
            &InteractionContext::default(),
        );
        let id = requested_prompt_id(&effects);

        let effects = engine.handle_input(
            InputEvent::PromptSubmit {
                id,
                value: "gd".to_string(),
            },
            &InteractionContext {
                fallback_word: Some("cursor_symbol".to_string()),
                ..InteractionContext::default()
            },
        );

        assert_eq!(
            effects,
            vec![
                CoreEffect::ClearPrompt { id },
                CoreEffect::Command(CommandParse::Parsed(command::Command::FindDefinition {
                    symbol: "cursor_symbol".to_string()
                }))
            ]
        );
    }

    #[test]
    fn search_prompt_submit_emits_diff_search_effect() {
        let mut engine = CoreInteractionEngine::new();
        let effects = engine.handle_input(
            key_event(Key::Char('/'), InputModifiers::default()),
            &InteractionContext::default(),
        );
        let id = requested_prompt_id(&effects);

        let effects = engine.handle_input(
            InputEvent::PromptSubmit {
                id,
                value: "needle".to_string(),
            },
            &InteractionContext::default(),
        );

        assert_eq!(
            effects,
            vec![
                CoreEffect::ClearPrompt { id },
                CoreEffect::DiffSearch(DiffSearchEffect::Submit {
                    query: "needle".to_string()
                })
            ]
        );
    }

    #[test]
    fn prompt_cancel_clears_active_prompt() {
        let mut engine = CoreInteractionEngine::new();
        let effects = engine.handle_input(
            key_event(Key::Char(':'), InputModifiers::default()),
            &InteractionContext::default(),
        );
        let id = requested_prompt_id(&effects);

        let effects = engine.handle_input(
            InputEvent::PromptCancel { id },
            &InteractionContext::default(),
        );

        assert_eq!(effects, vec![CoreEffect::ClearPrompt { id }]);
    }
}
