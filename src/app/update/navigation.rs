use super::comments;
use super::cursor::clamp_cursor_and_scroll;
use super::output::AppOutput;
use super::viewport::AppViewport;
use crate::app::diff_rows::{inline_diff_rows, side_by_side_diff_rows};
use crate::app::document::RowIndex;
use crate::app::{AppState, FileListSectionFocus, JumpLocation};
use crate::core::command::Command;
use crate::core::navigation::{self as core_navigation, Direction, FileNavigationScope};
use crate::core::search as core_search;
use crate::core::text::suffix_from_char;
use crate::review_types::{CommentAnchorSide, ContentMode, PaneFocus, RenderVariant, ReviewStatus};

pub fn navigate_to_search_match(
    state: &mut AppState,
    view: &impl AppViewport,
    m: &crate::review_types::SearchMatch,
) -> AppOutput {
    let mut update = AppOutput::handled();
    let target = core_search::resolve_search_target(&state.files, m);
    navigate_to_location_target(state, view, &mut update, target);
    update
}

pub fn navigate_to_definition(
    state: &mut AppState,
    view: &impl AppViewport,
    def: &crate::review_types::DefinitionLocation,
) -> AppOutput {
    let mut update = AppOutput::handled();
    let target = core_search::resolve_definition_target(&state.files, def);
    navigate_to_location_target(state, view, &mut update, target);
    update
}

pub fn navigate_to_location_target(
    state: &mut AppState,
    view: &impl AppViewport,
    update: &mut AppOutput,
    target: core_search::LocationTarget,
) {
    match target {
        core_search::LocationTarget::InDiff {
            file_index,
            line_number,
        } => {
            push_jump_stack(state);
            let focus = state.focus_for_file(file_index);
            state.select_file(file_index, focus, false);
            state.diff_line_cursor = (line_number as usize).saturating_sub(1);
            clamp_cursor_and_scroll(state, view);
            state.mark_model_changed();
        }
        core_search::LocationTarget::External {
            file_path,
            line_number,
        } => {
            push_jump_stack(state);
            update.request_command(Command::ViewFile {
                path: file_path,
                line_number,
            });
        }
    }
}

fn push_jump_stack(state: &mut AppState) {
    state.jump_stack.push(JumpLocation {
        file_index: state.selected_file,
        diff_scroll: state.diff_scroll,
        diff_line_cursor: state.diff_line_cursor,
        content_mode: state.content_mode,
        render_variant: state.render_variant,
    });
}

pub fn toggle_inline_diff(state: &mut AppState, update: &mut AppOutput) {
    if state.content_mode == ContentMode::Diff {
        let previous = state.render_variant;
        let target = current_diff_source_line(state);
        state.render_variant = match state.render_variant {
            RenderVariant::Inline => RenderVariant::SideBySide,
            RenderVariant::SideBySide => RenderVariant::Inline,
            other => other,
        };
        if state.render_variant == RenderVariant::SideBySide && state.base_content.is_none() {
            state.load_base_content();
        }
        if state.render_variant != previous {
            if let Some(row) = target.and_then(|target| row_for_diff_source_line(state, target)) {
                state.diff_line_cursor = row;
                state.diff_scroll = state.diff_scroll.min(state.diff_line_cursor);
                state.diff_col_cursor = 0;
            }
            state.invalidate_diff_search_matches();
            state.mark_model_changed();
        }
    }
    update.clear_status();
}

#[derive(Debug, Clone, Copy)]
struct DiffSourceLine {
    side: CommentAnchorSide,
    line: u32,
}

