use super::cursor::clamp_cursor_and_scroll;
use super::output::AppOutput;
use super::viewport::AppViewport;
use crate::app::AppState;
use crate::core::CommentsPanelEffect;
use crate::review_types::{Comment, ContentMode, PaneFocus, RenderVariant};

pub fn current_comment(state: &AppState) -> Option<&Comment> {
    if state.pane_focus == PaneFocus::Comments {
        if let Some(id) = state.selected_comment_id {
            if let Some(comment) = state.comments.iter().find(|comment| comment.id == id) {
                return Some(comment);
            }
        }
    }

    let path = selected_path(state)?;
    let line = current_head_line(state)?;
    state
        .comments
        .iter()
        .filter(|comment| {
            comment.file_path == path
                && comment.line_start <= i64::from(line)
                && comment.line_end >= i64::from(line)
        })
        .min_by_key(|comment| {
            (
                comment.line_end.saturating_sub(comment.line_start),
                comment.id,
            )
        })
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
        CommentsPanelEffect::SelectPrevious => select_adjacent(state, Direction::Previous),
        CommentsPanelEffect::ToggleExpanded => toggle_selected_expanded_or_navigate(state, view),
        CommentsPanelEffect::NavigateToSelected => navigate_to_selected(state, view),
        CommentsPanelEffect::NavigateNextComment => {
            navigate_adjacent_comment(state, view, Direction::Next)
        }
        CommentsPanelEffect::NavigatePreviousComment => {
            navigate_adjacent_comment(state, view, Direction::Previous);
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

fn navigate_adjacent_comment(state: &mut AppState, view: &impl AppViewport, direction: Direction) {
    let comments = current_file_comments(state);
    if comments.is_empty() {
        return;
    }
    let selected = current_comment_id(state).or(state.selected_comment_id);
    let current_index = selected
        .and_then(|id| comments.iter().position(|comment| comment.id == id))
        .unwrap_or(0);
    let next_index = match direction {
        Direction::Next => (current_index + 1) % comments.len(),
        Direction::Previous => current_index.checked_sub(1).unwrap_or(comments.len() - 1),
    };
    let comment = comments[next_index].clone();
    state.selected_comment_id = Some(comment.id);
    navigate_to_comment(state, view, &comment);
}

fn select_adjacent(state: &mut AppState, direction: Direction) {
    let comments = current_file_comments(state);
    if comments.is_empty() {
        state.selected_comment_id = None;
        state.mark_model_changed();
        return;
    }
    let current_index = state
        .selected_comment_id
        .and_then(|id| comments.iter().position(|comment| comment.id == id))
        .unwrap_or(0);
    let next_index = match direction {
        Direction::Next => (current_index + 1) % comments.len(),
        Direction::Previous => current_index.checked_sub(1).unwrap_or(comments.len() - 1),
    };
    state.selected_comment_id = Some(comments[next_index].id);
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

fn navigate_to_comment(state: &mut AppState, view: &impl AppViewport, comment: &Comment) {
    if let Some(file_index) = state
        .files
        .iter()
        .position(|entry| entry.change.path == comment.file_path)
    {
        if file_index != state.selected_file {
            state.selected_file = file_index;
            state.on_file_changed();
        }
    }
    state.selected_comment_id = Some(comment.id);
    state.pane_focus = PaneFocus::Diff;
    state.diff_line_cursor = display_row_for_head_line(state, comment.line_start as u32);
    state.diff_col_cursor = 0;
    clamp_cursor_and_scroll(state, view);
    state.mark_model_changed();
}

pub fn ensure_selected_comment(state: &mut AppState) {
    let selected_is_current_file = state.selected_comment_id.is_some_and(|id| {
        current_file_comments(state)
            .iter()
            .any(|comment| comment.id == id)
    });
    if selected_is_current_file {
        return;
    }
    state.selected_comment_id = current_comment_id(state).or_else(|| {
        current_file_comments(state)
            .first()
            .map(|comment| comment.id)
    });
}

fn selected_comment(state: &AppState) -> Option<&Comment> {
    let id = state.selected_comment_id?;
    state.comments.iter().find(|comment| comment.id == id)
}

fn current_file_comments(state: &AppState) -> Vec<&Comment> {
    let Some(path) = selected_path(state) else {
        return Vec::new();
    };
    let mut comments: Vec<&Comment> = state
        .comments
        .iter()
        .filter(|comment| comment.file_path == path)
        .collect();
    comments.sort_by_key(|comment| (comment.line_start, comment.line_end, comment.id));
    comments
}

fn selected_path(state: &AppState) -> Option<&str> {
    state
        .selected_file_entry()
        .map(|entry| entry.change.path.as_str())
}

fn current_head_line(state: &AppState) -> Option<u32> {
    match state.content_mode {
        ContentMode::FullFile => match state.render_variant {
            RenderVariant::HeadVersion => Some(state.diff_line_cursor.saturating_add(1) as u32),
            RenderVariant::BaseVersion => None,
            _ => None,
        },
        ContentMode::Diff => diff_new_line_at_row(state, state.diff_line_cursor),
    }
}

fn display_row_for_head_line(state: &AppState, target_line: u32) -> usize {
    match state.content_mode {
        ContentMode::FullFile => match state.render_variant {
            RenderVariant::HeadVersion => target_line.saturating_sub(1) as usize,
            RenderVariant::BaseVersion => target_line.saturating_sub(1) as usize,
            _ => target_line.saturating_sub(1) as usize,
        },
        ContentMode::Diff => diff_row_for_new_line(state, target_line)
            .unwrap_or_else(|| target_line.saturating_sub(1) as usize),
    }
}

fn diff_new_line_at_row(state: &AppState, row: usize) -> Option<u32> {
    let entry = state.selected_file_entry()?;
    let head_lines = state
        .head_content
        .as_deref()
        .map(|content| content.lines().count())
        .unwrap_or(0);
    if entry.diff.hunks.is_empty() {
        return (row < head_lines).then_some(row.saturating_add(1) as u32);
    }

    let mut display_row = 0usize;
    let mut new_cursor = 1u32;
    for hunk in &entry.diff.hunks {
        while new_cursor < hunk.new_start && (new_cursor as usize) <= head_lines {
            if display_row == row {
                return Some(new_cursor);
            }
            display_row = display_row.saturating_add(1);
            new_cursor = new_cursor.saturating_add(1);
        }

        for line in &hunk.lines {
            if display_row == row {
                return line.new_lineno;
            }
            display_row = display_row.saturating_add(1);
            if line.new_lineno.is_some() {
                new_cursor = new_cursor.saturating_add(1);
            }
        }
    }

    while (new_cursor as usize) <= head_lines {
        if display_row == row {
            return Some(new_cursor);
        }
        display_row = display_row.saturating_add(1);
        new_cursor = new_cursor.saturating_add(1);
    }
    None
}

fn diff_row_for_new_line(state: &AppState, target_line: u32) -> Option<usize> {
    let entry = state.selected_file_entry()?;
    let head_lines = state
        .head_content
        .as_deref()
        .map(|content| content.lines().count())
        .unwrap_or(0);
    if entry.diff.hunks.is_empty() {
        return ((target_line as usize) <= head_lines)
            .then_some(target_line.saturating_sub(1) as usize);
    }

    let mut display_row = 0usize;
    let mut new_cursor = 1u32;
    for hunk in &entry.diff.hunks {
        while new_cursor < hunk.new_start && (new_cursor as usize) <= head_lines {
            if new_cursor == target_line {
                return Some(display_row);
            }
            display_row = display_row.saturating_add(1);
            new_cursor = new_cursor.saturating_add(1);
        }

        for line in &hunk.lines {
            if line.new_lineno == Some(target_line) {
                return Some(display_row);
            }
            display_row = display_row.saturating_add(1);
            if line.new_lineno.is_some() {
                new_cursor = new_cursor.saturating_add(1);
            }
        }
    }

    while (new_cursor as usize) <= head_lines {
        if new_cursor == target_line {
            return Some(display_row);
        }
        display_row = display_row.saturating_add(1);
        new_cursor = new_cursor.saturating_add(1);
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Next,
    Previous,
}
