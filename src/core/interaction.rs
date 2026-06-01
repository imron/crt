//! Core interaction entrypoint scaffold.

use super::input::{InputEvent, Key, KeyEventKind};
use super::prompt::{PromptId, PromptKind, PromptRequest};
use super::render::{RenderUpdate, StatusMessage};

/// Core output effect stream.
pub type CoreEffects = Vec<CoreEffect>;

/// Core-to-UI effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreEffect {
    RequestPrompt(PromptRequest),
    ClearPrompt { id: PromptId },
    Render(RenderUpdate),
    Status(StatusMessage),
    ConnectionState(ConnectionState),
    TransientError(String),
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
}

/// Stage-20 scaffold interaction engine.
///
/// This intentionally contains minimal behavior; later stages will migrate
/// existing TUI behavior into this engine.
#[derive(Debug, Default)]
pub struct CoreInteractionEngine {
    next_prompt_id: u64,
    active_prompt: Option<PromptId>,
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
                    && key.modifiers == Default::default()
                    && key.key == Key::Char(':') =>
            {
                let id = self.next_prompt();
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
                    && key.modifiers == Default::default()
                    && key.key == Key::Char('/') =>
            {
                let id = self.next_prompt();
                vec![CoreEffect::RequestPrompt(PromptRequest {
                    id,
                    kind: PromptKind::Search,
                    title: "Search".to_string(),
                    placeholder: Some("Type a regex".to_string()),
                    initial_value: String::new(),
                })]
            }
            InputEvent::PromptSubmit { id, value } => {
                // Scaffold behavior: echo submit and close prompt.
                self.active_prompt = None;
                vec![
                    CoreEffect::ClearPrompt { id },
                    CoreEffect::Status(StatusMessage {
                        text: format!("Input received: {value}"),
                    }),
                ]
            }
            InputEvent::PromptCancel { id } => {
                self.active_prompt = None;
                vec![CoreEffect::ClearPrompt { id }]
            }
            _ => Vec::new(),
        }
    }

    fn next_prompt(&mut self) -> PromptId {
        self.next_prompt_id = self.next_prompt_id.saturating_add(1);
        let id = PromptId(self.next_prompt_id);
        self.active_prompt = Some(id);
        id
    }
}
