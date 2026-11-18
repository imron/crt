//! TUI-side application of core effects.

use super::state::TuiState;
use crate::app::AppState;
use crate::app_update;
use crate::core::{CoreEffect, PromptKind};

pub fn apply_core_effects(
    state: &mut AppState,
    tui_state: &mut TuiState,
    effects: Vec<CoreEffect>,
) -> bool {
    let mut handled = false;
    let mut app_effects = Vec::new();

    for effect in effects {
        handled = true;
        match effect {
            CoreEffect::RequestPrompt(prompt) => match prompt.kind {
                PromptKind::CommandLine => {
                    tui_state.open_command_prompt(prompt.id, prompt.initial_value);
                }
                PromptKind::Search => {
                    tui_state.open_diff_search_prompt(prompt.id, prompt.initial_value);
                }
                PromptKind::Custom(_) => {}
            },
            CoreEffect::ClearPrompt { id } => {
                if tui_state.active_core_prompt == Some(id) {
                    tui_state.clear_prompt();
                }
            }
            effect => app_effects.push(effect),
        }
    }

    app_update::apply_core_effects(state, app_effects) || handled
}
