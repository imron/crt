use ratatui::style::{Color, Style};
use ratatui::text::Line;

use super::comment_markers::{
    CommentMarker, CommentMarkerSet, marker_column_width, marker_for_side_line,
};
use super::content::BuiltContent;
use super::line::{digit_width, make_line, make_line_with_emphasis};
use crate::app::diff_rows::{LinearDiffRow, LinearDiffRows};
use crate::app::model::BlameLine;
use crate::config::DiffStyle;
use crate::review_types::{CommentAnchorSide, LineKind};

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
    let marker = marker_for_inline_row(comment_markers, row);
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

fn marker_for_inline_row(comment_markers: &CommentMarkerSet, row: &LinearDiffRow) -> CommentMarker {
    match row.kind {
        LineKind::Deletion => {
            marker_for_side_line(comment_markers, CommentAnchorSide::Base, row.old_lineno)
        }
        LineKind::Addition => {
            marker_for_side_line(comment_markers, CommentAnchorSide::Head, row.new_lineno)
        }
        LineKind::Context => {
            let head =
                marker_for_side_line(comment_markers, CommentAnchorSide::Head, row.new_lineno);
            if head.text().is_empty() {
                marker_for_side_line(comment_markers, CommentAnchorSide::Base, row.old_lineno)
            } else {
                head
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::diff_rows::inline_diff_rows;
    use crate::app::model::{CommentAttachment, CommentAttachmentRange};
    use crate::config::DiffStyle;
    use crate::review_types::{AnchorStatus, DiffHunk, DiffLine};

    #[test]
    fn inline_replacement_uses_side_specific_comment_marker_ranges() {
        let comments = [CommentAttachment {
            id: 1,
            line_start: 29,
            line_end: 31,
            resolved: false,
            anchor_status: AnchorStatus::Anchored,
            side_ranges: vec![
                CommentAttachmentRange {
                    side: CommentAnchorSide::Base,
                    line_start: 29,
                    line_end: 29,
                },
                CommentAttachmentRange {
                    side: CommentAnchorSide::Head,
                    line_start: 31,
                    line_end: 31,
                },
            ],
        }];
        let markers = CommentMarkerSet::new(&comments, None);
        let hunk = DiffHunk {
            header: "@@ -29 +31 @@".to_string(),
            old_start: 29,
            old_lines: 1,
            new_start: 31,
            new_lines: 1,
            lines: vec![
                DiffLine {
                    kind: LineKind::Deletion,
                    content: "a.context_before, a.context_after, a.status".to_string(),
                    old_lineno: Some(29),
                    new_lineno: None,
                },
                DiffLine {
                    kind: LineKind::Addition,
                    content: "a.context_before, a.context_after, a.status, ''".to_string(),
                    old_lineno: None,
                    new_lineno: Some(31),
                },
            ],
        };
        let head_content = (1..=34)
            .map(|n| format!("head {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let rows = inline_diff_rows(&[hunk], Some(&head_content));

        let built = build_inline_diff(
            &DiffStyle::default(),
            Color::Black,
            &rows,
            &[],
            &[],
            &markers,
            Color::Blue,
            120,
        );
        let deletion: String = built.lines[30]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        let addition: String = built.lines[31]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();

        assert!(deletion.contains("● -"));
        assert!(addition.contains("● +"));
    }
}