fn current_diff_source_line(state: &AppState) -> Option<DiffSourceLine> {
    let entry = state.selected_file_entry()?;
    match state.render_variant {
        RenderVariant::Inline => {
            let rows = inline_diff_rows(&entry.diff.hunks, state.head_content.as_deref());
            let row = rows.rows.get(state.diff_line_cursor)?;
            row.new_lineno
                .map(|line| DiffSourceLine {
                    side: CommentAnchorSide::Head,
                    line,
                })
                .or_else(|| {
                    row.old_lineno.map(|line| DiffSourceLine {
                        side: CommentAnchorSide::Base,
                        line,
                    })
                })
        }
        RenderVariant::SideBySide => {
            let rows = side_by_side_diff_rows(
                &entry.diff.hunks,
                state.base_content.as_deref(),
                state.head_content.as_deref(),
            );
            let row = rows.rows.get(state.diff_line_cursor)?;
            row.head
                .as_ref()
                .map(|cell| DiffSourceLine {
                    side: CommentAnchorSide::Head,
                    line: cell.line_number,
                })
                .or_else(|| {
                    row.base.as_ref().map(|cell| DiffSourceLine {
                        side: CommentAnchorSide::Base,
                        line: cell.line_number,
                    })
                })
        }
        _ => None,
    }
}

fn row_for_diff_source_line(state: &AppState, target: DiffSourceLine) -> Option<usize> {
    let entry = state.selected_file_entry()?;
    match state.render_variant {
        RenderVariant::Inline => {
            let rows = inline_diff_rows(&entry.diff.hunks, state.head_content.as_deref());
            rows.rows.iter().position(|row| match target.side {
                CommentAnchorSide::Base => row.old_lineno == Some(target.line),
                CommentAnchorSide::Head => row.new_lineno == Some(target.line),
            })
        }
        RenderVariant::SideBySide => {
            let rows = side_by_side_diff_rows(
                &entry.diff.hunks,
                state.base_content.as_deref(),
                state.head_content.as_deref(),
            );
            rows.rows.iter().position(|row| match target.side {
                CommentAnchorSide::Base => row
                    .base
                    .as_ref()
                    .is_some_and(|cell| cell.line_number == target.line),
                CommentAnchorSide::Head => row
                    .head
                    .as_ref()
                    .is_some_and(|cell| cell.line_number == target.line),
            })
        }
        _ => None,
    }
}

pub fn toggle_blame(state: &mut AppState, update: &mut AppOutput) {
    state.show_blame = !state.show_blame;
    state.load_blame();
    let status = if state.show_blame {
        "Blame: shown"
    } else {
        "Blame: hidden"
    };
    update.set_status(status);
}

pub fn toggle_whitespace_ignored(state: &mut AppState, update: &mut AppOutput) {
    state.ignore_whitespace = !state.ignore_whitespace;
    state.reload_current_diff();
    state.invalidate_diff_search_matches();
    let status = if state.ignore_whitespace {
        "Whitespace: ignored"
    } else {
        "Whitespace: shown"
    };
    update.set_status(status);
}

pub fn cycle_diff_algorithm(state: &mut AppState, update: &mut AppOutput) {
    state.diff_algorithm = state.diff_algorithm.next();
    state.reload_current_diff();
    update.set_status(format!("Diff algorithm: {}", state.diff_algorithm.label()));
    update.request_layout_save();
}

pub fn toggle_diff_base(state: &mut AppState, update: &mut AppOutput) {
    let has_reviewed_commit = state.selected_file_entry().is_some_and(|e| {
        matches!(
            &e.status,
            ReviewStatus::Reviewed {
                reviewed_commit: Some(_),
                ..
            } | ReviewStatus::Changed {
                reviewed_commit: Some(_),
                ..
            }
        )
    });
    if has_reviewed_commit {
        state.show_merge_base = !state.show_merge_base;
        state.reload_current_diff();
        let label = if state.show_merge_base {
            "Diff base: merge base"
        } else {
            "Diff base: since review"
        };
        update.set_status(label);
    } else {
        update.set_status("File not yet reviewed");
    }
}

