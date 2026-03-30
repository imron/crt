//! Diff/file rendering: inline diff within full file context, full-file
//! mode with change highlighting, and reviewed-file summary.

use std::collections::HashSet;

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::AppState;
use crate::model::{ContentMode, DiffHunk, LineKind, PaneFocus, RenderVariant, ReviewStatus};

/// Draw the diff/file view pane.
pub fn draw(frame: &mut Frame, state: &mut AppState, area: Rect) {
    let focused = state.pane_focus == PaneFocus::Diff;
    let border_style = super::pane_border_style(focused);

    let (title, content, hunk_starts, hunk_ends) = build_content(state);

    state.hunk_start_rows = hunk_starts;
    state.hunk_end_rows = hunk_ends;

    // Store plain text for clipboard extraction.
    state.diff_rendered_text = content
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect();

    // Update content/viewport dimensions for scroll clamping.
    state.diff_content_height = content.len();
    state.diff_view_height = area.height.saturating_sub(2) as usize;
    state.clamp_diff_scroll();

    // Apply scroll offset.
    let visible: Vec<Line> = content.into_iter().skip(state.diff_scroll).collect();

    let paragraph = Paragraph::new(visible).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title),
    );

    frame.render_widget(paragraph, area);
}

// ---------------------------------------------------------------------------
// Content dispatch
// ---------------------------------------------------------------------------

/// Return type for content builders: lines, hunk start rows, hunk end rows.
struct BuiltContent {
    lines: Vec<Line<'static>>,
    hunk_starts: Vec<usize>,
    hunk_ends: Vec<usize>,
}

fn build_content(state: &AppState) -> (String, Vec<Line<'static>>, Vec<usize>, Vec<usize>) {
    let entry = match state.selected_file_entry() {
        None => {
            return (
                " Diff ".to_string(),
                vec![Line::from("No files changed.")],
                vec![],
                vec![],
            )
        }
        Some(e) => e,
    };

    // Reviewed file summary mode.
    if matches!(entry.status, ReviewStatus::Reviewed { .. }) && !state.reviewed_diff_expanded {
        let at = match &entry.status {
            ReviewStatus::Reviewed { at } => at.as_str(),
            _ => "",
        };
        let title = format!(" {} ", entry.change.path);
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  {} \u{2014} reviewed at {at}", entry.change.path),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Press Enter to view diff.",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        return (title, lines, vec![], vec![]);
    }

    // Binary file.
    if entry.diff.is_binary {
        let title = format!(" {} ", entry.change.path);
        return (
            title,
            vec![Line::from(Span::styled(
                "  Binary file",
                Style::default().fg(Color::DarkGray),
            ))],
            vec![],
            vec![],
        );
    }

    let built = match state.content_mode {
        ContentMode::Diff => build_inline_diff(&entry.diff.hunks, state.head_content.as_deref()),
        ContentMode::FullFile => match state.render_variant {
            RenderVariant::HeadVersion => {
                build_full_file_head(&entry.diff.hunks, state.head_content.as_deref())
            }
            RenderVariant::BaseVersion => {
                build_full_file_base(&entry.diff.hunks, state.base_content.as_deref())
            }
            _ => BuiltContent {
                lines: vec![Line::from("  (unknown variant)")],
                hunk_starts: vec![],
                hunk_ends: vec![],
            },
        },
    };

    // Build title with hunk navigation info.
    let total_hunks = built.hunk_starts.len();
    let title = build_title(state, entry, total_hunks);

    (title, built.lines, built.hunk_starts, built.hunk_ends)
}

/// Build the border title with hunk navigation context.
fn build_title(state: &AppState, entry: &crate::model::FileEntry, total_hunks: usize) -> String {
    let path = &entry.change.path;

    let mode_label = match state.content_mode {
        ContentMode::FullFile => match state.render_variant {
            RenderVariant::HeadVersion => " (HEAD)",
            RenderVariant::BaseVersion => " (base)",
            _ => "",
        },
        ContentMode::Diff => "",
    };

    let hunk_info = if total_hunks == 0 {
        String::new()
    } else if let Some(idx) = state.current_hunk_index() {
        format!(" \u{2014} diff {}/{total_hunks}", idx + 1)
    } else {
        format!(" \u{2014} {total_hunks} diffs")
    };

    format!(" {path}{mode_label}{hunk_info} ")
}

// ---------------------------------------------------------------------------
// Inline diff: full file with additions + deletions interleaved
// ---------------------------------------------------------------------------

