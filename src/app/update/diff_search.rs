use super::cursor::clamp_cursor_and_scroll;
use super::output::AppOutput;
use super::viewport::AppViewport;
use crate::app::AppState;
use crate::app::document::{ColumnIndex, DocumentPosition, RowIndex};
use crate::core::navigation::Direction;

pub fn apply_diff_search(
    state: &mut AppState,
    view: &impl AppViewport,
    update: &mut AppOutput,
    query: String,
) {
    if query.is_empty() {
        state.diff_search_query = None;
        state.diff_search_matches.clear();
        state.diff_search_current = 0;
    } else {
        state.diff_search_query = Some(query);
        if let Some(err) = recompute_diff_search_matches(state, view) {
            state.diff_search_query = None;
            update.set_status(err);
        } else if state.diff_search_matches.is_empty() {
            update.set_status("No matches");
        } else {
            diff_search_jump_to_current(state, view);
            let total = state.diff_search_matches.len();
            let cur = state.diff_search_current + 1;
            update.set_status(format!("{cur}/{total}"));
        }
    }
    state.mark_model_changed();
}

pub fn navigate_diff_search_match(
    state: &mut AppState,
    view: &impl AppViewport,
    update: &mut AppOutput,
    direction: Direction,
) {
    if state.diff_search_query.is_none() || state.diff_search_matches.is_empty() {
        return;
    }

    let cursor = state.diff_line_cursor;
    let cursor_col = state.diff_col_cursor;
    let len = state.diff_search_matches.len();
    let idx = match direction {
        Direction::Next => state
            .diff_search_matches
            .iter()
            .position(|(row, start, _)| *row > cursor || (*row == cursor && *start > cursor_col))
            .unwrap_or(0),
        Direction::Prev => state
            .diff_search_matches
            .iter()
            .rposition(|(row, start, _)| *row < cursor || (*row == cursor && *start < cursor_col))
            .unwrap_or(len - 1),
    };

    state.diff_search_current = idx;
    set_cursor_to_match(state, view, idx);
    clamp_cursor_and_scroll(state, view);
    state.mark_model_changed();
    let cur = idx + 1;
    update.set_status(format!("{cur}/{len}"));
}

pub fn clear_diff_search(state: &mut AppState) {
    state.diff_search_query = None;
    state.diff_search_matches.clear();
    state.diff_search_current = 0;
    state.mark_model_changed();
}

pub fn refresh_active_diff_search(state: &mut AppState, view: &impl AppViewport) -> bool {
    if state.diff_search_query.is_none() {
        return false;
    }

    let previous_matches = state.diff_search_matches.clone();
    let previous_current = state.diff_search_current;
    if recompute_diff_search_matches(state, view).is_some() {
        return false;
    }

    let matches_changed = state.diff_search_matches != previous_matches;
    if matches_changed && !state.diff_search_matches.is_empty() {
        diff_search_jump_to_current(state, view);
    }

    let changed = matches_changed || state.diff_search_current != previous_current;
    if changed {
        state.mark_model_changed();
    }
    changed
}

fn recompute_diff_search_matches(state: &mut AppState, view: &impl AppViewport) -> Option<String> {
    state.diff_search_matches.clear();
    state.diff_search_current = 0;
    let query = match &state.diff_search_query {
        Some(q) if !q.is_empty() => q.clone(),
        _ => return None,
    };
    let _re = match regex::RegexBuilder::new(&query)
        .case_insensitive(true)
        .build()
    {
        Ok(re) => re,
        Err(e) => {
            let msg = e.to_string();
            let short = msg
                .lines()
                .next()
                .unwrap_or(&msg)
                .trim_start_matches("regex parse error:")
                .trim();
            return Some(format!("Invalid regex: {short}"));
        }
    };
    let current = DocumentPosition {
        row: RowIndex(state.diff_line_cursor),
        column: ColumnIndex(state.diff_col_cursor),
    };
    let Some(active) = state.active_document.as_mut() else {
        return None;
    };
    active
        .document_mut()
        .diff
        .search_with_current(query.clone(), Some(current));
    for match_ in active.document().diff.all_search_matches() {
        state.diff_search_matches.push((
            match_.row.0,
            match_.columns.start.0,
            match_.columns.end.0,
        ));
    }
    let _ = view;
    None
}