/// Pop the jump stack and restore the previous location.
pub fn pop_jump_stack(state: &mut AppState, view: &impl AppViewport, update: &mut AppOutput) {
    if let Some(loc) = state.jump_stack.pop() {
        if loc.file_index != state.selected_file && loc.file_index < state.files.len() {
            let focus = state.focus_for_file(loc.file_index);
            state.select_file(loc.file_index, focus, false);
        }
        state.diff_scroll = loc.diff_scroll;
        state.diff_line_cursor = loc.diff_line_cursor;
        state.content_mode = loc.content_mode;
        state.render_variant = loc.render_variant;
        clamp_cursor_and_scroll(state, view);
        state.mark_model_changed();
        update.set_status(format!("Jump stack: {} remaining", state.jump_stack.len()));
    } else {
        update.set_status("Jump stack empty");
    }
}

/// Request go-to-definition for the word under the cursor.
pub fn request_go_to_definition(
    state: &mut AppState,
    view: &impl AppViewport,
    update: &mut AppOutput,
) {
    let word = extract_word_at_cursor(state, view);
    if let Some(word) = word {
        if word.is_empty() {
            update.set_status("No word under cursor");
        } else {
            update.request_command(Command::FindDefinition { symbol: word });
        }
    } else {
        update.set_status("No word under cursor");
    }
}

/// Extract the identifier-like word under the current diff column cursor.
pub fn extract_word_at_cursor(state: &AppState, view: &impl AppViewport) -> Option<String> {
    let content = content_for_word_extraction(state, view)?;
    word_at_char_offset(content, state.diff_col_cursor)
}

fn content_for_word_extraction<'a>(
    state: &AppState,
    view: &'a impl AppViewport,
) -> Option<&'a str> {
    let line = view.diff_rendered_text().get(state.diff_line_cursor)?;
    let content_start = view.diff_content_start_col();
    suffix_from_char(line, content_start).map(str::trim)
}

fn word_at_char_offset(content: &str, column: usize) -> Option<String> {
    let chars: Vec<char> = content.chars().collect();
    let ch = *chars.get(column)?;
    if !is_identifier_char(ch) {
        return None;
    }

    let mut start = column;
    while start > 0 && is_identifier_char(chars[start - 1]) {
        start -= 1;
    }

    let mut end = column;
    while end + 1 < chars.len() && is_identifier_char(chars[end + 1]) {
        end += 1;
    }

    Some(chars[start..=end].iter().collect())
}

fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub fn cycle_view_mode(state: &mut AppState, view: &impl AppViewport) {
    let approx_line = estimate_current_line(state, view);

    match (&state.content_mode, &state.render_variant) {
        (ContentMode::Diff, _) => {
            state.content_mode = ContentMode::FullFile;
            state.render_variant = RenderVariant::HeadVersion;
            let row = approx_line.saturating_sub(1);
            state.diff_line_cursor = row;
            state.diff_scroll = row;
            state.invalidate_diff_search_matches();
        }
        (ContentMode::FullFile, RenderVariant::HeadVersion) => {
            if state.base_content.is_none() {
                state.load_base_content();
            }
            state.render_variant = RenderVariant::BaseVersion;
            let base_line =
                core_navigation::map_new_to_old_line(state.selected_file_entry(), approx_line);
            let row = base_line.saturating_sub(1);
            state.diff_line_cursor = row;
            state.diff_scroll = row;
            state.invalidate_diff_search_matches();
        }
        (ContentMode::FullFile, RenderVariant::BaseVersion) => {
            state.content_mode = ContentMode::Diff;
            state.render_variant = RenderVariant::Inline;
            let old_line = state.diff_line_cursor + 1;
            let new_line =
                core_navigation::map_old_to_new_line(state.selected_file_entry(), old_line);
            let row = new_line.saturating_sub(1);
            state.diff_line_cursor = row;
            state.diff_scroll = row;
            state.invalidate_diff_search_matches();
        }
        _ => {
            state.content_mode = ContentMode::Diff;
            state.render_variant = RenderVariant::Inline;
            state.invalidate_diff_search_matches();
        }
    }
    state.mark_model_changed();
}

