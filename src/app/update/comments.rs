use super::cursor::clamp_cursor_and_scroll;
use super::output::AppOutput;
use super::viewport::AppViewport;
use crate::app::{AppState, FileListSectionFocus};
use crate::core::CommentsPanelEffect;
use crate::review_types::{Comment, ContentMode, LineKind, PaneFocus, RenderVariant};

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
    current_comment_for_line(
        &state.comments,
        path,
        i64::from(line),
        state.selected_comment_id,
    )
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

pub fn navigate_to_comment_id(state: &mut AppState, view: &impl AppViewport, comment_id: i64) {
    let Some(comment) = state
        .comments
        .iter()
        .find(|comment| comment.id == comment_id)
        .cloned()
    else {
        return;
    };
    navigate_to_comment(state, view, &comment);
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
    let comments = current_file_comments(state);
    if comments.is_empty() {
        return;
    }
    let Some(cursor_line) = current_head_line_for_navigation(state) else {
        return;
    };
    let cursor_line = i64::from(cursor_line);
    let next_index = match direction {
        Direction::Next => comments
            .iter()
            .position(|comment| comment.line_start > cursor_line)
            .unwrap_or(0),
        Direction::Previous => comments
            .iter()
            .rposition(|comment| comment.line_start < cursor_line)
            .unwrap_or(comments.len() - 1),
    };
    let comment = comments[next_index].clone();
    state.selected_comment_id = Some(comment.id);
    navigate_to_comment(state, view, &comment);
}

fn select_adjacent(state: &mut AppState, direction: Direction) {
    let comments = current_file_comments(state);
    if comments.is_empty() {
        state.selected_comment_id = None;
    } else if let Some(cursor_line) = current_head_line_for_navigation(state).map(i64::from) {
        let next_index = match direction {
            Direction::Next => comments
                .iter()
                .position(|comment| comment.line_start > cursor_line)
                .unwrap_or(0),
            Direction::Previous => comments
                .iter()
                .rposition(|comment| comment.line_start < cursor_line)
                .unwrap_or(comments.len() - 1),
        };
        state.selected_comment_id = Some(comments[next_index].id);
    }
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
    let Some(file_index) = state
        .files
        .iter()
        .position(|entry| entry.change.path == comment.file_path)
    else {
        select_out_of_range_comment(state, comment.id);
        return;
    };
    let focus = if state.file_list_section_focus == FileListSectionFocus::UnresolvedComments {
        FileListSectionFocus::UnresolvedComments
    } else {
        state.focus_for_file(file_index)
    };
    state.select_file(file_index, focus, false);
    state.selected_comment_id = Some(comment.id);
    state.pane_focus = PaneFocus::Diff;
    let start_row = display_row_for_head_line(state, comment_line(comment.line_start));
    let end_row = display_row_for_head_line(state, comment_line(comment.line_end)).max(start_row);
    state.diff_line_cursor = start_row;
    state.diff_col_cursor = 0;
    scroll_to_comment(state, view, start_row, end_row);
    clamp_cursor_and_scroll(state, view);
    state.mark_model_changed();
}

fn comment_line(line: i64) -> u32 {
    u32::try_from(line.max(1)).unwrap_or(u32::MAX)
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

fn current_comment_for_line<'a>(
    comments: &'a [Comment],
    path: &str,
    line: i64,
    selected_comment_id: Option<i64>,
) -> Option<&'a Comment> {
    let candidates: Vec<&Comment> = comments
        .iter()
        .filter(|comment| {
            comment.file_path == path && comment.line_start <= line && comment.line_end >= line
        })
        .collect();
    let max_start = candidates.iter().map(|comment| comment.line_start).max()?;
    if let Some(selected) = selected_comment_id.and_then(|id| {
        candidates
            .iter()
            .copied()
            .find(|comment| comment.id == id && comment.line_start == max_start)
    }) {
        return Some(selected);
    }
    candidates
        .into_iter()
        .filter(|comment| comment.line_start == max_start)
        .max_by_key(|comment| comment.id)
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

pub(super) fn current_head_line_for_navigation(state: &AppState) -> Option<u32> {
    current_head_line(state).or_else(|| match state.content_mode {
        ContentMode::Diff => diff_insertion_head_line_before_row(state, state.diff_line_cursor),
        ContentMode::FullFile => None,
    })
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
    if state.render_variant == RenderVariant::SideBySide {
        return side_by_side_new_line_at_row(state, row);
    }
    inline_new_line_at_row(state, row)
}

fn inline_new_line_at_row(state: &AppState, row: usize) -> Option<u32> {
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
    if state.render_variant == RenderVariant::SideBySide {
        return side_by_side_row_for_new_line(state, target_line);
    }
    inline_row_for_new_line(state, target_line)
}

