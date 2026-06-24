use crate::app::AppState;
use crate::core::{PaneEffect, PaneId};
use crate::review_types::{PaneFocus, ReviewStatus};

pub(super) fn apply_pane_effect(state: &mut AppState, effect: PaneEffect) {
    match effect {
        PaneEffect::ActivateFileListSelection => {
            if state.show_diff_pane {
                state.pane_focus = PaneFocus::Diff;
                state.mark_model_changed();
            }
        }
        PaneEffect::ActivateDiffSelection => {
            if let Some(entry) = state.selected_file_entry() {
                if matches!(entry.status, ReviewStatus::Reviewed { .. })
                    && !state.reviewed_diff_expanded
                {
                    state.reviewed_diff_expanded = true;
                    state.diff_scroll = 0;
                    state.mark_model_changed();
                }
            }
        }
        PaneEffect::SelectFile { file_index } => {
            if state.files.get(file_index).is_some() {
                if file_index != state.selected_file {
                    state.selected_file = file_index;
                    state.on_file_changed();
                }
            }
        }
    }
}

pub(super) fn toggle_pane_focus(state: &mut AppState) {
    if state.show_file_list && state.show_diff_pane {
        state.pane_focus = match state.pane_focus {
            PaneFocus::FileList => PaneFocus::Diff,
            PaneFocus::Diff => PaneFocus::FileList,
        };
        state.mark_model_changed();
    }
}

/// Toggle visibility of a pane. At least one pane must remain visible.
pub(super) fn toggle_pane_visibility(state: &mut AppState, pane: PaneId) {
    match pane {
        PaneId::FileList => {
            if state.show_file_list {
                if state.show_diff_pane {
                    state.show_file_list = false;
                    state.pane_focus = PaneFocus::Diff;
                    state.mark_model_changed();
                }
            } else {
                state.show_file_list = true;
                state.mark_model_changed();
            }
        }
        PaneId::Diff => {
            if state.show_diff_pane {
                if state.show_file_list {
                    state.show_diff_pane = false;
                    state.pane_focus = PaneFocus::FileList;
                    state.mark_model_changed();
                }
            } else {
                state.show_diff_pane = true;
                state.mark_model_changed();
            }
        }
        _ => {}
    }
}