fn estimate_current_line(state: &AppState, view: &impl AppViewport) -> usize {
    match state.content_mode {
        ContentMode::Diff => {
            let scroll = state.diff_line_cursor;
            if let Some(entry) = state.selected_file_entry() {
                let mut deletions_above = 0usize;
                for (i, &start) in view.hunk_start_rows().iter().enumerate() {
                    let end = view.hunk_end_rows().get(i).copied().unwrap_or(start);
                    if start > scroll {
                        break;
                    }
                    let hunk = &entry.diff.hunks[i.min(entry.diff.hunks.len() - 1)];
                    let hunk_scroll_end = scroll.min(end);
                    for (j, line) in hunk.lines.iter().enumerate() {
                        if start + j > hunk_scroll_end {
                            break;
                        }
                        if line.kind == crate::review_types::LineKind::Deletion
                            && start + j <= scroll
                        {
                            deletions_above += 1;
                        }
                    }
                }
                scroll.saturating_sub(deletions_above) + 1
            } else {
                scroll + 1
            }
        }
        ContentMode::FullFile => state.diff_line_cursor + 1,
    }
}

pub fn jump_to_next_hunk(state: &mut AppState, view: &impl AppViewport) {
    if let Some(jump) = core_navigation::jump_to_next_hunk(
        state.diff_line_cursor,
        state.diff_scroll,
        view.diff_view_height(),
        view.hunk_first_change_rows(),
        view.hunk_end_rows(),
    ) {
        state.diff_line_cursor = jump.cursor;
        state.diff_scroll = jump.scroll;
        state.diff_col_cursor = 0;
        clamp_cursor_and_scroll(state, view);
        state.mark_model_changed();
    }
}

pub fn jump_to_prev_hunk(state: &mut AppState, view: &impl AppViewport) {
    if let Some(jump) = core_navigation::jump_to_prev_hunk(
        state.diff_line_cursor,
        state.diff_scroll,
        view.diff_view_height(),
        view.hunk_first_change_rows(),
        view.hunk_end_rows(),
    ) {
        state.diff_line_cursor = jump.cursor;
        state.diff_scroll = jump.scroll;
        state.diff_col_cursor = 0;
        clamp_cursor_and_scroll(state, view);
        state.mark_model_changed();
    }
}

pub fn navigate_file(state: &mut AppState, view: &impl AppViewport, dir: Direction) {
    if state.file_list_section_focus == FileListSectionFocus::UnresolvedComments {
        navigate_unresolved_comment(state, view, dir);
        return;
    }

    let scope = if state.pane_focus == PaneFocus::Diff {
        FileNavigationScope::DiffPane
    } else {
        FileNavigationScope::FileListPane
    };

    if let Some(new_idx) = core_navigation::navigate_file(
        state.selected_file,
        state.files.len(),
        state.unreviewed_count(),
        scope,
        dir,
    ) {
        let focus = state.focus_for_file(new_idx);
        state.select_file(new_idx, focus, true);
    }
}

fn navigate_unresolved_comment(state: &mut AppState, view: &impl AppViewport, dir: Direction) {
    let targets = unresolved_comment_targets(state);
    if targets.is_empty() {
        state.selected_comment_id = None;
        state.mark_model_changed();
        return;
    }

    let current = state
        .selected_comment_id
        .and_then(|id| targets.iter().position(|target| target.comment_id == id))
        .or_else(|| {
            targets
                .iter()
                .position(|target| target.file_index == Some(state.selected_file))
        })
        .unwrap_or(0);
    let next = match dir {
        Direction::Next => (current + 1) % targets.len(),
        Direction::Prev => {
            if current == 0 {
                targets.len() - 1
            } else {
                current - 1
            }
        }
    };
    activate_unresolved_comment(state, view, targets[next].comment_id);
}

pub fn navigate_unresolved_comment_from_cursor(
    state: &mut AppState,
    view: &impl AppViewport,
    dir: Direction,
) -> bool {
    let targets = unresolved_comment_targets(state);
    if targets.is_empty() {
        state.selected_comment_id = None;
        state.mark_model_changed();
        return false;
    }

    if let Some(comment_id) = state
        .active_document
        .as_ref()
        .and_then(|document| {
            document
                .diff
                .next_unresolved_comment(RowIndex(state.diff_line_cursor), dir)
        })
        .map(|comment| comment.id)
        .filter(|comment_id| Some(*comment_id) != state.selected_comment_id)
    {
        activate_unresolved_comment(state, view, comment_id);
        return true;
    }

    if let Some(target) = next_unresolved_file_target(state, &targets, dir) {
        activate_unresolved_comment_target(state, view, target, dir);
    }
    true
}

