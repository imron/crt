use super::cursor;
use super::output::AppOutput;
use super::viewport::AppViewport;
use crate::app::diff_rows::{
    LinearDiffRow, SideBySideCell, inline_diff_rows, side_by_side_diff_rows,
};
use crate::app::{AppState, CommentAnchorCapture, VisualSelection, VisualSelectionMode};
use crate::core::text::char_to_byte_index;
use crate::core::{TextAnchor, VisualSelectionEffect};
use crate::review_types::{
    AnchorMatchMethod, AnchorPlacementStatus, CommentAnchorSegment, CommentAnchorSide, ContentMode,
    LineKind, PaneFocus, RenderVariant,
};

const CONTEXT_LINES: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceLine {
    entries: Vec<SourceLineEntry>,
    content: String,
    is_change: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceLineEntry {
    side: CommentAnchorSide,
    line_number: i64,
    content: String,
}

pub fn apply_visual_selection_effect(
    state: &mut AppState,
    view: &impl AppViewport,
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
            match capture_visual_selection(state, view) {
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
    state.visual_selection = Some(VisualSelection {
        mode,
        start: anchor,
        end: anchor,
    });
    state.pending_comment_anchor = None;
    state.mark_model_changed();
}

fn extend_selection(state: &mut AppState, anchor: TextAnchor) {
    state.pane_focus = PaneFocus::Diff;
    state.diff_line_cursor = anchor.line;
    state.diff_col_cursor = anchor.column;
    if let Some(selection) = state.visual_selection.as_mut() {
        selection.end = anchor;
    } else {
        state.visual_selection = Some(VisualSelection {
            mode: VisualSelectionMode::Text,
            start: anchor,
            end: anchor,
        });
    }
    state.mark_model_changed();
}

fn capture_visual_selection(
    state: &AppState,
    view: &impl AppViewport,
) -> Option<CommentAnchorCapture> {
    let selection = state.visual_selection.as_ref()?;
    let file_path = state.selected_file_entry()?.change.path.clone();
    let source_lines = source_lines_for_state(state, view);
    if source_lines.is_empty() {
        return None;
    }

    let (start_anchor, end_anchor) = ordered_anchors(selection.start, selection.end);
    let selection_lines = &source_lines;
    let start_row = start_anchor
        .line
        .min(selection_lines.len().saturating_sub(1));
    let end_row = end_anchor.line.min(selection_lines.len().saturating_sub(1));
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
    selected_lines: &[SourceLine],
    side: CommentAnchorSide,
) -> Vec<CommentAnchorSegment> {
    let side_rows: Vec<(&SourceLineEntry, i64)> = selected_lines
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
    rows: &[(&SourceLineEntry, i64)],
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

fn ordered_anchors(start: TextAnchor, end: TextAnchor) -> (TextAnchor, TextAnchor) {
    if (end.line, end.column) < (start.line, start.column) {
        (end, start)
    } else {
        (start, end)
    }
}

fn normalized_selected_lines(
    state: &AppState,
    selection_lines: &[SourceLine],
    start_row: usize,
    end_row: usize,
) -> Vec<SourceLine> {
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

fn change_group_count(lines: &[SourceLine]) -> usize {
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
    selection_lines: &[SourceLine],
    idx: usize,
    rows: &mut Vec<SourceLine>,
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

fn collect_change_row(selection_lines: &[SourceLine], idx: usize, rows: &mut Vec<SourceLine>) {
    let Some(line) = selection_lines.get(idx) else {
        return;
    };
    if line.is_change {
        rows.push(line.clone());
        return;
    }
    collect_paired_change_row(selection_lines, idx, rows);
}

fn source_lines_for_state(state: &AppState, view: &impl AppViewport) -> Vec<SourceLine> {
    match (state.content_mode, state.render_variant) {
        (ContentMode::FullFile, RenderVariant::BaseVersion) => state
            .base_content
            .as_deref()
            .map(|content| source_lines_from_file_content(content, CommentAnchorSide::Base))
            .unwrap_or_else(|| fallback_source_lines(view)),
        (ContentMode::FullFile, _) => state
            .head_content
            .as_deref()
            .map(|content| source_lines_from_file_content(content, CommentAnchorSide::Head))
            .unwrap_or_else(|| fallback_source_lines(view)),
        (ContentMode::Diff, _) => {
            diff_source_lines(state).unwrap_or_else(|| fallback_source_lines(view))
        }
    }
}

fn source_lines_from_file_content(content: &str, side: CommentAnchorSide) -> Vec<SourceLine> {
    content
        .lines()
        .enumerate()
        .map(|(idx, content)| SourceLine {
            entries: vec![SourceLineEntry {
                side,
                line_number: (idx + 1) as i64,
                content: content.to_string(),
            }],
            content: content.to_string(),
            is_change: false,
        })
        .collect()
}

fn diff_source_lines(state: &AppState) -> Option<Vec<SourceLine>> {
    if state.render_variant == RenderVariant::SideBySide {
        return side_by_side_diff_source_lines(state);
    }

    let entry = state.selected_file_entry()?;
    let rows = inline_diff_rows(&entry.diff.hunks, state.head_content.as_deref());
    let lines = rows
        .rows
        .iter()
        .map(source_line_from_linear_row)
        .collect::<Vec<_>>();
    Some(lines)
}

fn source_line_from_linear_row(row: &LinearDiffRow) -> SourceLine {
    let mut entries = Vec::new();
    if let Some(line_number) = row.old_lineno {
        entries.push(SourceLineEntry {
            side: CommentAnchorSide::Base,
            line_number: line_number as i64,
            content: row.content.clone(),
        });
    }
    if let Some(line_number) = row.new_lineno {
        entries.push(SourceLineEntry {
            side: CommentAnchorSide::Head,
            line_number: line_number as i64,
            content: row.content.clone(),
        });
    }
    SourceLine {
        entries,
        content: row.content.clone(),
        is_change: row.kind != LineKind::Context,
    }
}

fn side_by_side_diff_source_lines(state: &AppState) -> Option<Vec<SourceLine>> {
    let entry = state.selected_file_entry()?;
    let layout = side_by_side_diff_rows(
        &entry.diff.hunks,
        state.base_content.as_deref(),
        state.head_content.as_deref(),
    );
    let lines = layout
        .rows
        .iter()
        .map(source_line_from_side_by_side_row)
        .collect::<Vec<_>>();
    Some(lines).filter(|lines| !lines.is_empty())
}

fn source_line_from_side_by_side_row(row: &crate::app::diff_rows::SideBySideRow) -> SourceLine {
    let entries = [row.base.as_ref(), row.head.as_ref()]
        .into_iter()
        .flatten()
        .map(source_line_entry_from_side_by_side_cell)
        .collect::<Vec<_>>();
    let content = [row.base.as_ref(), row.head.as_ref()]
        .into_iter()
        .flatten()
        .map(|cell| cell.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let is_change = [row.base.as_ref(), row.head.as_ref()]
        .into_iter()
        .flatten()
        .any(|cell| cell.kind != LineKind::Context);
    SourceLine {
        entries,
        content,
        is_change,
    }
}

fn source_line_entry_from_side_by_side_cell(cell: &SideBySideCell) -> SourceLineEntry {
    SourceLineEntry {
        side: cell.side,
        line_number: cell.line_number as i64,
        content: cell.content.clone(),
    }
}

fn fallback_source_lines(view: &impl AppViewport) -> Vec<SourceLine> {
    let start = view.diff_content_start_col();
    view.diff_rendered_text()
        .iter()
        .enumerate()
        .map(|(idx, line)| {
            let content = char_to_byte_index(line, start)
                .map(|start| line[start..].to_string())
                .unwrap_or_default();
            SourceLine {
                entries: vec![SourceLineEntry {
                    side: CommentAnchorSide::Head,
                    line_number: (idx + 1) as i64,
                    content: content.clone(),
                }],
                content,
                is_change: false,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn entry_for(line: &SourceLine, side: CommentAnchorSide) -> &SourceLineEntry {
        line.entries
            .iter()
            .find(|entry| entry.side == side)
            .unwrap_or_else(|| panic!("missing {side:?} entry in {line:?}"))
    }

    #[test]
    fn side_by_side_source_lines_match_render_rows_for_offset_replacement() {
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

        let layout = side_by_side_diff_rows(&[hunk], Some(&base_content), Some(&head_content));
        let rows = layout
            .rows
            .iter()
            .map(source_line_from_side_by_side_row)
            .collect::<Vec<_>>();

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
            SourceLine {
                entries: vec![SourceLineEntry {
                    side: CommentAnchorSide::Head,
                    line_number: 2,
                    content: "two".to_string(),
                }],
                content: "two".to_string(),
                is_change: true,
            },
            SourceLine {
                entries: vec![SourceLineEntry {
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
