//! Navigation rules shared by UI adapters.

use crate::model::FileEntry;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HunkJump {
    pub cursor: usize,
    pub scroll: usize,
}

pub fn map_new_to_old_line(entry: Option<&FileEntry>, new_line: usize) -> usize {
    let Some(entry) = entry else {
        return new_line;
    };

    let mut offset: i64 = 0;
    for hunk in &entry.diff.hunks {
        if (hunk.new_start as usize) > new_line {
            break;
        }
        offset = (hunk.old_start as i64 + hunk.old_lines as i64)
            - (hunk.new_start as i64 + hunk.new_lines as i64);
    }

    (new_line as i64 + offset).max(1) as usize
}

pub fn map_old_to_new_line(entry: Option<&FileEntry>, old_line: usize) -> usize {
    let Some(entry) = entry else {
        return old_line;
    };

    let mut offset: i64 = 0;
    for hunk in &entry.diff.hunks {
        if (hunk.old_start as usize) > old_line {
            break;
        }
        offset = (hunk.new_start as i64 + hunk.new_lines as i64)
            - (hunk.old_start as i64 + hunk.old_lines as i64);
    }

    (old_line as i64 + offset).max(1) as usize
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

pub fn jump_to_next_hunk(
    current_cursor: usize,
    current_scroll: usize,
    view_height: usize,
    first_change_rows: &[usize],
    hunk_end_rows: &[usize],
) -> Option<HunkJump> {
    let idx = first_change_rows.iter().position(|&r| r > current_cursor)?;
    jump_to_hunk(
        idx,
        current_scroll,
        view_height,
        first_change_rows,
        hunk_end_rows,
    )
}

pub fn jump_to_prev_hunk(
    current_cursor: usize,
    current_scroll: usize,
    view_height: usize,
    first_change_rows: &[usize],
    hunk_end_rows: &[usize],
) -> Option<HunkJump> {
    let idx = first_change_rows
        .iter()
        .rposition(|&r| r < current_cursor)?;
    jump_to_hunk(
        idx,
        current_scroll,
        view_height,
        first_change_rows,
        hunk_end_rows,
    )
}

fn jump_to_hunk(
    idx: usize,
    current_scroll: usize,
    view_height: usize,
    first_change_rows: &[usize],
    hunk_end_rows: &[usize],
) -> Option<HunkJump> {
    let cursor = *first_change_rows.get(idx)?;
    let hunk_end = hunk_end_rows.get(idx).copied().unwrap_or(cursor + 1);
    let scroll = scroll_to_show_hunk(cursor, hunk_end, current_scroll, view_height);
    Some(HunkJump { cursor, scroll })
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
    use crate::model::{ChangeKind, DiffContent, DiffHunk, DiffLine, FileChange, ReviewStatus};

    fn entry() -> FileEntry {
        FileEntry {
            change: FileChange {
                path: "a.rs".to_string(),
                old_path: None,
                kind: ChangeKind::Modified,
            },
            status: ReviewStatus::Unreviewed,
            diff: DiffContent {
                hunks: vec![DiffHunk {
                    old_start: 10,
                    old_lines: 5,
                    new_start: 10,
                    new_lines: 3,
                    header: String::new(),
                    lines: vec![DiffLine {
                        kind: crate::model::LineKind::Context,
                        content: String::new(),
                        old_lineno: Some(10),
                        new_lineno: Some(10),
                    }],
                }],
                is_binary: false,
                diff_hash: "hash".to_string(),
            },
        }
    }

    #[test]
    fn maps_new_and_old_lines_across_hunk_offsets() {
        let file = entry();

        assert_eq!(map_new_to_old_line(Some(&file), 20), 22);
        assert_eq!(map_old_to_new_line(Some(&file), 20), 18);
    }

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

    #[test]
    fn hunk_jump_selects_next_change_and_preserves_visible_scroll() {
        let jump = jump_to_next_hunk(3, 0, 10, &[5, 20], &[8, 25]).unwrap();

        assert_eq!(
            jump,
            HunkJump {
                cursor: 5,
                scroll: 0
            }
        );
    }

    #[test]
    fn hunk_jump_scrolls_large_hunk_to_top() {
        let jump = jump_to_next_hunk(3, 0, 5, &[10], &[30]).unwrap();

        assert_eq!(
            jump,
            HunkJump {
                cursor: 10,
                scroll: 10
            }
        );
    }
}