fn build_inline_diff(hunks: &[DiffHunk], head_content: Option<&str>) -> BuiltContent {
    if hunks.is_empty() {
        return BuiltContent {
            lines: vec![Line::from("  No changes.")],
            hunk_starts: vec![],
            hunk_ends: vec![],
        };
    }

    let head_lines: Vec<&str> = head_content
        .map(|c| c.lines().collect())
        .unwrap_or_default();

    // Compute gutter width.
    let max_old = hunks
        .iter()
        .map(|h| h.old_start + h.old_lines)
        .max()
        .unwrap_or(0);
    let max_new = head_lines.len().max(
        hunks
            .iter()
            .map(|h| (h.new_start + h.new_lines) as usize)
            .max()
            .unwrap_or(0),
    ) as u32;
    let gutter_w = digit_width(max_old.max(max_new));

    let mut result = Vec::new();
    let mut hunk_starts = Vec::new();
    let mut hunk_ends = Vec::new();
    let mut old_cursor: u32 = 1;
    let mut new_cursor: u32 = 1;

    for hunk in hunks {
        // Gap before this hunk: unchanged lines from HEAD.
        while new_cursor < hunk.new_start && (new_cursor as usize) <= head_lines.len() {
            let content = head_lines.get((new_cursor - 1) as usize).unwrap_or(&"");
            result.push(make_line(
                Some(old_cursor),
                Some(new_cursor),
                " ",
                content,
                Style::default().fg(Color::Gray),
                gutter_w,
            ));
            old_cursor += 1;
            new_cursor += 1;
        }

        // Record hunk start.
        hunk_starts.push(result.len());

        // Hunk lines.
        for diff_line in &hunk.lines {
            let (prefix, style) = match diff_line.kind {
                LineKind::Context => (" ", Style::default().fg(Color::Gray)),
                LineKind::Addition => ("+", Style::default().fg(Color::Green)),
                LineKind::Deletion => ("-", Style::default().fg(Color::Red)),
            };
            let content = diff_line.content.trim_end_matches('\n');
            result.push(make_line(
                diff_line.old_lineno,
                diff_line.new_lineno,
                prefix,
                content,
                style,
                gutter_w,
            ));
            match diff_line.kind {
                LineKind::Context => {
                    old_cursor += 1;
                    new_cursor += 1;
                }
                LineKind::Addition => new_cursor += 1,
                LineKind::Deletion => old_cursor += 1,
            }
        }

        // Record hunk end.
        hunk_ends.push(result.len());
    }

    // Gap after last hunk.
    while (new_cursor as usize) <= head_lines.len() {
        let content = head_lines.get((new_cursor - 1) as usize).unwrap_or(&"");
        result.push(make_line(
            Some(old_cursor),
            Some(new_cursor),
            " ",
            content,
            Style::default().fg(Color::Gray),
            gutter_w,
        ));
        old_cursor += 1;
        new_cursor += 1;
    }

    BuiltContent {
        lines: result,
        hunk_starts,
        hunk_ends,
    }
}

// ---------------------------------------------------------------------------
// Full-file HEAD: HEAD version with additions highlighted
// ---------------------------------------------------------------------------

fn build_full_file_head(hunks: &[DiffHunk], head_content: Option<&str>) -> BuiltContent {
    let content = match head_content {
        Some(c) => c,
        None => {
            return BuiltContent {
                lines: vec![Line::from(Span::styled(
                    "  File not available at HEAD.",
                    Style::default().fg(Color::DarkGray),
                ))],
                hunk_starts: vec![],
                hunk_ends: vec![],
            };
        }
    };

    let file_lines: Vec<&str> = content.lines().collect();
    if file_lines.is_empty() {
        return BuiltContent {
            lines: vec![Line::from(Span::styled(
                "  (empty file)",
                Style::default().fg(Color::DarkGray),
            ))],
            hunk_starts: vec![],
            hunk_ends: vec![],
        };
    }

    // Collect all new-file line numbers that are additions.
    let addition_lines: HashSet<u32> = hunks
        .iter()
        .flat_map(|h| h.lines.iter())
        .filter(|l| l.kind == LineKind::Addition)
        .filter_map(|l| l.new_lineno)
        .collect();

    // Hunk boundaries in new-file line numbers.
    let hunk_new_ranges: Vec<(u32, u32)> = hunks
        .iter()
        .map(|h| (h.new_start, h.new_start + h.new_lines))
        .collect();

    let gutter_w = digit_width(file_lines.len() as u32);
    let mut result = Vec::new();
    let mut hunk_starts = Vec::new();
    let mut hunk_ends = Vec::new();

    for (i, text) in file_lines.iter().enumerate() {
        let lineno = (i + 1) as u32;

        // Track hunk boundaries.
        for &(start, end) in &hunk_new_ranges {
            if lineno == start {
                hunk_starts.push(result.len());
            }
            if lineno == end {
                hunk_ends.push(result.len());
            }
        }

        let is_addition = addition_lines.contains(&lineno);
        let style = if is_addition {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Gray)
        };
        let prefix = if is_addition { "+" } else { " " };

        result.push(make_line(None, Some(lineno), prefix, text, style, gutter_w));
    }

    // Close any unclosed hunks (hunk extends to end of file).
    while hunk_ends.len() < hunk_starts.len() {
        hunk_ends.push(result.len());
    }

    BuiltContent {
        lines: result,
        hunk_starts,
        hunk_ends,
    }
}