pub fn selected_comment_visible_in_current_view(state: &AppState) -> bool {
    let Some(id) = state.selected_comment_id else {
        return true;
    };
    state
        .active_document
        .as_ref()
        .is_none_or(|document| document.diff.comment_span(id).is_some())
}

fn activate_unresolved_comment(state: &mut AppState, view: &impl AppViewport, comment_id: i64) {
    state.file_list_section_focus = FileListSectionFocus::UnresolvedComments;
    state.show_comments_panel = true;
    state.pending_delete_comment_id = None;
    comments::navigate_to_comment_id(state, view, comment_id);
}

fn activate_unresolved_comment_target(
    state: &mut AppState,
    view: &impl AppViewport,
    target: UnresolvedCommentTarget,
    dir: Direction,
) {
    state.file_list_section_focus = FileListSectionFocus::UnresolvedComments;
    state.show_comments_panel = true;
    state.pending_delete_comment_id = None;

    let Some(file_index) = target.file_index else {
        comments::navigate_to_comment_id(state, view, target.comment_id);
        return;
    };

    state.select_file(file_index, FileListSectionFocus::UnresolvedComments, false);
    state.selected_comment_id = None;
    let comment_id = state
        .active_document
        .as_ref()
        .and_then(|document| match dir {
            Direction::Next => document.diff.first_unresolved_comment(),
            Direction::Prev => document.diff.last_unresolved_comment(),
        })
        .map(|comment| comment.id)
        .unwrap_or(target.comment_id);
    comments::navigate_to_comment_id(state, view, comment_id);
}

