use super::cursor::clamp_cursor_and_scroll;
use super::output::AppOutput;
use super::viewport::AppViewport;
use crate::app::document::{RowIndex, RowSpan};
use crate::app::model::CommentProjection;
use crate::app::{AppState, FileListSectionFocus};
use crate::core::CommentsPanelEffect;
use crate::core::navigation::Direction;
use crate::review_types::{Comment, PaneFocus};

pub fn current_comment(state: &AppState) -> Option<&Comment> {
    if state.pane_focus == PaneFocus::Comments {
        if let Some(id) = state.selected_comment_id {
            if let Some(comment) = state.comments.iter().find(|comment| comment.id == id) {
                return Some(comment);
            }
        }
    }

    let comment_id = state
        .active_document
        .as_ref()
        .and_then(|document| document.diff.current_comment_id())
        .or_else(|| {
            let path = selected_path(state)?;
            let line = CommentProjection::current_visible_line(state)?;
            CommentProjection::current_comment_id_for_line(
                &state.comments,
                path,
                i64::from(line),
                CommentProjection::current_comment_side(state),
                state.selected_comment_id,
            )
        })?;
    state
        .comments
        .iter()
        .find(|comment| comment.id == comment_id)
}

pub fn current_comment_id(state: &AppState) -> Option<i64> {
    current_comment(state).map(|comment| comment.id)
}

pub fn apply_comments_panel_effect(
    state: &mut AppState,
    view: &impl AppViewport,
    update: &mut AppOutput,
    effect: CommentsPanelEffect,
) {
    match effect {
        CommentsPanelEffect::Toggle => toggle_panel(state, update),
        CommentsPanelEffect::SelectNext => select_adjacent(state, Direction::Next),
        CommentsPanelEffect::SelectPrevious => select_adjacent(state, Direction::Prev),
        CommentsPanelEffect::ToggleExpanded => toggle_selected_expanded_or_navigate(state, view),
        CommentsPanelEffect::NavigateToSelected => navigate_to_selected(state, view),
        CommentsPanelEffect::NavigateNextComment => {
            navigate_adjacent_comment(state, view, Direction::Next)
        }
        CommentsPanelEffect::NavigatePreviousComment => {
            navigate_adjacent_comment(state, view, Direction::Prev);
        }
        CommentsPanelEffect::EditCurrent => {
            if current_comment(state).is_none() {
                update.set_status("No comment at cursor");
            }
        }
        CommentsPanelEffect::ToggleResolvedCurrent => toggle_resolved_current(state, update),
        CommentsPanelEffect::RequestDeleteCurrent => request_delete_current(state, update),
    }
}

fn toggle_panel(state: &mut AppState, update: &mut AppOutput) {
    state.show_comments_panel = !state.show_comments_panel;
    if state.show_comments_panel {
        ensure_selected_comment(state);
        update.set_status("Comments panel: shown");
    } else {
        if state.pane_focus == PaneFocus::Comments {
            state.pane_focus = PaneFocus::Diff;
        }
        update.set_status("Comments panel: hidden");
    }
    state.mark_model_changed();
}

fn toggle_selected_expanded_or_navigate(state: &mut AppState, view: &impl AppViewport) {
    let Some(comment) = selected_comment(state).cloned() else {
        return;
    };
    if comment.resolved {
        if !state.expanded_comment_ids.insert(comment.id) {
            state.expanded_comment_ids.remove(&comment.id);
        }
        state.mark_model_changed();
    } else {
        navigate_to_comment(state, view, &comment);
    }
}

fn navigate_to_selected(state: &mut AppState, view: &impl AppViewport) {
    let Some(comment) = selected_comment(state).cloned() else {
        return;
    };
    navigate_to_comment(state, view, &comment);
}

pub fn navigate_to_comment_id(
    state: &mut AppState,
    view: &impl AppViewport,
    comment_id: i64,
) -> bool {
    let Some(comment) = state
        .comments
        .iter()
        .find(|comment| comment.id == comment_id)
        .cloned()
    else {
        return false;
    };
    navigate_to_comment(state, view, &comment)
}

pub fn select_out_of_range_comment(state: &mut AppState, comment_id: i64) {
    state.selected_comment_id = Some(comment_id);
    state.show_comments_panel = true;
    state.pane_focus = PaneFocus::Comments;
    state.pending_delete_comment_id = None;
    state.mark_model_changed();
}

