//! TUI-side application of core effects.

use super::state::TuiState;
use crate::app::App;
use crate::core::{CoreEffect, PromptKind};

pub fn apply_core_effects(
    app: &mut App,
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
                PromptKind::Comment => {
                    tui_state.open_comment_prompt(prompt.id, prompt.initial_value);
                }
                PromptKind::Custom(_) => {}
            },
            CoreEffect::ClearPrompt { id } => {
                if tui_state.active_core_prompt == Some(id) {
                    tui_state.clear_prompt();
                }
            }
            CoreEffect::ShowHelp => {
                tui_state.show_help = true;
            }
            CoreEffect::DismissHelp => {
                tui_state.show_help = false;
            }
            effect => app_effects.push(effect),
        }
    }

    let app_output = app.apply_core_effects(tui_state, app_effects);
    let app_handled = app_output.handled;
    tui_state.apply_app_output(app_output);
    app_handled || handled
}
