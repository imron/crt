//! TUI-side application of core effects.

use super::state::TuiState;
use crate::app::AppState;
use crate::app_update;
use crate::core::{CoreEffect, DefinitionResultsEffect, PromptKind, SearchResultsEffect};

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
            CoreEffect::ShowHelp => {
                tui_state.show_help = true;
            }
            CoreEffect::DismissHelp => {
                tui_state.show_help = false;
            }
            CoreEffect::SearchResults(effect) => {
                apply_search_results_effect(state, tui_state, effect);
            }
            CoreEffect::DefinitionResults(effect) => {
                apply_definition_results_effect(state, tui_state, effect);
            }
            effect => app_effects.push(effect),
        }
    }

    let app_update = app_update::apply_core_effects(state, app_effects);
    let app_handled = app_update.handled;
    let save_layout = app_update.save_layout;
    tui_state.apply_app_update(app_update);
    if save_layout {
        save_layout_config(state, tui_state);
    }
    app_handled || handled
}

fn apply_search_results_effect(
    state: &mut AppState,
    tui_state: &mut TuiState,
    effect: SearchResultsEffect,
) {
    match effect {
        SearchResultsEffect::Close => {
            tui_state.search_results = None;
        }
        SearchResultsEffect::SelectNext => {
            let Some(results) = tui_state.search_results.as_mut() else {
                return;
            };
            if !results.matches.is_empty() {
                results.selected = (results.selected + 1).min(results.matches.len() - 1);
                if results.selected >= results.scroll + 20 {
                    results.scroll = results.selected.saturating_sub(19);
                }
            }
        }
        SearchResultsEffect::SelectPrevious => {
            let Some(results) = tui_state.search_results.as_mut() else {
                return;
            };
            results.selected = results.selected.saturating_sub(1);
            if results.selected < results.scroll {
                results.scroll = results.selected;
            }
        }
        SearchResultsEffect::SelectFirst => {
            let Some(results) = tui_state.search_results.as_mut() else {
                return;
            };
            results.selected = 0;
            results.scroll = 0;
        }
        SearchResultsEffect::SelectLast => {
            let Some(results) = tui_state.search_results.as_mut() else {
                return;
            };
            if !results.matches.is_empty() {
                results.selected = results.matches.len() - 1;
                results.scroll = results.selected.saturating_sub(19);
            }
        }
        SearchResultsEffect::AcceptSelected => {
            let selected = tui_state
                .search_results
                .as_ref()
                .and_then(|results| results.matches.get(results.selected).cloned());
            tui_state.search_results = None;
            if let Some(search_match) = selected {
                let update = app_update::navigate_to_search_match(state, &search_match);
                tui_state.apply_app_update(update);
            }
        }
    }
}

fn apply_definition_results_effect(
    state: &mut AppState,
    tui_state: &mut TuiState,
    effect: DefinitionResultsEffect,
) {
    match effect {
        DefinitionResultsEffect::Close => {
            tui_state.definition_results = None;
        }
        DefinitionResultsEffect::SelectNext => {
            let Some(results) = tui_state.definition_results.as_mut() else {
                return;
            };
            if !results.definitions.is_empty() {
                results.selected = (results.selected + 1).min(results.definitions.len() - 1);
            }
        }
        DefinitionResultsEffect::SelectPrevious => {
            let Some(results) = tui_state.definition_results.as_mut() else {
                return;
            };
            results.selected = results.selected.saturating_sub(1);
        }
        DefinitionResultsEffect::AcceptSelected => {
            let selected = tui_state
                .definition_results
                .as_ref()
                .and_then(|results| results.definitions.get(results.selected).cloned());
            tui_state.definition_results = None;
            if let Some(definition) = selected {
                let update = app_update::navigate_to_definition(state, &definition);
                tui_state.apply_app_update(update);
            }
        }
    }
}

fn save_layout_config(state: &AppState, tui_state: &TuiState) {
    if let Some(path) = &tui_state.config_path {
        let layout = crate::config::LayoutConfig {
            file_list_width: tui_state.file_list_width,
            diff_algorithm: Some(state.diff_algorithm),
        };
        crate::config::save_layout(path, &layout);
    }
}
