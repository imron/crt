//! App-owned update step for core interaction effects.
//!
//! Input adapters feed `InputEvent`s to core interaction. The resulting
//! `CoreEffect`s are applied here to mutate the current app state, enqueue
//! async work for the app loop, or report presentation updates to the UI.

mod command;
mod comments;
mod cursor;
pub mod diff_search;
mod navigation;
mod output;
mod overlays;
mod panes;
mod selection;
mod viewport;

use super::AppState;
use crate::core::navigation::Direction;
use crate::core::{
    CommentEffect, CoreEffect, CurrentCommentContext, DiffSearchEffect, InteractionContext, PaneId,
};
use crate::review_types::PaneFocus;

pub use output::{AppOutput, StatusUpdate};
pub use viewport::AppViewport;

pub fn interaction_context(state: &AppState) -> InteractionContext {
    InteractionContext {
        focused_pane: Some(match state.pane_focus {
            PaneFocus::FileList => PaneId::FileList,
            PaneFocus::Diff => PaneId::Diff,
            PaneFocus::Comments => PaneId::Comments,
        }),
        search_results_visible: state.search_results.is_some(),
        definition_results_visible: state.definition_results.is_some(),
        diff_search_active: state.diff_search_query.is_some(),
        diff_search_has_matches: state.diff_search_query.is_some()
            && !state.diff_search_matches.is_empty(),
        diff_pane_visible: state.show_diff_pane,
        visual_selection_active: state.visual_selection.is_some(),
        pending_comment_anchor_active: state.pending_comment_anchor.is_some(),
        comments_panel_visible: state.show_comments_panel,
        current_comment_active: comments::current_comment(state).is_some(),
        current_comment: comments::current_comment(state).map(|comment| CurrentCommentContext {
            id: comment.id,
            body: comment.body.clone(),
        }),
        ..InteractionContext::default()
    }
}

pub fn prompt_submit_context(state: &AppState, view: &impl AppViewport) -> InteractionContext {
    InteractionContext {
        fallback_word: navigation::extract_word_at_cursor(state, view),
        ..InteractionContext::default()
    }
}

pub fn apply_core_effects(
    state: &mut AppState,
    view: &impl AppViewport,
    effects: Vec<CoreEffect>,
) -> AppOutput {
    let mut update = AppOutput::default();
    for effect in effects {
        update.handled = true;
        match effect {
            CoreEffect::RequestPrompt(_) => {}
            CoreEffect::Status(status) => {
                update.set_status(status.text);
            }
            CoreEffect::ClearPrompt { .. } => {}
            CoreEffect::Command(command) => {
                command::apply_command(state, &mut update, command);
            }
            CoreEffect::DiffSearch(DiffSearchEffect::Submit { query }) => {
                diff_search::apply_diff_search(state, view, &mut update, query);
            }
            CoreEffect::DiffSearch(DiffSearchEffect::NextMatch) => {
                diff_search::navigate_diff_search_match(state, view, &mut update, Direction::Next);
            }
            CoreEffect::DiffSearch(DiffSearchEffect::PreviousMatch) => {
                diff_search::navigate_diff_search_match(state, view, &mut update, Direction::Prev);
            }
            CoreEffect::DiffSearch(DiffSearchEffect::Clear) => {
                diff_search::clear_diff_search(state);
            }
            CoreEffect::Comment(CommentEffect::SubmitBody { body }) => {
                let body = body.trim_end().to_string();
                if body.is_empty() {
                    update.set_status("Comment body is empty");
                } else if let Some(anchor) = state.pending_comment_anchor.clone() {
                    update.request_comment_create(anchor, body);
                    update.set_status("Creating comment...");
                } else {
                    update.set_status("No comment anchor captured");
                }
            }
            CoreEffect::Comment(CommentEffect::SubmitEditBody { id, body }) => {
                let body = body.trim_end().to_string();
                if body.is_empty() {
                    update.set_status("Empty comment ignored");
                } else {
                    update.request_comment_update(id, body);
                    update.set_status(format!("Updating comment #{id}..."));
                }
            }
            CoreEffect::Comment(CommentEffect::Cancel) => {
                state.pending_comment_anchor = None;
                state.visual_selection = None;
                state.mark_model_changed();
                update.set_status("Comment canceled");
            }
            CoreEffect::DiffCursor(effect) => {
                cursor::apply_diff_cursor_effect(state, view, effect);
            }
            CoreEffect::VisualSelection(effect) => {
                selection::apply_visual_selection_effect(state, view, &mut update, effect);
            }
            CoreEffect::SearchResults(effect) => {
                overlays::apply_search_results_effect(state, view, &mut update, effect);
            }
            CoreEffect::DefinitionResults(effect) => {
                overlays::apply_definition_results_effect(state, view, &mut update, effect);
            }
            CoreEffect::Pane(effect) => {
                panes::apply_pane_effect(state, effect);
            }
            CoreEffect::CommentsPanel(effect) => {
                comments::apply_comments_panel_effect(state, view, &mut update, effect);
            }
            CoreEffect::Quit => {
                update.request_quit();
            }
            CoreEffect::Suspend => {
                update.request_suspend();
            }
            CoreEffect::ShowHelp | CoreEffect::DismissHelp => {}
            CoreEffect::ReviewToggle => {
                navigation::toggle_review(state, &mut update);
                update.clear_status();
            }
            CoreEffect::NavigateFile(direction) => {
                navigation::navigate_file(state, direction);
                update.clear_status();
            }
            CoreEffect::JumpHunk(Direction::Next) => {
                navigation::jump_to_next_hunk(state, view);
                update.clear_status();
            }
            CoreEffect::JumpHunk(Direction::Prev) => {
                navigation::jump_to_prev_hunk(state, view);
                update.clear_status();
            }
            CoreEffect::GoToDefinition => {
                navigation::request_go_to_definition(state, view, &mut update);
            }
            CoreEffect::PopJumpStack => {
                navigation::pop_jump_stack(state, view, &mut update);
            }
            CoreEffect::TogglePaneFocus => {
                panes::toggle_pane_focus(state);
                update.clear_status();
            }
            CoreEffect::TogglePaneVisibility(pane) => {
                panes::toggle_pane_visibility(state, pane);
                update.clear_status();
            }
            CoreEffect::ToggleInlineDiff => {
                navigation::toggle_inline_diff(state, &mut update);
            }
            CoreEffect::ToggleBlame => {
                navigation::toggle_blame(state, &mut update);
            }
            CoreEffect::ToggleWhitespaceIgnored => {
                navigation::toggle_whitespace_ignored(state, &mut update);
            }
            CoreEffect::CycleViewMode => {
                update.clear_status();
                navigation::cycle_view_mode(state, view);
            }
            CoreEffect::CycleDiffAlgorithm => {
                navigation::cycle_diff_algorithm(state, &mut update);
            }
            CoreEffect::ToggleDiffBase => {
                navigation::toggle_diff_base(state, &mut update);
            }
            CoreEffect::ConnectionState(_) | CoreEffect::TransientError(_) => {}
        }
    }
    update
}

pub fn navigate_to_search_match(
    state: &mut AppState,
    view: &impl AppViewport,
    m: &crate::review_types::SearchMatch,
) -> AppOutput {
    navigation::navigate_to_search_match(state, view, m)
}

pub fn navigate_to_definition(
    state: &mut AppState,
    view: &impl AppViewport,
    def: &crate::review_types::DefinitionLocation,
) -> AppOutput {
    navigation::navigate_to_definition(state, view, def)
}
