use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use super::comment_markers::{CommentMarkerSet, marker_column_width, marker_for_line};
use super::content::BuiltContent;
use super::line::{digit_width, make_line};
use crate::app::diff_rows::{LinearDiffRow, LinearDiffRows};
use crate::app::model::BlameLine;
use crate::config::DiffStyle;
use crate::review_types::LineKind;

// ---------------------------------------------------------------------------
// Full-file HEAD: HEAD version with additions highlighted
// ---------------------------------------------------------------------------

pub fn build_full_file_head(
    ds: &DiffStyle,
    default_bg: Color,
    rows: &LinearDiffRows,
    head_content: Option<&str>,
    blame: &[BlameLine],
    comment_markers: &CommentMarkerSet,
    current_comment_fg: Color,
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
                comment_marker_w: 0,
            };
        }
    };

    if content.lines().next().is_none() {
        return BuiltContent {
            lines: vec![Line::from(Span::styled(
                "  (empty file)",
                Style::default().fg(*ds.placeholder_fg),
            ))],
            hunk_starts: vec![],
            hunk_ends: vec![],
            hunk_first_changes: vec![],
            gutter_w: 0,
            comment_marker_w: 0,
        };
    }

    let context_style = Style::default().fg(*ds.context_fg);
    let addition_style = Style::default().fg(*ds.addition_fg).bg(*ds.addition_bg);
    let gutter_fg = *ds.gutter_fg;
    let blame_fg = *ds.blame_fg;
    let gutter_w = digit_width(max_lineno(rows));
    let result = rows
        .rows
        .iter()
        .map(|row| {
            full_file_row(
                row,
                row.new_lineno,
                comment_markers,
                row.new_lineno
                    .and_then(|lineno| blame.get((lineno as usize).wrapping_sub(1))),
                if row.kind == LineKind::Addition {
                    addition_style
                } else {
                    context_style
                },
                if row.kind == LineKind::Addition {
                    "+"
                } else {
                    " "
                },
                gutter_fg,
                current_comment_fg,
                blame_fg,
                default_bg,
                gutter_w,
                inner_w,
            )
        })
        .collect();

    BuiltContent {
        lines: result,
        hunk_starts: rows.hunk_starts.clone(),
        hunk_ends: rows.hunk_ends.clone(),
        hunk_first_changes: rows.hunk_first_changes.clone(),
        gutter_w,
        comment_marker_w: marker_column_width(comment_markers),
    }
}

// ---------------------------------------------------------------------------
// Full-file base: base version with deletions highlighted
// ---------------------------------------------------------------------------

pub fn build_full_file_base(
    ds: &DiffStyle,
    default_bg: Color,
    rows: &LinearDiffRows,
    base_content: Option<&str>,
    blame: &[BlameLine],
    comment_markers: &CommentMarkerSet,
    current_comment_fg: Color,
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
                comment_marker_w: 0,
            };
        }
    };

    if content.lines().next().is_none() {
        return BuiltContent {
            lines: vec![Line::from(Span::styled(
                "  (empty file)",
                Style::default().fg(*ds.placeholder_fg),
            ))],
            hunk_starts: vec![],
            hunk_ends: vec![],
            hunk_first_changes: vec![],
            gutter_w: 0,
            comment_marker_w: 0,
        };
    }

    let context_style = Style::default().fg(*ds.context_fg);
    let deletion_style = Style::default().fg(*ds.deletion_fg).bg(*ds.deletion_bg);
    let gutter_fg = *ds.gutter_fg;
    let blame_fg = *ds.blame_fg;
    let gutter_w = digit_width(max_lineno(rows));
    let result = rows
        .rows
        .iter()
        .map(|row| {
            full_file_row(
                row,
                row.old_lineno,
                comment_markers,
                row.old_lineno
                    .and_then(|lineno| blame.get((lineno as usize).wrapping_sub(1))),
                if row.kind == LineKind::Deletion {
                    deletion_style
                } else {
                    context_style
                },
                if row.kind == LineKind::Deletion {
                    "-"
                } else {
                    " "
                },
                gutter_fg,
                current_comment_fg,
                blame_fg,
                default_bg,
                gutter_w,
                inner_w,
            )
        })
        .collect();

    BuiltContent {
        lines: result,
        hunk_starts: rows.hunk_starts.clone(),
        hunk_ends: rows.hunk_ends.clone(),
        hunk_first_changes: rows.hunk_first_changes.clone(),
        gutter_w,
        comment_marker_w: marker_column_width(comment_markers),
    }
}

fn max_lineno(rows: &LinearDiffRows) -> u32 {
    rows.rows
        .iter()
        .filter_map(|row| row.old_lineno.or(row.new_lineno))
        .max()
        .unwrap_or(0)
}

#[allow(clippy::too_many_arguments)]
fn full_file_row(
    row: &LinearDiffRow,
    marker_line: Option<u32>,
    comment_markers: &CommentMarkerSet,
    blame: Option<&BlameLine>,
    style: Style,
    prefix: &str,
    gutter_fg: Color,
    current_comment_fg: Color,
    blame_fg: Color,
    default_bg: Color,
    gutter_w: usize,
    inner_w: usize,
) -> Line<'static> {
    make_line(
        row.old_lineno,
        row.new_lineno,
        &marker_for_line(comment_markers, marker_line),
        prefix,
        &row.content,
        style,
        gutter_fg,
        current_comment_fg,
        blame_fg,
        default_bg,
        gutter_w,
        inner_w,
        blame,
    )
}
