use super::navigation::navigate_to_location_target;
use super::output::AppOutput;
use super::viewport::AppViewport;
use crate::app::AppState;
use crate::core::search as core_search;
use crate::core::{DefinitionResultsEffect, SearchResultsEffect};

pub fn apply_search_results_effect(
    state: &mut AppState,
    view: &impl AppViewport,
    update: &mut AppOutput,
    effect: SearchResultsEffect,
) {
    match effect {
        SearchResultsEffect::Close => {
            state.search_results = None;
            state.mark_model_changed();
        }
        SearchResultsEffect::SelectNext => {
            let Some(results) = state.search_results.as_mut() else {
                return;
            };
            if !results.matches.is_empty() {
                results.selected = (results.selected + 1).min(results.matches.len() - 1);
                state.mark_model_changed();
            }
        }
        SearchResultsEffect::SelectPrevious => {
            let Some(results) = state.search_results.as_mut() else {
                return;
            };
            results.selected = results.selected.saturating_sub(1);
            state.mark_model_changed();
        }
        SearchResultsEffect::SelectFirst => {
            let Some(results) = state.search_results.as_mut() else {
                return;
            };
            results.selected = 0;
            state.mark_model_changed();
        }
        SearchResultsEffect::SelectLast => {
            let Some(results) = state.search_results.as_mut() else {
                return;
            };
            if !results.matches.is_empty() {
                results.selected = results.matches.len() - 1;
                state.mark_model_changed();
            }
        }
        SearchResultsEffect::AcceptSelected => {
            let selected = state
                .search_results
                .as_ref()
                .and_then(|results| results.matches.get(results.selected).cloned());
            state.search_results = None;
            state.mark_model_changed();
            if let Some(search_match) = selected {
                let target = core_search::resolve_search_target(&state.files, &search_match);
                navigate_to_location_target(state, view, update, target);
            }
        }
    }
}

pub fn apply_definition_results_effect(
    state: &mut AppState,
    view: &impl AppViewport,
    update: &mut AppOutput,
    effect: DefinitionResultsEffect,
) {
    match effect {
        DefinitionResultsEffect::Close => {
            state.definition_results = None;
            state.mark_model_changed();
        }
        DefinitionResultsEffect::SelectNext => {
            let Some(results) = state.definition_results.as_mut() else {
                return;
            };
            if !results.definitions.is_empty() {
                results.selected = (results.selected + 1).min(results.definitions.len() - 1);
                state.mark_model_changed();
            }
        }
        DefinitionResultsEffect::SelectPrevious => {
            let Some(results) = state.definition_results.as_mut() else {
                return;
            };
            results.selected = results.selected.saturating_sub(1);
            state.mark_model_changed();
        }
        DefinitionResultsEffect::AcceptSelected => {
            let selected = state
                .definition_results
                .as_ref()
                .and_then(|results| results.definitions.get(results.selected).cloned());
            state.definition_results = None;
            state.mark_model_changed();
            if let Some(definition) = selected {
                let target = core_search::resolve_definition_target(&state.files, &definition);
                navigate_to_location_target(state, view, update, target);
            }
        }
    }
}
