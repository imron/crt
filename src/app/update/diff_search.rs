use super::cursor::clamp_cursor_and_scroll;
use super::output::AppOutput;
use super::viewport::AppViewport;
use crate::app::AppState;
use crate::core::navigation::Direction;
use crate::core::text::char_to_byte_index;

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
    let cursor_col = view.diff_content_start_col() + state.diff_col_cursor;
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
    let re = match regex::RegexBuilder::new(&query)
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
    for (row, line) in view.diff_rendered_text().iter().enumerate() {
        let search_start =
            char_to_byte_index(line, view.diff_content_start_col()).unwrap_or(line.len());
        let content = &line[search_start..];
        for m in re.find_iter(content) {
            if m.start() == m.end() {
                continue;
            }
            let abs_start = search_start + m.start();
            let abs_end = search_start + m.end();
            state.diff_search_matches.push((row, abs_start, abs_end));
        }
    }
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
    state.diff_col_cursor = start.saturating_sub(view.diff_content_start_col());
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn state() -> AppState {
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
        state.diff_search_query = Some("specific".to_string());
        state
    }

    #[test]
    fn diff_search_skips_multibyte_comment_marker_gutter() {
        let mut state = state();
        let view = View {
            lines: vec![" 25  25┃   on specific lines.".to_string()],
        };

        assert!(recompute_diff_search_matches(&mut state, &view).is_none());

        assert_eq!(state.diff_search_matches.len(), 1);
        let (_, start, end) = state.diff_search_matches[0];
        assert_eq!(&view.lines[0][start..end], "specific");
    }

    #[test]
    fn diff_search_navigation_visits_multiple_matches_on_same_line() {
        let mut state = state();
        let view = View {
            lines: vec![
                "           specific and specific here".to_string(),
                "           later specific".to_string(),
            ],
        };
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