fn navigate_adjacent_comment(state: &mut AppState, view: &impl AppViewport, direction: Direction) {
    if !state.show_comments_panel {
        state.show_comments_panel = true;
        state.mark_model_changed();
    }
    state.ensure_active_document();
    let row = RowIndex(state.diff_line_cursor);
    let Some(comment_id) = state
        .active_document
        .as_ref()
        .and_then(|document| document.diff.next_comment(row, direction))
        .map(|comment| comment.id)
    else {
        return;
    };
    navigate_to_comment_id(state, view, comment_id);
}

fn select_adjacent(state: &mut AppState, direction: Direction) {
    state.ensure_active_document();
    state.selected_comment_id = state
        .active_document
        .as_ref()
        .and_then(|document| {
            document
                .diff
                .next_comment(RowIndex(state.diff_line_cursor), direction)
        })
        .map(|comment| comment.id);
    state.pending_delete_comment_id = None;
    state.mark_model_changed();
}

fn toggle_resolved_current(state: &mut AppState, update: &mut AppOutput) {
    let Some(comment) = current_comment(state).cloned() else {
        update.set_status("No comment at cursor");
        return;
    };
    if comment.resolved {
        update.request_comment_unresolve(comment.id);
        update.set_status(format!("Unresolving comment #{}", comment.id));
    } else {
        update.request_comment_resolve(comment.id);
        update.set_status(format!("Resolving comment #{}", comment.id));
    }
}

fn request_delete_current(state: &mut AppState, update: &mut AppOutput) {
    let Some(comment) = current_comment(state).cloned() else {
        update.set_status("No comment at cursor");
        return;
    };
    if state.pending_delete_comment_id == Some(comment.id) {
        update.request_comment_delete(comment.id);
        update.set_status(format!("Deleting comment #{}", comment.id));
    } else {
        state.pending_delete_comment_id = Some(comment.id);
        state.mark_model_changed();
        update.set_status(format!("Press d again to delete comment #{}", comment.id));
    }
}

fn navigate_to_comment(state: &mut AppState, view: &impl AppViewport, comment: &Comment) -> bool {
    let Some(file_index) = state
        .files
        .iter()
        .position(|entry| entry.change.path == comment.file_path())
    else {
        select_out_of_range_comment(state, comment.id);
        return false;
    };
    let focus = if state.file_list_section_focus == FileListSectionFocus::UnresolvedComments {
        FileListSectionFocus::UnresolvedComments
    } else {
        state.focus_for_file(file_index)
    };
    state.select_file(file_index, focus, false);
    state.selected_comment_id = Some(comment.id);
    state.ensure_active_document();
    let Some(span) = comment_display_span(state, comment) else {
        state.mark_model_changed();
        return false;
    };
    state.pane_focus = PaneFocus::Diff;
    state.diff_line_cursor = span.start.0;
    state.diff_col_cursor = 0;
    scroll_to_comment(state, view, span.start.0, span.end.0);
    clamp_cursor_and_scroll(state, view);
    state.mark_model_changed();
    true
}

fn scroll_to_comment(
    state: &mut AppState,
    view: &impl AppViewport,
    start_row: usize,
    end_row: usize,
) {
    let view_height = view.diff_view_height();
    if view_height == 0 {
        return;
    }

    let comment_height = end_row.saturating_sub(start_row).saturating_add(1);
    let scroll = if comment_height <= view_height {
        let max_full_view_scroll = view.diff_content_height().saturating_sub(view_height);
        end_row
            .saturating_add(2)
            .saturating_sub(view_height)
            .min(max_full_view_scroll)
    } else {
        let max_start_offset = view_height / 2;
        start_row
            .saturating_sub(max_start_offset)
            .min(view.max_diff_scroll())
    };

    state.diff_scroll = scroll;
}

pub fn ensure_selected_comment(state: &mut AppState) {
    state.selected_comment_id = current_comment_id(state);
}

fn selected_comment(state: &AppState) -> Option<&Comment> {
    let id = state.selected_comment_id?;
    state.comments.iter().find(|comment| comment.id == id)
}

fn selected_path(state: &AppState) -> Option<&str> {
    state
        .selected_file_entry()
        .map(|entry| entry.change.path.as_str())
}

fn comment_display_span(state: &AppState, comment: &Comment) -> Option<RowSpan> {
    state
        .active_document
        .as_ref()
        .and_then(|document| document.diff.comment_span(comment.id))
        .or_else(|| {
            CommentProjection::display_rows_for_comment(state, comment).map(|(start, end)| {
                RowSpan {
                    start: RowIndex(start),
                    end: RowIndex(end),
                }
            })
        })
}