// ---------------------------------------------------------------------------
// Full-file base: base version with deletions highlighted
// ---------------------------------------------------------------------------

fn build_full_file_base(hunks: &[DiffHunk], base_content: Option<&str>) -> BuiltContent {
    let content = match base_content {
        Some(c) => c,
        None => {
            return BuiltContent {
                lines: vec![Line::from(Span::styled(
                    "  File not available at base.",
                    Style::default().fg(Color::DarkGray),
                ))],
                hunk_starts: vec![],
                hunk_ends: vec![],
            };
        }
    };

    let file_lines: Vec<&str> = content.lines().collect();
    if file_lines.is_empty() {
        return BuiltContent {
            lines: vec![Line::from(Span::styled(
                "  (empty file)",
                Style::default().fg(Color::DarkGray),
            ))],
            hunk_starts: vec![],
            hunk_ends: vec![],
        };
    }

    // Collect all old-file line numbers that are deletions.
    let deletion_lines: HashSet<u32> = hunks
        .iter()
        .flat_map(|h| h.lines.iter())
        .filter(|l| l.kind == LineKind::Deletion)
        .filter_map(|l| l.old_lineno)
        .collect();

    // Hunk boundaries in old-file line numbers.
    let hunk_old_ranges: Vec<(u32, u32)> = hunks
        .iter()
        .map(|h| (h.old_start, h.old_start + h.old_lines))
        .collect();

    let gutter_w = digit_width(file_lines.len() as u32);
    let mut result = Vec::new();
    let mut hunk_starts = Vec::new();
    let mut hunk_ends = Vec::new();

    for (i, text) in file_lines.iter().enumerate() {
        let lineno = (i + 1) as u32;

        for &(start, end) in &hunk_old_ranges {
            if lineno == start {
                hunk_starts.push(result.len());
            }
            if lineno == end {
                hunk_ends.push(result.len());
            }
        }

        let is_deletion = deletion_lines.contains(&lineno);
        let style = if is_deletion {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Gray)
        };
        let prefix = if is_deletion { "-" } else { " " };

        result.push(make_line(Some(lineno), None, prefix, text, style, gutter_w));
    }

    while hunk_ends.len() < hunk_starts.len() {
        hunk_ends.push(result.len());
    }

    BuiltContent {
        lines: result,
        hunk_starts,
        hunk_ends,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a styled line with optional old/new line number gutters.
fn make_line(
    old_lineno: Option<u32>,
    new_lineno: Option<u32>,
    prefix: &str,
    content: &str,
    content_style: Style,
    gutter_w: usize,
) -> Line<'static> {
    let gutter_style = Style::default().fg(Color::DarkGray);

    let old_num = match old_lineno {
        Some(n) => format!("{n:>gutter_w$}"),
        None => " ".repeat(gutter_w),
    };
    let new_num = match new_lineno {
        Some(n) => format!("{n:>gutter_w$}"),
        None => " ".repeat(gutter_w),
    };

    Line::from(vec![
        Span::styled(old_num, gutter_style),
        Span::styled(" ", gutter_style),
        Span::styled(new_num, gutter_style),
        Span::styled(format!(" {prefix} "), content_style),
        Span::styled(content.to_string(), content_style),
    ])
}

/// Number of decimal digits needed to display `n` (minimum 3).
fn digit_width(n: u32) -> usize {
    match n {
        0..=9 => 1,
        10..=99 => 2,
        100..=999 => 3,
        1000..=9999 => 4,
        10000..=99999 => 5,
        _ => 6,
    }
    .max(3)
}