fn diff_search_jump_to_current(state: &mut AppState, view: &impl AppViewport) {
    if state.diff_search_matches.is_empty() {
        return;
    }
    let idx = state
        .diff_search_matches
        .iter()
        .position(|(row, _, _)| *row >= state.diff_line_cursor)
        .unwrap_or(0);
    state.diff_search_current = idx;
    set_cursor_to_match(state, view, idx);
    clamp_cursor_and_scroll(state, view);
}

fn set_cursor_to_match(state: &mut AppState, view: &impl AppViewport, idx: usize) {
    let Some((row, start, _)) = state.diff_search_matches.get(idx).copied() else {
        return;
    };
    state.diff_line_cursor = row;
    state.diff_col_cursor = start;
    let _ = view;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_types::{
        ChangeKind, ConnectionContext, DiffContent, DiffHunk, DiffLine, FileChange, FileEntry,
        LineKind, ReviewStatus,
    };

    struct View {
        lines: Vec<String>,
    }

    impl AppViewport for View {
        fn diff_content_height(&self) -> usize {
            self.lines.len()
        }

        fn diff_view_height(&self) -> usize {
            self.lines.len()
        }
    }

    fn state_with_lines(lines: Vec<DiffLine>) -> AppState {
        let mut state = AppState::new(
            crate::config::DiffAlgorithm::Myers,
            ConnectionContext {
                repo_root: "/repo".to_string(),
                worktree: "/repo".to_string(),
                base_ref: "main".to_string(),
                head_ref: "feature".to_string(),
                merge_base: "abc123".to_string(),
            },
            vec![FileEntry {
                change: FileChange {
                    path: "src/lib.rs".to_string(),
                    old_path: None,
                    kind: ChangeKind::Modified,
                },
                status: ReviewStatus::Unreviewed,
                diff: DiffContent {
                    hunks: vec![DiffHunk {
                        old_start: 1,
                        old_lines: lines.len() as u32,
                        new_start: 1,
                        new_lines: lines.len() as u32,
                        header: "@@ -1 +1 @@".to_string(),
                        lines,
                    }],
                    is_binary: false,
                    diff_hash: "diff".to_string(),
                },
            }],
            40,
        );
        state.diff_search_query = Some("specific".to_string());
        state.rebuild_active_document();
        state
    }

    fn context_line(content: &str, line: u32) -> DiffLine {
        DiffLine {
            kind: LineKind::Context,
            content: content.to_string(),
            old_lineno: Some(line),
            new_lineno: Some(line),
        }
    }

    #[test]
    fn diff_search_uses_document_text_not_viewport_rendered_text() {
        let mut state = state_with_lines(vec![context_line("on specific lines.", 1)]);
        let view = View {
            lines: vec!["viewport text is ignored".to_string()],
        };

        assert!(recompute_diff_search_matches(&mut state, &view).is_none());

        assert_eq!(state.diff_search_matches.len(), 1);
        let (_, start, end) = state.diff_search_matches[0];
        assert_eq!((start, end), (3, 11));
    }

    #[test]
    fn diff_search_navigation_visits_multiple_matches_on_same_line() {
        let mut state = state_with_lines(vec![
            context_line("specific and specific here", 1),
            context_line("later specific", 2),
        ]);
        let view = View { lines: Vec::new() };
        assert!(recompute_diff_search_matches(&mut state, &view).is_none());
        assert_eq!(state.diff_search_matches.len(), 3);
        state.diff_search_current = 0;
        state.diff_line_cursor = 0;
        state.diff_col_cursor = 0;

        let mut update = AppOutput::default();
        navigate_diff_search_match(&mut state, &view, &mut update, Direction::Next);

        assert_eq!(state.diff_search_current, 1);
        assert_eq!(state.diff_line_cursor, 0);
        assert_eq!(state.diff_col_cursor, 13);

        navigate_diff_search_match(&mut state, &view, &mut update, Direction::Prev);

        assert_eq!(state.diff_search_current, 0);
        assert_eq!(state.diff_line_cursor, 0);
        assert_eq!(state.diff_col_cursor, 0);
    }
}
