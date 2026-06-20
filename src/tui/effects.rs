//! TUI-side application of core effects.

use super::state::TuiState;
use crate::app::{App, AppState};
use crate::core::{
    CoreEffect, DefinitionResultsEffect, PaneEffect, PaneId, PromptKind, SearchResultsEffect,
};
use crate::review_types::PaneFocus;

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
                apply_search_results_effect(app, tui_state, effect);
            }
            CoreEffect::DefinitionResults(effect) => {
                apply_definition_results_effect(app, tui_state, effect);
            }
            CoreEffect::TogglePaneFocus => {
                toggle_pane_focus(&mut app.state, tui_state);
                tui_state.clear_status_message();
            }
            CoreEffect::TogglePaneVisibility(PaneId::FileList) => {
                toggle_pane_visibility(&mut app.state, tui_state, PaneFocus::FileList);
                tui_state.clear_status_message();
            }
            CoreEffect::TogglePaneVisibility(PaneId::Diff) => {
                toggle_pane_visibility(&mut app.state, tui_state, PaneFocus::Diff);
                tui_state.clear_status_message();
            }
            CoreEffect::TogglePaneVisibility(_) => {}
            CoreEffect::Pane(PaneEffect::ActivateFileListSelection) => {
                if tui_state.show_diff_pane {
                    app.state.pane_focus = PaneFocus::Diff;
                }
            }
            effect => app_effects.push(effect),
        }
    }

    let app_output = app.apply_core_effects(tui_state, app_effects);
    let app_handled = app_output.handled;
    let save_layout = app_output.save_layout;
    tui_state.apply_app_output(app_output);
    if save_layout {
        save_layout_config(app, tui_state);
    }
    app_handled || handled
}

fn toggle_pane_focus(state: &mut AppState, tui_state: &TuiState) {
    if tui_state.show_file_list && tui_state.show_diff_pane {
        state.pane_focus = match state.pane_focus {
            PaneFocus::FileList => PaneFocus::Diff,
            PaneFocus::Diff => PaneFocus::FileList,
        };
    }
}

/// Toggle visibility of a pane. At least one pane must remain visible.
fn toggle_pane_visibility(state: &mut AppState, tui_state: &mut TuiState, pane: PaneFocus) {
    match pane {
        PaneFocus::FileList => {
            if tui_state.show_file_list {
                if tui_state.show_diff_pane {
                    tui_state.show_file_list = false;
                    state.pane_focus = PaneFocus::Diff;
                }
            } else {
                tui_state.show_file_list = true;
            }
        }
        PaneFocus::Diff => {
            if tui_state.show_diff_pane {
                if tui_state.show_file_list {
                    tui_state.show_diff_pane = false;
                    state.pane_focus = PaneFocus::FileList;
                }
            } else {
                tui_state.show_diff_pane = true;
            }
        }
    }
}

fn apply_search_results_effect(
    app: &mut App,
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
                let output = app.navigate_to_search_match(tui_state, &search_match);
                tui_state.apply_app_output(output);
            }
        }
    }
}

fn apply_definition_results_effect(
    app: &mut App,
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
                let output = app.navigate_to_definition(tui_state, &definition);
                tui_state.apply_app_output(output);
            }
        }
    }
}

fn save_layout_config(app: &App, tui_state: &TuiState) {
    if let Some(path) = &tui_state.config_path {
        let layout = crate::config::LayoutConfig {
            file_list_width: tui_state.file_list_width,
            diff_algorithm: Some(app.state.diff_algorithm),
        };
        crate::config::save_layout(path, &layout);
    }
}