fn inline_row_for_new_line(state: &AppState, target_line: u32) -> Option<usize> {
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

fn side_by_side_new_line_at_row(state: &AppState, row: usize) -> Option<u32> {
    find_side_by_side_row(state, |diff_row| {
        (diff_row.display_row == row).then_some(diff_row.new_lineno)
    })
    .flatten()
}

fn side_by_side_row_for_new_line(state: &AppState, target_line: u32) -> Option<usize> {
    find_side_by_side_row(state, |diff_row| {
        (diff_row.new_lineno == Some(target_line)).then_some(diff_row.display_row)
    })
}

#[derive(Debug, Clone, Copy)]
struct SideBySideDiffRow {
    display_row: usize,
    new_lineno: Option<u32>,
    insertion_head_line: Option<u32>,
}

fn find_side_by_side_row<T>(
    state: &AppState,
    mut f: impl FnMut(SideBySideDiffRow) -> Option<T>,
) -> Option<T> {
    let entry = state.selected_file_entry()?;
    let head_lines = state
        .head_content
        .as_deref()
        .map(|content| content.lines().count())
        .unwrap_or(0);
    if entry.diff.hunks.is_empty() {
        for row in 0..head_lines {
            let diff_row = SideBySideDiffRow {
                display_row: row,
                new_lineno: Some(row.saturating_add(1) as u32),
                insertion_head_line: None,
            };
            if let Some(value) = f(diff_row) {
                return Some(value);
            }
        }
        return None;
    }

    let mut display_row = 0usize;
    let mut new_cursor = 1u32;
    for hunk in &entry.diff.hunks {
        while new_cursor < hunk.new_start && (new_cursor as usize) <= head_lines {
            let diff_row = SideBySideDiffRow {
                display_row,
                new_lineno: Some(new_cursor),
                insertion_head_line: None,
            };
            if let Some(value) = f(diff_row) {
                return Some(value);
            }
            display_row = display_row.saturating_add(1);
            new_cursor = new_cursor.saturating_add(1);
        }

        let mut index = 0;
        while index < hunk.lines.len() {
            let line = &hunk.lines[index];
            if line.kind == LineKind::Context {
                let diff_row = SideBySideDiffRow {
                    display_row,
                    new_lineno: line.new_lineno,
                    insertion_head_line: None,
                };
                if let Some(value) = f(diff_row) {
                    return Some(value);
                }
                display_row = display_row.saturating_add(1);
                new_cursor = new_cursor.saturating_add(1);
                index += 1;
                continue;
            }

            let block_start = index;
            let mut del_end = index;
            while del_end < hunk.lines.len() && hunk.lines[del_end].kind == LineKind::Deletion {
                del_end += 1;
            }
            let mut add_end = del_end;
            while add_end < hunk.lines.len() && hunk.lines[add_end].kind == LineKind::Addition {
                add_end += 1;
            }

            let deletion_count = del_end.saturating_sub(block_start);
            let additions = &hunk.lines[del_end..add_end];
            let max_count = deletion_count.max(additions.len());
            for offset in 0..max_count {
                let new_lineno = additions.get(offset).and_then(|line| line.new_lineno);
                let diff_row = SideBySideDiffRow {
                    display_row,
                    new_lineno,
                    insertion_head_line: new_lineno
                        .is_none()
                        .then_some(new_cursor.saturating_sub(1)),
                };
                if let Some(value) = f(diff_row) {
                    return Some(value);
                }
                display_row = display_row.saturating_add(1);
                if new_lineno.is_some() {
                    new_cursor = new_cursor.saturating_add(1);
                }
            }
            index = add_end;
        }
    }

    while (new_cursor as usize) <= head_lines {
        let diff_row = SideBySideDiffRow {
            display_row,
            new_lineno: Some(new_cursor),
            insertion_head_line: None,
        };
        if let Some(value) = f(diff_row) {
            return Some(value);
        }
        display_row = display_row.saturating_add(1);
        new_cursor = new_cursor.saturating_add(1);
    }
    None
}

fn diff_insertion_head_line_before_row(state: &AppState, row: usize) -> Option<u32> {
    let entry = state.selected_file_entry()?;
    let head_lines = state
        .head_content
        .as_deref()
        .map(|content| content.lines().count())
        .unwrap_or(0);
    if entry.diff.hunks.is_empty() {
        return None;
    }
    if state.render_variant == RenderVariant::SideBySide {
        return find_side_by_side_row(state, |diff_row| {
            (diff_row.display_row == row && diff_row.new_lineno.is_none())
                .then_some(diff_row.insertion_head_line)
        })
        .flatten();
    }

    let mut display_row = 0usize;
    let mut new_cursor = 1u32;
    for hunk in &entry.diff.hunks {
        while new_cursor < hunk.new_start && (new_cursor as usize) <= head_lines {
            display_row = display_row.saturating_add(1);
            new_cursor = new_cursor.saturating_add(1);
        }

        let mut index = 0;
        while index < hunk.lines.len() {
            let line = &hunk.lines[index];
            if line.kind == LineKind::Context {
                display_row = display_row.saturating_add(1);
                new_cursor = new_cursor.saturating_add(1);
                index += 1;
                continue;
            }

            let block_start = index;
            let mut del_end = index;
            while del_end < hunk.lines.len() && hunk.lines[del_end].kind == LineKind::Deletion {
                del_end += 1;
            }
            let mut add_end = del_end;
            while add_end < hunk.lines.len() && hunk.lines[add_end].kind == LineKind::Addition {
                add_end += 1;
            }

            let deletion_count = del_end.saturating_sub(block_start);
            let additions = &hunk.lines[del_end..add_end];
            for _ in 0..deletion_count {
                if display_row == row {
                    return Some(new_cursor.saturating_sub(1));
                }
                display_row = display_row.saturating_add(1);
            }
            for _ in additions {
                display_row = display_row.saturating_add(1);
                new_cursor = new_cursor.saturating_add(1);
            }
            index = add_end;
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Next,
    Previous,
}
