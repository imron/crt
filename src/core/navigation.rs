//! Navigation rules shared by UI adapters.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Next,
    Prev,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileNavigationScope {
    DiffPane,
    FileListPane,
}

pub fn navigate_file(
    selected_file: usize,
    total_files: usize,
    unreviewed_count: usize,
    scope: FileNavigationScope,
    direction: Direction,
) -> Option<usize> {
    if total_files == 0 {
        return None;
    }

    let (range_start, range_end) = match scope {
        FileNavigationScope::DiffPane => {
            if unreviewed_count == 0 {
                (0, total_files)
            } else {
                (0, unreviewed_count)
            }
        }
        FileNavigationScope::FileListPane => {
            if selected_file < unreviewed_count {
                (0, unreviewed_count.max(1))
            } else {
                (unreviewed_count, total_files)
            }
        }
    };

    let range_len = range_end.saturating_sub(range_start);
    if range_len == 0 {
        return None;
    }

    let pos = selected_file.saturating_sub(range_start);
    let new_pos = match direction {
        Direction::Next => (pos + 1) % range_len,
        Direction::Prev => {
            if pos == 0 {
                range_len - 1
            } else {
                pos - 1
            }
        }
    };

    let new_idx = range_start + new_pos;
    (new_idx != selected_file).then_some(new_idx)
}

pub fn scroll_to_show_hunk(
    first_row: usize,
    hunk_end: usize,
    current_scroll: usize,
    view_height: usize,
) -> usize {
    if view_height == 0 {
        return current_scroll;
    }

    let hunk_size = hunk_end.saturating_sub(first_row);
    let viewport_end = current_scroll + view_height;

    if first_row >= current_scroll && hunk_end <= viewport_end {
        return current_scroll;
    }

    let centered_scroll = first_row.saturating_sub(view_height / 2);
    let visible_end = centered_scroll + view_height;

    if hunk_end <= visible_end || hunk_size >= view_height {
        if hunk_size >= view_height {
            first_row
        } else {
            centered_scroll
        }
    } else {
        hunk_end.saturating_sub(view_height).min(first_row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_navigation_cycles_unreviewed_in_diff_pane() {
        assert_eq!(
            navigate_file(1, 5, 3, FileNavigationScope::DiffPane, Direction::Next),
            Some(2)
        );
        assert_eq!(
            navigate_file(2, 5, 3, FileNavigationScope::DiffPane, Direction::Next),
            Some(0)
        );
    }

    #[test]
    fn file_navigation_scopes_to_reviewed_section_in_file_list() {
        assert_eq!(
            navigate_file(3, 5, 2, FileNavigationScope::FileListPane, Direction::Prev,),
            Some(2)
        );
        assert_eq!(
            navigate_file(2, 5, 2, FileNavigationScope::FileListPane, Direction::Prev,),
            Some(4)
        );
    }
}
