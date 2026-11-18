use super::comments;
use super::viewport::ViewportMetrics;
use crate::app::{AppState, FileListSectionFocus};
use crate::core::{PaneEffect, PaneId};
use crate::review_types::{PaneFocus, ReviewStatus};

pub fn apply_pane_effect(state: &mut AppState, view: &impl ViewportMetrics, effect: PaneEffect) {
    match effect {
        PaneEffect::ActivateFileListSelection => {
            if state.file_list_section_focus == FileListSectionFocus::UnresolvedComments {
                if let Some(comment_id) = state.selected_comment_id {
                    comments::navigate_to_comment_id(state, view, comment_id);
                    return;
                }
            }
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
                let focus = state.focus_for_file(file_index);
                state.select_file(file_index, focus, true);
            }
        }
        PaneEffect::SelectComment { comment_id } => {
            state.file_list_section_focus = FileListSectionFocus::UnresolvedComments;
            comments::navigate_to_comment_id(state, view, comment_id);
        }
    }
}

pub fn toggle_pane_focus(state: &mut AppState) {
    let mut panes = Vec::new();
    if state.show_file_list {
        panes.push(PaneFocus::FileList);
    }
    if state.show_diff_pane {
        panes.push(PaneFocus::Diff);
    }
    if state.show_comments_panel && state.show_diff_pane {
        panes.push(PaneFocus::Comments);
    }
    if panes.len() <= 1 {
        return;
    }
    let current = panes
        .iter()
        .position(|pane| *pane == state.pane_focus)
        .unwrap_or(0);
    state.pane_focus = panes[(current + 1) % panes.len()];
    state.mark_model_changed();
}

/// Toggle visibility of a pane. At least one pane must remain visible.
pub fn toggle_pane_visibility(state: &mut AppState, pane: PaneId) {
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
        PaneId::Comments => {
            state.show_comments_panel = !state.show_comments_panel;
            if state.show_comments_panel {
                state.pane_focus = PaneFocus::Comments;
            } else if state.pane_focus == PaneFocus::Comments {
                state.pane_focus = PaneFocus::Diff;
            }
            state.mark_model_changed();
        }
        _ => {}
    }
}
