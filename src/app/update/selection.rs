use super::cursor;
use super::output::AppOutput;
use super::viewport::AppViewport;
use crate::app::{AppState, CommentAnchorCapture, VisualSelection, VisualSelectionMode};
use crate::core::{TextAnchor, VisualSelectionEffect};
use crate::review_types::{ContentMode, LineKind, PaneFocus, RenderVariant};

const CONTEXT_LINES: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceLine {
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
    let start_row = start_anchor.line.min(source_lines.len().saturating_sub(1));
    let end_row = end_anchor.line.min(source_lines.len().saturating_sub(1));
    let selected_lines = &source_lines[start_row..=end_row];

    let line_start = selected_lines
        .iter()
        .map(|line| line.line_number)
        .min()
        .unwrap_or((start_row + 1) as i64);
    let line_end = selected_lines
        .iter()
        .map(|line| line.line_number)
        .max()
        .unwrap_or((end_row + 1) as i64);

    let head_lines: Vec<&str> = state
        .head_content
        .as_deref()
        .map(|c| c.lines().collect())
        .unwrap_or_default();
    if head_lines.is_empty() {
        return None;
    }

    let s = (line_start as usize)
        .saturating_sub(1)
        .min(head_lines.len().saturating_sub(1));
    let e = (line_end as usize)
        .saturating_sub(1)
        .min(head_lines.len().saturating_sub(1));
    let anchor_text = if s <= e {
        head_lines[s..=e].join("\n")
    } else {
        String::new()
    };

    let context_before = {
        let idx = (line_start as usize).saturating_sub(1);
        let from = idx.saturating_sub(CONTEXT_LINES);
        head_lines[from..idx.min(head_lines.len())].join("\n")
    };
    let context_after = {
        let idx = (line_end as usize).saturating_sub(1);
        let from = idx + 1;
        if from >= head_lines.len() {
            String::new()
        } else {
            let to = (from + CONTEXT_LINES).min(head_lines.len());
            head_lines[from..to].join("\n")
        }
    };

    Some(CommentAnchorCapture {
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

fn ordered_anchors(start: TextAnchor, end: TextAnchor) -> (TextAnchor, TextAnchor) {
    if (end.line, end.column) < (start.line, start.column) {
        (end, start)
    } else {
        (start, end)
    }
}

fn source_lines_for_state(state: &AppState, view: &impl AppViewport) -> Vec<SourceLine> {
    match (state.content_mode, state.render_variant) {
        (ContentMode::FullFile, RenderVariant::BaseVersion) => state
            .base_content
            .as_deref()
            .map(source_lines_from_file_content)
            .unwrap_or_else(|| fallback_source_lines(view)),
        (ContentMode::FullFile, _) => state
            .head_content
            .as_deref()
            .map(source_lines_from_file_content)
            .unwrap_or_else(|| fallback_source_lines(view)),
        (ContentMode::Diff, _) => {
            diff_source_lines(state).unwrap_or_else(|| fallback_source_lines(view))
        }
    }
}

fn source_lines_from_file_content(content: &str) -> Vec<SourceLine> {
    content
        .lines()
        .enumerate()
        .map(|(idx, content)| SourceLine {
            line_number: (idx + 1) as i64,
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
                        line_number: (idx + 1) as i64,
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
                line_number: new_cursor as i64,
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
            let line_number = line.new_lineno.unwrap_or(new_cursor) as i64;
            lines.push(SourceLine {
                line_number,
                content: line.content.trim_end_matches('\n').to_string(),
            });
            match line.kind {
                LineKind::Context => {
                    old_cursor = old_cursor.saturating_add(1);
                    new_cursor = new_cursor.saturating_add(1);
                }
                LineKind::Addition => {
                    new_cursor = new_cursor.saturating_add(1);
                }
                LineKind::Deletion => {
                    old_cursor = old_cursor.saturating_add(1);
                }
            }
        }
    }

    while (new_cursor as usize) <= head_lines.len() {
        lines.push(SourceLine {
            line_number: new_cursor as i64,
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

fn fallback_source_lines(view: &impl AppViewport) -> Vec<SourceLine> {
    let start = view.diff_content_start_col();
    view.diff_rendered_text()
        .iter()
        .enumerate()
        .map(|(idx, line)| {
            let content = if start < line.len() {
                line[start..].to_string()
            } else {
                String::new()
            };
            SourceLine {
                line_number: (idx + 1) as i64,
                content,
            }
        })
        .collect()
}
