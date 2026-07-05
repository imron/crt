use ratatui::style::{Color, Style};
use ratatui::text::Line;

use super::comment_markers::{CommentMarkerSet, marker_column_width, marker_for_line};
use super::content::BuiltContent;
use super::line::{digit_width, make_line, make_line_with_emphasis};
use crate::app::diff_rows::{LinearDiffRow, LinearDiffRows};
use crate::app::model::BlameLine;
use crate::config::DiffStyle;
use crate::review_types::LineKind;

// ---------------------------------------------------------------------------
// Inline diff: full file with additions + deletions interleaved
// ---------------------------------------------------------------------------

pub fn build_inline_diff(
    ds: &DiffStyle,
    default_bg: Color,
    rows: &LinearDiffRows,
    head_blame: &[BlameLine],
    base_blame: &[BlameLine],
    comment_markers: &CommentMarkerSet,
    current_comment_fg: Color,
    inner_w: usize,
) -> BuiltContent {
    if rows.rows.is_empty() {
        return BuiltContent {
            lines: vec![Line::from("  No changes.")],
            hunk_starts: vec![],
            hunk_ends: vec![],
            hunk_first_changes: vec![],
            gutter_w: 0,
            comment_marker_w: 0,
        };
    }

    let max_old = rows
        .rows
        .iter()
        .filter_map(|row| row.old_lineno)
        .max()
        .unwrap_or(0);
    let max_new = rows
        .rows
        .iter()
        .filter_map(|row| row.new_lineno)
        .max()
        .unwrap_or(0);
    let gutter_w = digit_width(max_old.max(max_new));

    let context_style = Style::default().fg(*ds.context_fg);
    let gutter_fg = *ds.gutter_fg;
    let blame_fg = *ds.blame_fg;

    // Blame lookups: line numbers are 1-indexed, blame vecs are 0-indexed.
    let hblame = |lineno: Option<u32>| -> Option<&BlameLine> {
        lineno.and_then(|n| head_blame.get((n as usize).wrapping_sub(1)))
    };
    let bblame = |lineno: Option<u32>| -> Option<&BlameLine> {
        lineno.and_then(|n| base_blame.get((n as usize).wrapping_sub(1)))
    };

    let result = rows
        .rows
        .iter()
        .map(|row| {
            inline_row(
                ds,
                default_bg,
                row,
                comment_markers,
                current_comment_fg,
                gutter_w,
                inner_w,
                gutter_fg,
                blame_fg,
                hblame(row.new_lineno).or_else(|| bblame(row.old_lineno)),
                context_style,
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

#[allow(clippy::too_many_arguments)]
fn inline_row(
    ds: &DiffStyle,
    default_bg: Color,
    row: &LinearDiffRow,
    comment_markers: &CommentMarkerSet,
    current_comment_fg: Color,
    gutter_w: usize,
    inner_w: usize,
    gutter_fg: Color,
    blame_fg: Color,
    blame: Option<&BlameLine>,
    context_style: Style,
) -> Line<'static> {
    let marker_line = row.new_lineno;
    let marker = marker_for_line(comment_markers, marker_line);
    match row.kind {
        LineKind::Context => make_line(
            row.old_lineno,
            row.new_lineno,
            &marker,
            " ",
            &row.content,
            context_style,
            gutter_fg,
            current_comment_fg,
            blame_fg,
            default_bg,
            gutter_w,
            inner_w,
            blame,
        ),
        LineKind::Deletion => {
            if let Some((old_spans, _)) = row
                .paired_content
                .as_deref()
                .and_then(|paired| super::super::word_diff::compute(&row.content, paired))
            {
                make_line_with_emphasis(
                    row.old_lineno,
                    row.new_lineno,
                    &marker,
                    "-",
                    &old_spans,
                    Style::default().fg(*ds.deletion_fg).bg(*ds.deletion_bg),
                    Style::default()
                        .fg(*ds.deletion_fg)
                        .bg(*ds.deletion_emphasis_bg),
                    gutter_fg,
                    current_comment_fg,
                    blame_fg,
                    default_bg,
                    gutter_w,
                    inner_w,
                    blame,
                )
            } else {
                make_line(
                    row.old_lineno,
                    row.new_lineno,
                    &marker,
                    "-",
                    &row.content,
                    Style::default().fg(*ds.deletion_fg).bg(*ds.deletion_bg),
                    gutter_fg,
                    current_comment_fg,
                    blame_fg,
                    default_bg,
                    gutter_w,
                    inner_w,
                    blame,
                )
            }
        }
        LineKind::Addition => {
            if let Some((_, new_spans)) = row
                .paired_content
                .as_deref()
                .and_then(|paired| super::super::word_diff::compute(paired, &row.content))
            {
                make_line_with_emphasis(
                    row.old_lineno,
                    row.new_lineno,
                    &marker,
                    "+",
                    &new_spans,
                    Style::default().fg(*ds.addition_fg).bg(*ds.addition_bg),
                    Style::default()
                        .fg(*ds.addition_fg)
                        .bg(*ds.addition_emphasis_bg),
                    gutter_fg,
                    current_comment_fg,
                    blame_fg,
                    default_bg,
                    gutter_w,
                    inner_w,
                    blame,
                )
            } else {
                make_line(
                    row.old_lineno,
                    row.new_lineno,
                    &marker,
                    "+",
                    &row.content,
                    Style::default().fg(*ds.addition_fg).bg(*ds.addition_bg),
                    gutter_fg,
                    current_comment_fg,
                    blame_fg,
                    default_bg,
                    gutter_w,
                    inner_w,
                    blame,
                )
            }
        }
    }
}
