use ratatui::style::{Color, Style};
use ratatui::text::Line;

use super::comment_markers::{CommentMarkerSet, marker_column_width, marker_for_inline_row};
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
        .enumerate()
        .map(|(row_index, row)| {
            inline_row(
                ds,
                default_bg,
                row_index,
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
    row_index: usize,
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
    let marker = marker_for_inline_row(comment_markers, row_index);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::diff_rows::inline_diff_rows;
    use crate::app::model::{CommentAttachment, CommentAttachmentRange};
    use crate::config::DiffStyle;
    use crate::review_types::{AnchorStatus, CommentAnchorSide, DiffHunk, DiffLine};

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
        let markers =
            CommentMarkerSet::new_with_current_inline_rows(&comments, None, None, &rows, None);

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

    #[test]
    fn inline_replacement_draws_one_logical_multiline_comment() {
        let comments = [CommentAttachment {
            id: 1,
            line_start: 130,
            line_end: 142,
            resolved: false,
            anchor_status: AnchorStatus::Anchored,
            side_ranges: vec![
                CommentAttachmentRange {
                    side: CommentAnchorSide::Base,
                    line_start: 130,
                    line_end: 130,
                },
                CommentAttachmentRange {
                    side: CommentAnchorSide::Head,
                    line_start: 139,
                    line_end: 142,
                },
            ],
        }];
        let hunk = DiffHunk {
            header: "@@ -121,2 +130,5 @@".to_string(),
            old_start: 121,
            old_lines: 2,
            new_start: 130,
            new_lines: 5,
            lines: vec![
                DiffLine {
                    kind: LineKind::Context,
                    content: "return;".to_string(),
                    old_lineno: Some(121),
                    new_lineno: Some(130),
                },
                DiffLine {
                    kind: LineKind::Deletion,
                    content: ".position(|comment| comment.line_start > cursor_line)".to_string(),
                    old_lineno: Some(130),
                    new_lineno: None,
                },
                DiffLine {
                    kind: LineKind::Addition,
                    content: ".position(|comment| {".to_string(),
                    old_lineno: None,
                    new_lineno: Some(139),
                },
                DiffLine {
                    kind: LineKind::Addition,
                    content: "    visible_comment_line_range(state, comment)".to_string(),
                    old_lineno: None,
                    new_lineno: Some(140),
                },
                DiffLine {
                    kind: LineKind::Addition,
                    content: "        .is_some_and(|(line_start, _)| line_start > cursor_line)"
                        .to_string(),
                    old_lineno: None,
                    new_lineno: Some(141),
                },
                DiffLine {
                    kind: LineKind::Addition,
                    content: "})".to_string(),
                    old_lineno: None,
                    new_lineno: Some(142),
                },
            ],
        };
        let rows = inline_diff_rows(&[hunk], None);
        let markers = CommentMarkerSet::new_with_current_inline_rows(
            &comments,
            Some(130),
            Some(1),
            &rows,
            None,
        );

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
        let rendered = built
            .lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(!rendered[0].contains("●"));
        assert!(!rendered[0].contains("┃"));
        assert!(rendered[1].contains("● -"));
        assert!(rendered[2].contains("┃ +"));
        assert!(rendered[3].contains("┃ +"));
        assert!(rendered[4].contains("┃ +"));
        assert!(rendered[5].contains("● +"));
    }
}