fn next_unresolved_file_target(
    state: &AppState,
    targets: &[UnresolvedCommentTarget],
    dir: Direction,
) -> Option<UnresolvedCommentTarget> {
    match dir {
        Direction::Next => targets
            .iter()
            .find(|target| target.order_index > state.selected_file)
            .copied()
            .or_else(|| targets.first().copied()),
        Direction::Prev => targets
            .iter()
            .rev()
            .find(|target| target.order_index < state.selected_file)
            .copied()
            .or_else(|| targets.last().copied()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UnresolvedCommentTarget {
    file_index: Option<usize>,
    order_index: usize,
    comment_id: i64,
}

fn unresolved_comment_targets(state: &AppState) -> Vec<UnresolvedCommentTarget> {
    let mut targets = Vec::new();
    let mut current_file_paths = std::collections::HashSet::new();
    for (file_index, entry) in state.files.iter().enumerate() {
        current_file_paths.insert(entry.change.path.as_str());
        let comments: Vec<_> = state
            .comments
            .iter()
            .filter(|comment| !comment.resolved && comment.file_path() == entry.change.path)
            .collect();
        targets.extend(comments.into_iter().map(|comment| UnresolvedCommentTarget {
            file_index: Some(file_index),
            order_index: file_index,
            comment_id: comment.id,
        }));
    }
    let mut external_paths: Vec<&str> = state
        .comments
        .iter()
        .filter(|comment| !comment.resolved && !current_file_paths.contains(comment.file_path()))
        .map(|comment| comment.file_path())
        .collect();
    external_paths.sort_unstable();
    external_paths.dedup();
    let external_start = state.files.len();
    for (offset, path) in external_paths.into_iter().enumerate() {
        let comments: Vec<_> = state
            .comments
            .iter()
            .filter(|comment| !comment.resolved && comment.file_path() == path)
            .collect();
        targets.extend(comments.into_iter().map(|comment| UnresolvedCommentTarget {
            file_index: None,
            order_index: external_start + offset,
            comment_id: comment.id,
        }));
    }
    targets
}

pub fn navigate_file_section(state: &mut AppState, dir: Direction) {
    let order = [
        FileListSectionFocus::Unreviewed,
        FileListSectionFocus::Reviewed,
        FileListSectionFocus::UnresolvedComments,
    ];
    let current = order
        .iter()
        .position(|section| *section == state.file_list_section_focus)
        .unwrap_or(0);

    for offset in 1..=order.len() {
        let index = match dir {
            Direction::Next => (current + offset) % order.len(),
            Direction::Prev => (current + order.len() - offset) % order.len(),
        };
        let section = order[index];
        if section == FileListSectionFocus::UnresolvedComments {
            let Some(target) = unresolved_comment_targets(state).first().copied() else {
                continue;
            };
            if let Some(file_index) = target.file_index {
                state.select_file(file_index, section, true);
            } else {
                state.file_list_section_focus = section;
                state.mark_model_changed();
            }
            state.selected_comment_id = Some(target.comment_id);
            state.mark_model_changed();
            return;
        }
        let Some(file_index) = first_file_in_section(state, section) else {
            continue;
        };
        state.select_file(file_index, section, true);
        state.selected_comment_id = None;
        state.pending_delete_comment_id = None;
        state.mark_model_changed();
        return;
    }
}

fn first_file_in_section(state: &AppState, section: FileListSectionFocus) -> Option<usize> {
    match section {
        FileListSectionFocus::Unreviewed => (state.unreviewed_count() > 0).then_some(0),
        FileListSectionFocus::Reviewed => {
            let start = state.unreviewed_count();
            (start < state.files.len()).then_some(start)
        }
        FileListSectionFocus::UnresolvedComments => unresolved_comment_targets(state)
            .first()
            .and_then(|target| target.file_index),
    }
}

pub fn toggle_review(state: &AppState, update: &mut AppOutput) {
    if state.files.get(state.selected_file).is_some() {
        update.request_review_toggle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_at_char_offset_selects_identifier_under_cursor() {
        assert_eq!(
            word_at_char_offset("let first = second_value;", 12),
            Some("second_value".to_string())
        );
    }

    #[test]
    fn word_at_char_offset_selects_identifier_from_underscore() {
        assert_eq!(
            word_at_char_offset("call merge_base now", 10),
            Some("merge_base".to_string())
        );
    }

    #[test]
    fn word_at_char_offset_returns_none_on_separator() {
        assert_eq!(word_at_char_offset("foo.bar", 3), None);
    }

    #[test]
    fn word_at_char_offset_uses_character_offsets() {
        assert_eq!(
            word_at_char_offset("let caf\u{e9}_value = 1", 6),
            Some("caf\u{e9}_value".to_string())
        );
    }

    #[test]
    fn word_extraction_skips_multibyte_comment_marker_gutter() {
        struct View {
            lines: Vec<String>,
        }

        impl AppViewport for View {
            fn hunk_start_rows(&self) -> &[usize] {
                &[]
            }

            fn hunk_end_rows(&self) -> &[usize] {
                &[]
            }

            fn hunk_first_change_rows(&self) -> &[usize] {
                &[]
            }

            fn diff_gutter_cols(&self) -> usize {
                8
            }

            fn diff_content_height(&self) -> usize {
                self.lines.len()
            }

            fn diff_view_height(&self) -> usize {
                self.lines.len()
            }

            fn diff_rendered_text(&self) -> &[String] {
                &self.lines
            }
        }

        let mut state = AppState::new(
            crate::config::DiffAlgorithm::Myers,
            crate::review_types::ConnectionContext {
                repo_root: "/repo".to_string(),
                worktree: "/repo".to_string(),
                base_ref: "main".to_string(),
                head_ref: "feature".to_string(),
                merge_base: "abc123".to_string(),
            },
            Vec::new(),
            40,
        );
        state.diff_line_cursor = 0;
        state.diff_col_cursor = 0;

        let view = View {
            lines: vec![" 25  25┃   on specific lines.".to_string()],
        };

        assert_eq!(extract_word_at_cursor(&state, &view).as_deref(), Some("on"));
    }
}
