use std::collections::HashSet;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use super::content::BuiltContent;
use super::line::{digit_width, make_line};
use crate::app::model::{BlameLine, DiffHunk};
use crate::config::DiffStyle;
use crate::review_types::LineKind;

// ---------------------------------------------------------------------------
// Full-file HEAD: HEAD version with additions highlighted
// ---------------------------------------------------------------------------

pub(super) fn build_full_file_head(
    ds: &DiffStyle,
    default_bg: Color,
    hunks: &[DiffHunk],
    head_content: Option<&str>,
    blame: &[BlameLine],
    inner_w: usize,
) -> BuiltContent {
    let content = match head_content {
        Some(c) => c,
        None => {
            return BuiltContent {
                lines: vec![Line::from(Span::styled(
                    "  File not available at HEAD.",
                    Style::default().fg(*ds.placeholder_fg),
                ))],
                hunk_starts: vec![],
                hunk_ends: vec![],
                hunk_first_changes: vec![],
                gutter_w: 0,
            };
        }
    };

    let file_lines: Vec<&str> = content.lines().collect();
    if file_lines.is_empty() {
        return BuiltContent {
            lines: vec![Line::from(Span::styled(
                "  (empty file)",
                Style::default().fg(*ds.placeholder_fg),
            ))],
            hunk_starts: vec![],
            hunk_ends: vec![],
            hunk_first_changes: vec![],
            gutter_w: 0,
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

    let context_style = Style::default().fg(*ds.context_fg);
    let addition_style = Style::default().fg(*ds.addition_fg).bg(*ds.addition_bg);
    let gutter_fg = *ds.gutter_fg;
    let blame_fg = *ds.blame_fg;
    let gutter_w = digit_width(file_lines.len() as u32);
    let mut result = Vec::new();
    let mut hunk_starts = Vec::new();
    let mut hunk_ends = Vec::new();
    let mut hunk_first_changes = Vec::new();
    let mut current_hunk_first_change_recorded = false;

    for (i, text) in file_lines.iter().enumerate() {
        let lineno = (i + 1) as u32;

        // Track hunk boundaries.
        for &(start, end) in &hunk_new_ranges {
            if lineno == start {
                hunk_starts.push(result.len());
                current_hunk_first_change_recorded = false;
            }
            if lineno == end {
                hunk_ends.push(result.len());
            }
        }

        let is_addition = addition_lines.contains(&lineno);

        // Record first change row for the current hunk.
        if is_addition && !current_hunk_first_change_recorded {
            hunk_first_changes.push(result.len());
            current_hunk_first_change_recorded = true;
        }

        let style = if is_addition {
            addition_style
        } else {
            context_style
        };
        let prefix = if is_addition { "+" } else { " " };

        let bl = blame.get((lineno as usize).wrapping_sub(1));
        result.push(make_line(
            None,
            Some(lineno),
            prefix,
            text,
            style,
            gutter_fg,
            blame_fg,
            default_bg,
            gutter_w,
            inner_w,
            bl,
        ));
    }

    // Close any unclosed hunks (hunk extends to end of file).
    while hunk_ends.len() < hunk_starts.len() {
        hunk_ends.push(result.len());
    }
    // Ensure hunk_first_changes has an entry for every hunk.
    while hunk_first_changes.len() < hunk_starts.len() {
        hunk_first_changes.push(*hunk_starts.last().unwrap_or(&0));
    }

    BuiltContent {
        lines: result,
        hunk_starts,
        hunk_ends,
        hunk_first_changes,
        gutter_w,
    }
}

// ---------------------------------------------------------------------------
// Full-file base: base version with deletions highlighted
// ---------------------------------------------------------------------------

pub(super) fn build_full_file_base(
    ds: &DiffStyle,
    default_bg: Color,
    hunks: &[DiffHunk],
    base_content: Option<&str>,
    blame: &[BlameLine],
    inner_w: usize,
) -> BuiltContent {
    let content = match base_content {
        Some(c) => c,
        None => {
            return BuiltContent {
                lines: vec![Line::from(Span::styled(
                    "  File not available at base.",
                    Style::default().fg(*ds.placeholder_fg),
                ))],
                hunk_starts: vec![],
                hunk_ends: vec![],
                hunk_first_changes: vec![],
                gutter_w: 0,
            };
        }
    };

    let file_lines: Vec<&str> = content.lines().collect();
    if file_lines.is_empty() {
        return BuiltContent {
            lines: vec![Line::from(Span::styled(
                "  (empty file)",
                Style::default().fg(*ds.placeholder_fg),
            ))],
            hunk_starts: vec![],
            hunk_ends: vec![],
            hunk_first_changes: vec![],
            gutter_w: 0,
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

    let context_style = Style::default().fg(*ds.context_fg);
    let deletion_style = Style::default().fg(*ds.deletion_fg).bg(*ds.deletion_bg);
    let gutter_fg = *ds.gutter_fg;
    let blame_fg = *ds.blame_fg;
    let gutter_w = digit_width(file_lines.len() as u32);
    let mut result = Vec::new();
    let mut hunk_starts = Vec::new();
    let mut hunk_ends = Vec::new();
    let mut hunk_first_changes = Vec::new();
    let mut current_hunk_first_change_recorded = false;

    for (i, text) in file_lines.iter().enumerate() {
        let lineno = (i + 1) as u32;

        for &(start, end) in &hunk_old_ranges {
            if lineno == start {
                hunk_starts.push(result.len());
                current_hunk_first_change_recorded = false;
            }
            if lineno == end {
                hunk_ends.push(result.len());
            }
        }

        let is_deletion = deletion_lines.contains(&lineno);

        if is_deletion && !current_hunk_first_change_recorded {
            hunk_first_changes.push(result.len());
            current_hunk_first_change_recorded = true;
        }

        let style = if is_deletion {
            deletion_style
        } else {
            context_style
        };
        let prefix = if is_deletion { "-" } else { " " };
        let bl = blame.get((lineno as usize).wrapping_sub(1));

        result.push(make_line(
            Some(lineno),
            None,
            prefix,
            text,
            style,
            gutter_fg,
            blame_fg,
            default_bg,
            gutter_w,
            inner_w,
            bl,
        ));
    }

    while hunk_ends.len() < hunk_starts.len() {
        hunk_ends.push(result.len());
    }
    while hunk_first_changes.len() < hunk_starts.len() {
        hunk_first_changes.push(*hunk_starts.last().unwrap_or(&0));
    }

    BuiltContent {
        lines: result,
        hunk_starts,
        hunk_ends,
        hunk_first_changes,
        gutter_w,
    }
}
