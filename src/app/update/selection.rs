use super::cursor;
use super::output::AppOutput;
use super::viewport::ViewportMetrics;
use crate::app::document::{
    ColumnIndex, DocumentPosition, DocumentSourceLine, DocumentSourceLineEntry, RowIndex,
};
use crate::app::{AppState, CommentAnchorCapture, VisualSelection, VisualSelectionMode};
use crate::core::{TextAnchor, VisualSelectionEffect};
use crate::review_types::{
    AnchorMatchMethod, AnchorPlacementStatus, CommentAnchorSegment, CommentAnchorSide, PaneFocus,
    RenderVariant,
};

const CONTEXT_LINES: usize = 3;

pub fn apply_visual_selection_effect(
    state: &mut AppState,
    view: &impl ViewportMetrics,
    update: &mut AppOutput,
    effect: VisualSelectionEffect,
) {
    match effect {
        VisualSelectionEffect::StartLine => {
            let anchor = TextAnchor {
                line: state.diff_line_cursor,
                column: 0,
            };
            start_selection(state, VisualSelectionMode::Line, anchor);
            update.set_status("Line selection started");
        }
        VisualSelectionEffect::StartText { anchor } => {
            start_selection(state, VisualSelectionMode::Text, anchor);
        }
        VisualSelectionEffect::ExtendTo { anchor } => {
            extend_selection(state, anchor);
        }
        VisualSelectionEffect::Move(cursor_effect) => {
            cursor::apply_diff_cursor_effect(state, view, cursor_effect);
            let anchor = TextAnchor {
                line: state.diff_line_cursor,
                column: state.diff_col_cursor,
            };
            extend_selection(state, anchor);
        }
        VisualSelectionEffect::Cancel => {
            state.visual_selection = None;
            state.pending_comment_anchor = None;
            update.clear_status();
            state.mark_model_changed();
        }
        VisualSelectionEffect::Commit => {
            match capture_visual_selection(state) {
                Some(capture) => {
                    state.pending_comment_anchor = Some(capture);
                    update.set_status("Comment anchor captured");
                }
                None => {
                    update.set_status("No selectable diff text under cursor");
                }
            }
            state.mark_model_changed();
        }
    }
}

fn start_selection(state: &mut AppState, mode: VisualSelectionMode, anchor: TextAnchor) {
    state.pane_focus = PaneFocus::Diff;
    state.diff_line_cursor = anchor.line;
    state.diff_col_cursor = anchor.column;
    let position = document_position_from_anchor(anchor);
    state.visual_selection = Some(VisualSelection {
        mode,
        start: position,
        end: position,
    });
    state.pending_comment_anchor = None;
    state.mark_model_changed();
}

fn extend_selection(state: &mut AppState, anchor: TextAnchor) {
    state.pane_focus = PaneFocus::Diff;
    state.diff_line_cursor = anchor.line;
    state.diff_col_cursor = anchor.column;
    let position = document_position_from_anchor(anchor);
    if let Some(selection) = state.visual_selection.as_mut() {
        selection.end = position;
    } else {
        state.visual_selection = Some(VisualSelection {
            mode: VisualSelectionMode::Text,
            start: position,
            end: position,
        });
    }
    state.mark_model_changed();
}

fn document_position_from_anchor(anchor: TextAnchor) -> DocumentPosition {
    DocumentPosition {
        row: RowIndex(anchor.line),
        column: ColumnIndex(anchor.column),
    }
}

