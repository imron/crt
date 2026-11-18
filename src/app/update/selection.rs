use super::cursor;
use super::output::AppOutput;
use super::viewport::AppViewport;
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceLineEntry {
    side: CommentAnchorSide,
    line_number: i64,
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
    let hunk_lines = hunk_source_lines(state);
    let selection_lines = if state.content_mode == ContentMode::Diff
        && state.render_variant == RenderVariant::SideBySide
        && !hunk_lines.is_empty()
    {
        &hunk_lines
    } else if state.content_mode == ContentMode::Diff
        && !hunk_lines.is_empty()
        && end_anchor.line < hunk_lines.len()
    {
        &hunk_lines
    } else {
        &source_lines
    };
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
    let side_rows: Vec<(&SourceLine, i64)> = selected_lines
        .iter()
        .flat_map(|line| {
            line.entries
                .iter()
                .filter(move |entry| entry.side == side)
                .map(move |entry| (line, entry.line_number))
        })
        .collect();
    let Some(content) = content_for_side(state, side) else {
        return Vec::new();
    };
    let content_lines: Vec<&str> = content.lines().collect();
    let mut segments = Vec::new();
    let mut current: Vec<(&SourceLine, i64)> = Vec::new();

    for row in side_rows {
        let starts_new_segment = current
            .last()
            .is_some_and(|(_, previous_line)| row.1 != previous_line.saturating_add(1));
        if starts_new_segment {
            if let Some(segment) = segment_from_side_rows(file_path, side, &content_lines, &current)
            {
                segments.push(segment);
            }
            current.clear();
        }
        current.push(row);
    }

    if let Some(segment) = segment_from_side_rows(file_path, side, &content_lines, &current) {
        segments.push(segment);
    }

    segments
}

fn segment_from_side_rows(
    file_path: &str,
    side: CommentAnchorSide,
    content_lines: &[&str],
    rows: &[(&SourceLine, i64)],
) -> Option<CommentAnchorSegment> {
    let line_start = rows.iter().map(|(_, line)| *line).min()?;
    let line_end = rows.iter().map(|(_, line)| *line).max()?;
    let anchor_text = rows
        .iter()
        .map(|(line, _)| line.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");

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
    let from = idx.saturating_sub(CONTEXT_LINES);
    lines[from..idx.min(lines.len())].join("\n")
}

fn context_after(lines: &[&str], line_end: i64) -> String {
    let idx = line_end
        .checked_sub(1)
        .and_then(|line| usize::try_from(line).ok())
        .unwrap_or(0);
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
    let has_change = selected.iter().any(|line| line.entries.len() == 1);
    if !has_change {
        if state.render_variant == RenderVariant::SideBySide {
            let mut rows = Vec::new();
            for idx in end_row.saturating_add(1)..selection_lines.len() {
                collect_paired_change_row(selection_lines, idx, &mut rows);
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

    let mut rows = Vec::new();
    for idx in start_row..=end_row {
        collect_paired_change_row(selection_lines, idx, &mut rows);
    }
    if rows.is_empty() {
        for idx in start_row..=end_row {
            if let Some(line) = selection_lines.get(idx) {
                if line.entries.len() == 1 {
                    rows.push(line.clone());
                }
            }
        }
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
            }],
            content: content.to_string(),
        })
        .collect()
}

fn diff_source_lines(state: &AppState) -> Option<Vec<SourceLine>> {
    let entry = state.selected_file_entry()?;
    let head_lines: Vec<&str> = state
        .head_content
        .as_deref()
        .map(|content| content.lines().collect())
        .unwrap_or_default();

    if entry.diff.hunks.is_empty() {
        return if head_lines.is_empty() {
            None
        } else {
            Some(
                head_lines
                    .iter()
                    .enumerate()
                    .map(|(idx, content)| SourceLine {
                        entries: vec![SourceLineEntry {
                            side: CommentAnchorSide::Head,
                            line_number: (idx + 1) as i64,
                        }],
                        content: (*content).to_string(),
                    })
                    .collect(),
            )
        };
    }

    let mut lines = Vec::new();
    let mut old_cursor: u32 = 1;
    let mut new_cursor: u32 = 1;

    for hunk in &entry.diff.hunks {
        while new_cursor < hunk.new_start && (new_cursor as usize) <= head_lines.len() {
            lines.push(SourceLine {
                entries: vec![SourceLineEntry {
                    side: CommentAnchorSide::Head,
                    line_number: new_cursor as i64,
                }],
                content: head_lines
                    .get((new_cursor - 1) as usize)
                    .copied()
                    .unwrap_or("")
                    .to_string(),
            });
            old_cursor = old_cursor.saturating_add(1);
            new_cursor = new_cursor.saturating_add(1);
        }

        for line in &hunk.lines {
            match line.kind {
                LineKind::Context => {
                    lines.push(SourceLine {
                        entries: vec![
                            SourceLineEntry {
                                side: CommentAnchorSide::Base,
                                line_number: line.old_lineno.unwrap_or(old_cursor) as i64,
                            },
                            SourceLineEntry {
                                side: CommentAnchorSide::Head,
                                line_number: line.new_lineno.unwrap_or(new_cursor) as i64,
                            },
                        ],
                        content: line.content.trim_end_matches('\n').to_string(),
                    });
                    old_cursor = old_cursor.saturating_add(1);
                    new_cursor = new_cursor.saturating_add(1);
                }
                LineKind::Addition => {
                    lines.push(SourceLine {
                        entries: vec![SourceLineEntry {
                            side: CommentAnchorSide::Head,
                            line_number: line.new_lineno.unwrap_or(new_cursor) as i64,
                        }],
                        content: line.content.trim_end_matches('\n').to_string(),
                    });
                    new_cursor = new_cursor.saturating_add(1);
                }
                LineKind::Deletion => {
                    lines.push(SourceLine {
                        entries: vec![SourceLineEntry {
                            side: CommentAnchorSide::Base,
                            line_number: line.old_lineno.unwrap_or(old_cursor) as i64,
                        }],
                        content: line.content.trim_end_matches('\n').to_string(),
                    });
                    old_cursor = old_cursor.saturating_add(1);
                }
            }
        }
    }

    while (new_cursor as usize) <= head_lines.len() {
        lines.push(SourceLine {
            entries: vec![SourceLineEntry {
                side: CommentAnchorSide::Head,
                line_number: new_cursor as i64,
            }],
            content: head_lines
                .get((new_cursor - 1) as usize)
                .copied()
                .unwrap_or("")
                .to_string(),
        });
        new_cursor = new_cursor.saturating_add(1);
    }

    Some(lines)
}

fn hunk_source_lines(state: &AppState) -> Vec<SourceLine> {
    let Some(entry) = state.selected_file_entry() else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    for hunk in &entry.diff.hunks {
        for line in &hunk.lines {
            lines.push(source_line_from_diff_line(line));
        }
    }
    lines
}

fn source_line_from_diff_line(line: &crate::review_types::DiffLine) -> SourceLine {
    let mut entries = Vec::new();
    if let Some(line_number) = line.old_lineno {
        entries.push(SourceLineEntry {
            side: CommentAnchorSide::Base,
            line_number: line_number as i64,
        });
    }
    if let Some(line_number) = line.new_lineno {
        entries.push(SourceLineEntry {
            side: CommentAnchorSide::Head,
            line_number: line_number as i64,
        });
    }
    SourceLine {
        entries,
        content: line.content.trim_end_matches('\n').to_string(),
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
                }],
                content,
            }
        })
        .collect()
}