fn capture_visual_selection(state: &AppState) -> Option<CommentAnchorCapture> {
    let selection = state.visual_selection.as_ref()?;
    let file_path = state.selected_file_entry()?.change.path.clone();
    let source_lines = source_lines_for_active_document(state)?;
    if source_lines.is_empty() {
        return None;
    }

    let (start_anchor, end_anchor) = ordered_positions(selection.start, selection.end);
    let selection_lines = &source_lines;
    let start_row = start_anchor
        .row
        .0
        .min(selection_lines.len().saturating_sub(1));
    let end_row = end_anchor
        .row
        .0
        .min(selection_lines.len().saturating_sub(1));
    let selected_lines = normalized_selected_lines(state, selection_lines, start_row, end_row);

    let mut segments = Vec::new();
    for side in [CommentAnchorSide::Base, CommentAnchorSide::Head] {
        segments.extend(segments_for_selected_lines(
            state,
            &file_path,
            &selected_lines,
            side,
        ));
    }
    if segments.is_empty() {
        return None;
    }

    let line_start = selected_lines
        .iter()
        .flat_map(|line| line.entries.iter().map(|entry| entry.line_number))
        .min()
        .unwrap_or((start_row + 1) as i64);
    let line_end = selected_lines
        .iter()
        .flat_map(|line| line.entries.iter().map(|entry| entry.line_number))
        .max()
        .unwrap_or((end_row + 1) as i64);
    let anchor_text = selected_lines
        .iter()
        .map(|line| line.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let context_before = segments
        .iter()
        .find(|segment| segment.side == CommentAnchorSide::Head)
        .or_else(|| segments.first())
        .map(|segment| segment.context_before.clone())
        .unwrap_or_default();
    let context_after = segments
        .iter()
        .rev()
        .find(|segment| segment.side == CommentAnchorSide::Head)
        .or_else(|| segments.last())
        .map(|segment| segment.context_after.clone())
        .unwrap_or_default();

    Some(CommentAnchorCapture {
        segments,
        file_path,
        line_start,
        line_end,
        char_start: None,
        char_end: None,
        anchor_text,
        context_before,
        context_after,
    })
}

fn segments_for_selected_lines(
    state: &AppState,
    file_path: &str,
    selected_lines: &[DocumentSourceLine],
    side: CommentAnchorSide,
) -> Vec<CommentAnchorSegment> {
    let side_rows: Vec<(&DocumentSourceLineEntry, i64)> = selected_lines
        .iter()
        .flat_map(|line| {
            line.entries
                .iter()
                .filter(move |entry| entry.side == side)
                .map(move |entry| (entry, entry.line_number))
        })
        .collect();
    let content_lines: Vec<&str> = content_for_side(state, side)
        .map(|content| content.lines().collect())
        .unwrap_or_default();
    segment_from_side_rows(file_path, side, &content_lines, &side_rows)
        .into_iter()
        .collect()
}

fn segment_from_side_rows(
    file_path: &str,
    side: CommentAnchorSide,
    content_lines: &[&str],
    rows: &[(&DocumentSourceLineEntry, i64)],
) -> Option<CommentAnchorSegment> {
    let line_start = rows.iter().map(|(_, line)| *line).min()?;
    let line_end = rows.iter().map(|(_, line)| *line).max()?;
    let anchor_text = anchor_text_for_line_range(content_lines, line_start, line_end)
        .unwrap_or_else(|| {
            rows.iter()
                .map(|(entry, _)| entry.content.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        });

    Some(CommentAnchorSegment {
        side,
        file_path: file_path.to_string(),
        line_start,
        line_end,
        char_start: None,
        char_end: None,
        anchor_text,
        context_before: context_before(content_lines, line_start),
        context_after: context_after(content_lines, line_end),
        placement_status: AnchorPlacementStatus::Anchored,
        match_method: AnchorMatchMethod::ExactAtLine,
    })
}

fn anchor_text_for_line_range(lines: &[&str], line_start: i64, line_end: i64) -> Option<String> {
    let start = line_start
        .checked_sub(1)
        .and_then(|line| usize::try_from(line).ok())?;
    let end = usize::try_from(line_end).ok()?;
    if start >= end || end > lines.len() {
        return None;
    }
    Some(lines[start..end].join("\n"))
}

fn content_for_side(state: &AppState, side: CommentAnchorSide) -> Option<&str> {
    match side {
        CommentAnchorSide::Base => state.base_content.as_deref(),
        CommentAnchorSide::Head => state.head_content.as_deref(),
    }
}

fn context_before(lines: &[&str], line_start: i64) -> String {
    let idx = line_start
        .checked_sub(1)
        .and_then(|line| usize::try_from(line).ok())
        .unwrap_or(0);
    if idx > lines.len() {
        return String::new();
    }
    let from = idx.saturating_sub(CONTEXT_LINES);
    lines[from..idx.min(lines.len())].join("\n")
}

fn context_after(lines: &[&str], line_end: i64) -> String {
    let idx = line_end
        .checked_sub(1)
        .and_then(|line| usize::try_from(line).ok())
        .unwrap_or(0);
    if idx >= lines.len() {
        return String::new();
    }
    let from = idx.saturating_add(1);
    if from >= lines.len() {
        String::new()
    } else {
        let to = (from + CONTEXT_LINES).min(lines.len());
        lines[from..to].join("\n")
    }
}

fn ordered_positions(
    start: DocumentPosition,
    end: DocumentPosition,
) -> (DocumentPosition, DocumentPosition) {
    if (end.row, end.column) < (start.row, start.column) {
        (end, start)
    } else {
        (start, end)
    }
}

fn normalized_selected_lines(
    state: &AppState,
    selection_lines: &[DocumentSourceLine],
    start_row: usize,
    end_row: usize,
) -> Vec<DocumentSourceLine> {
    let selected = &selection_lines[start_row..=end_row];
    let has_change = selected.iter().any(|line| line.is_change);
    if !has_change {
        if state.render_variant == RenderVariant::SideBySide {
            let mut rows = Vec::new();
            for idx in end_row.saturating_add(1)..selection_lines.len() {
                collect_change_row(selection_lines, idx, &mut rows);
                if !rows.is_empty() {
                    rows.sort_by_key(|line| {
                        line.entries
                            .first()
                            .map(|entry| (entry.line_number, side_sort_key(entry.side)))
                            .unwrap_or((i64::MAX, 1))
                    });
                    rows.dedup();
                    return rows;
                }
            }
        }
        return selected.to_vec();
    }

    if change_group_count(selected) <= 1 {
        return selected.to_vec();
    }

    let mut rows = Vec::new();
    for idx in start_row..=end_row {
        collect_change_row(selection_lines, idx, &mut rows);
    }
    rows.sort_by_key(|line| {
        line.entries
            .first()
            .map(|entry| (entry.line_number, side_sort_key(entry.side)))
            .unwrap_or((i64::MAX, 1))
    });
    rows.dedup();
    rows
}

fn change_group_count(lines: &[DocumentSourceLine]) -> usize {
    let mut groups = 0usize;
    let mut in_group = false;
    for line in lines {
        if line.is_change {
            if !in_group {
                groups = groups.saturating_add(1);
                in_group = true;
            }
        } else {
            in_group = false;
        }
    }
    groups
}

fn side_sort_key(side: CommentAnchorSide) -> u8 {
    match side {
        CommentAnchorSide::Base => 0,
        CommentAnchorSide::Head => 1,
    }
}

fn collect_paired_change_row(
    selection_lines: &[DocumentSourceLine],
    idx: usize,
    rows: &mut Vec<DocumentSourceLine>,
) {
    let Some(line) = selection_lines.get(idx) else {
        return;
    };
    if line.entries.len() != 1 {
        return;
    }
    let side = line.entries[0].side;
    let pair_idx = match side {
        CommentAnchorSide::Base => idx.saturating_add(1),
        CommentAnchorSide::Head => idx.saturating_sub(1),
    };
    if let Some(pair) = selection_lines.get(pair_idx) {
        if pair.entries.len() == 1 && pair.entries[0].side != side {
            rows.push(line.clone());
            rows.push(pair.clone());
        }
    }
}

fn collect_change_row(
    selection_lines: &[DocumentSourceLine],
    idx: usize,
    rows: &mut Vec<DocumentSourceLine>,
) {
    let Some(line) = selection_lines.get(idx) else {
        return;
    };
    if line.is_change {
        rows.push(line.clone());
        return;
    }
    collect_paired_change_row(selection_lines, idx, rows);
}

fn source_lines_for_active_document(state: &AppState) -> Option<Vec<DocumentSourceLine>> {
    let active = state.active_document.as_ref()?;
    Some(active.document().diff.source_lines())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_types::LineKind;

    fn diff_line(
        kind: LineKind,
        content: &str,
        old_lineno: Option<u32>,
        new_lineno: Option<u32>,
    ) -> crate::review_types::DiffLine {
        crate::review_types::DiffLine {
            kind,
            content: content.to_string(),
            old_lineno,
            new_lineno,
        }
    }

    fn entry_for(line: &DocumentSourceLine, side: CommentAnchorSide) -> &DocumentSourceLineEntry {
        line.entries
            .iter()
            .find(|entry| entry.side == side)
            .unwrap_or_else(|| panic!("missing {side:?} entry in {line:?}"))
    }

    #[test]
    fn side_by_side_source_lines_come_from_active_document_rows() {
        let hunk = crate::review_types::DiffHunk {
            old_start: 26,
            old_lines: 1,
            new_start: 27,
            new_lines: 2,
            header: "@@ -26 +27,2 @@".to_string(),
            lines: vec![
                diff_line(LineKind::Deletion, "old line", Some(26), None),
                diff_line(LineKind::Addition, "new line one", None, Some(27)),
                diff_line(LineKind::Addition, "new line two", None, Some(28)),
            ],
        };
        let base_owned: Vec<String> = (1..=32).map(|n| format!("base {n}")).collect();
        let head_owned: Vec<String> = (1..=34).map(|n| format!("head {n}")).collect();
        let base_content = format!("{}\n", base_owned.join("\n"));
        let head_content = format!("{}\n", head_owned.join("\n"));
        let mut state = AppState::new(
            crate::config::DiffAlgorithm::Myers,
            crate::review_types::ConnectionContext {
                repo_root: "/repo".to_string(),
                worktree: "/repo".to_string(),
                base_ref: "main".to_string(),
                head_ref: "feature".to_string(),
                merge_base: "abc123".to_string(),
            },
            vec![crate::review_types::FileEntry {
                change: crate::review_types::FileChange {
                    path: "src/lib.rs".to_string(),
                    old_path: None,
                    kind: crate::review_types::ChangeKind::Modified,
                },
                status: crate::review_types::ReviewStatus::Unreviewed,
                diff: crate::review_types::DiffContent {
                    hunks: vec![hunk],
                    is_binary: false,
                    diff_hash: "diff".to_string(),
                    content_id: String::new(),
                },
            }],
            40,
        );
        state.render_variant = RenderVariant::SideBySide;
        state.base_content = Some(base_content);
        state.head_content = Some(head_content);
        state.rebuild_active_document();

        let rows = source_lines_for_active_document(&state).expect("active document rows");

        assert_eq!(rows.len(), 34);
        let replacement_start = &rows[26];
        let base = entry_for(replacement_start, CommentAnchorSide::Base);
        let head = entry_for(replacement_start, CommentAnchorSide::Head);
        assert_eq!(base.line_number, 26);
        assert_eq!(base.content, "old line");
        assert_eq!(head.line_number, 27);
        assert_eq!(head.content, "new line one");

        let replacement_tail = &rows[27];
        assert!(
            replacement_tail
                .entries
                .iter()
                .all(|entry| entry.side == CommentAnchorSide::Head)
        );
        let head = entry_for(replacement_tail, CommentAnchorSide::Head);
        assert_eq!(head.line_number, 28);
        assert_eq!(head.content, "new line two");
    }

    #[test]
    fn selected_non_contiguous_side_rows_coalesce_to_one_segment() {
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
        state.head_content = Some("one\ntwo\nthree\nfour\nfive\n".to_string());
        let selected_lines = vec![
            DocumentSourceLine {
                entries: vec![DocumentSourceLineEntry {
                    side: CommentAnchorSide::Head,
                    line_number: 2,
                    content: "two".to_string(),
                }],
                content: "two".to_string(),
                is_change: true,
            },
            DocumentSourceLine {
                entries: vec![DocumentSourceLineEntry {
                    side: CommentAnchorSide::Head,
                    line_number: 4,
                    content: "four".to_string(),
                }],
                content: "four".to_string(),
                is_change: true,
            },
        ];

        let segments = segments_for_selected_lines(
            &state,
            "src/lib.rs",
            &selected_lines,
            CommentAnchorSide::Head,
        );

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].line_start, 2);
        assert_eq!(segments[0].line_end, 4);
        assert_eq!(segments[0].anchor_text, "two\nthree\nfour");
    }
}
