use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use super::comment_markers::{
    CommentMarker, CommentMarkerSet, marker_column_width, marker_for_side_line,
};
use super::content::BuiltContent;
use super::line::{BLAME_COL_WIDTH, digit_width, format_blame};
use crate::app::diff_rows::{SideBySideCell, SideBySideDiffRows};
use crate::app::model::{BlameLine, DiffHunk};
use crate::config::DiffStyle;
use crate::review_types::LineKind;

// ---------------------------------------------------------------------------
// Side-by-side diff
// ---------------------------------------------------------------------------

pub fn build_side_by_side_diff(
    ds: &DiffStyle,
    default_bg: Color,
    hunks: &[DiffHunk],
    side_by_side_rows: &SideBySideDiffRows,
    head_content: Option<&str>,
    head_blame: &[BlameLine],
    base_blame: &[BlameLine],
    comment_markers: &CommentMarkerSet,
    current_comment_fg: Color,
    inner_w: usize,
) -> BuiltContent {
    let head_lines: Vec<&str> = head_content
        .map(|c| c.lines().collect())
        .unwrap_or_default();

    if side_by_side_rows.rows.is_empty() && head_lines.is_empty() {
        return BuiltContent {
            lines: vec![Line::from("  No changes.")],
            hunk_starts: vec![],
            hunk_ends: vec![],
            hunk_first_changes: vec![],
            gutter_w: 0,
            comment_marker_w: 0,
        };
    }

    // Compute gutter width from max line numbers.
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
    let comment_marker_w = marker_column_width(comment_markers);

    let has_blame = !head_blame.is_empty() || !base_blame.is_empty();
    let blame_w = if has_blame { BLAME_COL_WIDTH + 1 } else { 0 };

    // Each column: [blame + " "] + gutter + " " + marker + " " + content.
    // Middle divider is " │ ".
    let marker_sep = usize::from(comment_marker_w > 0);
    let col_fixed = blame_w + gutter_w + comment_marker_w + marker_sep + 1;
    let divider_w = 3; // " │ "
    let available = inner_w.saturating_sub(col_fixed * 2 + divider_w);
    let col_w = available / 2;

    let context_style = Style::default().fg(*ds.context_fg);
    let addition_style = Style::default().fg(*ds.addition_fg).bg(*ds.addition_bg);
    let deletion_style = Style::default().fg(*ds.deletion_fg).bg(*ds.deletion_bg);
    let addition_emphasis = Style::default()
        .fg(*ds.addition_fg)
        .bg(*ds.addition_emphasis_bg);
    let deletion_emphasis = Style::default()
        .fg(*ds.deletion_fg)
        .bg(*ds.deletion_emphasis_bg);
    let gutter_fg = *ds.gutter_fg;
    let blame_fg = *ds.blame_fg;
    let divider_style = Style::default().fg(*ds.gutter_fg);

    // Blame lookups.
    let hblame = |lineno: Option<u32>| -> Option<&BlameLine> {
        lineno.and_then(|n| head_blame.get((n as usize).wrapping_sub(1)))
    };
    let bblame = |lineno: Option<u32>| -> Option<&BlameLine> {
        lineno.and_then(|n| base_blame.get((n as usize).wrapping_sub(1)))
    };

    let mut result: Vec<Line<'static>> = Vec::new();
    // Helper: build one side of a row (blame + gutter + content), padded to col_w.
    let make_half = |lineno: Option<u32>,
                     content: &str,
                     prefix: &str,
                     comment_marker: &CommentMarker,
                     style: Style,
                     emphasis: Option<(&[super::super::word_diff::DiffSpan<'_>], Style)>,
                     blame: Option<&BlameLine>|
     -> Vec<Span<'static>> {
        let bg = style.bg.unwrap_or(default_bg);
        let gutter_style = Style::default().fg(gutter_fg).bg(bg);

        let mut spans = Vec::new();

        // Blame column.
        if has_blame {
            let blame_style = Style::default().fg(blame_fg).bg(bg);
            spans.push(Span::styled(format_blame(blame), blame_style));
            spans.push(Span::styled(" ", gutter_style));
        }

        let num = match lineno {
            Some(n) => format!("{n:>gutter_w$} "),
            None => format!("{} ", " ".repeat(gutter_w)),
        };
        spans.push(Span::styled(num, gutter_style));
        if !comment_marker.text().is_empty() {
            let marker_style = if comment_marker.is_current() {
                gutter_style.fg(current_comment_fg)
            } else {
                gutter_style
            };
            spans.push(Span::styled(
                comment_marker.text().to_string(),
                marker_style,
            ));
        }
        if comment_marker.width() > 0 {
            spans.push(Span::styled(" ", gutter_style));
        }

        if let Some((word_spans, em_style)) = emphasis {
            let mut chars = 0usize;
            if !prefix.is_empty() && chars < col_w {
                let visible_prefix: String = prefix.chars().take(col_w - chars).collect();
                chars += visible_prefix.chars().count();
                spans.push(Span::styled(visible_prefix, style));
            }
            for ws in word_spans {
                let (text, s) = match ws {
                    super::super::word_diff::DiffSpan::Common(t) => (*t, style),
                    super::super::word_diff::DiffSpan::Changed(t) => (*t, em_style),
                };
                let t: String = text.chars().take(col_w.saturating_sub(chars)).collect();
                chars += t.chars().count();
                spans.push(Span::styled(t, s));
                if chars >= col_w {
                    break;
                }
            }
            let pad = col_w.saturating_sub(chars);
            if pad > 0 {
                spans.push(Span::styled(" ".repeat(pad), style));
            }
        } else {
            let mut chars = 0usize;
            let mut text = String::new();
            if !prefix.is_empty() && chars < col_w {
                let visible_prefix: String = prefix.chars().take(col_w - chars).collect();
                chars += visible_prefix.chars().count();
                text.push_str(&visible_prefix);
            }
            if chars < col_w {
                let truncated: String = content.chars().take(col_w - chars).collect();
                chars += truncated.chars().count();
                text.push_str(&truncated);
            }
            let pad = col_w.saturating_sub(chars);
            spans.push(Span::styled(text, style));
            if pad > 0 {
                spans.push(Span::styled(" ".repeat(pad), style));
            }
        }
        spans
    };

    // Helper: build an empty half (filler).
    let make_empty_half = || -> Vec<Span<'static>> {
        let gutter_style = Style::default().fg(gutter_fg).bg(default_bg);
        let mut spans = Vec::new();
        if has_blame {
            spans.push(Span::styled(" ".repeat(BLAME_COL_WIDTH), gutter_style));
            spans.push(Span::styled(" ", gutter_style));
        }
        let num = format!("{} ", " ".repeat(gutter_w));
        spans.push(Span::styled(num, gutter_style));
        if comment_marker_w > 0 {
            spans.push(Span::styled(" ".repeat(comment_marker_w), gutter_style));
            spans.push(Span::styled(" ", gutter_style));
        }
        spans.push(Span::styled(
            " ".repeat(col_w),
            Style::default().bg(default_bg),
        ));
        spans
    };

    // Helper: combine left + divider + right into a Line.
    let make_row = |left: Vec<Span<'static>>, right: Vec<Span<'static>>| -> Line<'static> {
        let mut spans = left;
        spans.push(Span::styled(" \u{2502} ", divider_style));
        spans.extend(right);
        Line::from(spans)
    };

    for row in &side_by_side_rows.rows {
        let word_diff = match (&row.base, &row.head) {
            (Some(base), Some(head))
                if base.kind == LineKind::Deletion && head.kind == LineKind::Addition =>
            {
                super::super::word_diff::compute(&base.content, &head.content)
            }
            _ => None,
        };
        let left = match &row.base {
            Some(cell) => {
                let marker =
                    marker_for_side_line(comment_markers, cell.side, Some(cell.line_number));
                let emphasis = word_diff
                    .as_ref()
                    .map(|(old_spans, _)| (old_spans.as_slice(), deletion_emphasis));
                make_half(
                    Some(cell.line_number),
                    &cell.content,
                    prefix_for_cell(cell),
                    &marker,
                    style_for_cell(cell, context_style, addition_style, deletion_style),
                    emphasis,
                    bblame(Some(cell.line_number)),
                )
            }
            None => make_empty_half(),
        };
        let right = match &row.head {
            Some(cell) => {
                let marker =
                    marker_for_side_line(comment_markers, cell.side, Some(cell.line_number));
                let emphasis = word_diff
                    .as_ref()
                    .map(|(_, new_spans)| (new_spans.as_slice(), addition_emphasis));
                make_half(
                    Some(cell.line_number),
                    &cell.content,
                    prefix_for_cell(cell),
                    &marker,
                    style_for_cell(cell, context_style, addition_style, deletion_style),
                    emphasis,
                    hblame(Some(cell.line_number)),
                )
            }
            None => make_empty_half(),
        };
        result.push(make_row(left, right));
    }

    BuiltContent {
        lines: result,
        hunk_starts: side_by_side_rows.hunk_starts.clone(),
        hunk_ends: side_by_side_rows.hunk_ends.clone(),
        hunk_first_changes: side_by_side_rows.hunk_first_changes.clone(),
        gutter_w,
        comment_marker_w,
    }
}

fn prefix_for_cell(cell: &SideBySideCell) -> &'static str {
    match cell.kind {
        LineKind::Addition => "+ ",
        LineKind::Deletion => "- ",
        LineKind::Context => "",
    }
}

fn style_for_cell(
    cell: &SideBySideCell,
    context_style: Style,
    addition_style: Style,
    deletion_style: Style,
) -> Style {
    match cell.kind {
        LineKind::Addition => addition_style,
        LineKind::Deletion => deletion_style,
        LineKind::Context => context_style,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::diff_rows::side_by_side_diff_rows;
    use crate::app::model::{CommentAttachment, CommentAttachmentRange, DiffLine};
    use crate::config::DiffStyle;
    use crate::review_types::{self, AnchorStatus, CommentAnchorSide, LineKind};

    fn model_hunk_from_review(hunk: &review_types::DiffHunk) -> DiffHunk {
        DiffHunk {
            header: hunk.header.clone(),
            old_start: hunk.old_start,
            old_lines: hunk.old_lines,
            new_start: hunk.new_start,
            new_lines: hunk.new_lines,
            lines: hunk
                .lines
                .iter()
                .map(|line| DiffLine {
                    kind: line.kind,
                    content: line.content.clone(),
                    old_lineno: line.old_lineno,
                    new_lineno: line.new_lineno,
                })
                .collect(),
        }
    }

    #[test]
    fn side_by_side_separates_comment_marker_and_source() {
        let comments = [CommentAttachment {
            id: 1,
            line_start: 1,
            line_end: 1,
            resolved: false,
            anchor_status: AnchorStatus::Anchored,
            side_ranges: Vec::new(),
        }];
        let markers = CommentMarkerSet::new(&comments, None);
        let rows = side_by_side_diff_rows(&[], None, Some("source\n"));

        let built = build_side_by_side_diff(
            &DiffStyle::default(),
            Color::Black,
            &[],
            &rows,
            Some("source\n"),
            &[],
            &[],
            &markers,
            Color::Blue,
            80,
        );
        let rendered: String = built.lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();

        assert!(rendered.contains("● source"));
    }

    #[test]
    fn side_by_side_replacement_shows_comment_marker_on_both_columns() {
        let comments = [CommentAttachment {
            id: 1,
            line_start: 1,
            line_end: 1,
            resolved: false,
            anchor_status: AnchorStatus::Anchored,
            side_ranges: Vec::new(),
        }];
        let markers = CommentMarkerSet::new(&comments, None);
        let hunk = review_types::DiffHunk {
            header: "@@ -1 +1 @@".to_string(),
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            lines: vec![
                review_types::DiffLine {
                    kind: LineKind::Deletion,
                    content: "old".to_string(),
                    old_lineno: Some(1),
                    new_lineno: None,
                },
                review_types::DiffLine {
                    kind: LineKind::Addition,
                    content: "new".to_string(),
                    old_lineno: None,
                    new_lineno: Some(1),
                },
            ],
        };
        let model_hunk = model_hunk_from_review(&hunk);
        let rows = side_by_side_diff_rows(&[hunk], None, None);

        let built = build_side_by_side_diff(
            &DiffStyle::default(),
            Color::Black,
            &[model_hunk],
            &rows,
            None,
            &[],
            &[],
            &markers,
            Color::Blue,
            80,
        );
        let rendered: String = built.lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();

        assert_eq!(rendered.matches('●').count(), 2);
        assert!(rendered.contains("● - old"));
        assert!(rendered.contains("● + new"));
    }

    #[test]
    fn side_by_side_replacement_uses_side_specific_comment_marker_ranges() {
        let comments = [CommentAttachment {
            id: 1,
            line_start: 26,
            line_end: 28,
            resolved: false,
            anchor_status: AnchorStatus::Anchored,
            side_ranges: vec![
                CommentAttachmentRange {
                    side: CommentAnchorSide::Base,
                    line_start: 26,
                    line_end: 26,
                },
                CommentAttachmentRange {
                    side: CommentAnchorSide::Head,
                    line_start: 27,
                    line_end: 28,
                },
            ],
        }];
        let markers = CommentMarkerSet::new(&comments, None);
        let hunk = review_types::DiffHunk {
            header: "@@ -26 +27,2 @@".to_string(),
            old_start: 26,
            old_lines: 1,
            new_start: 27,
            new_lines: 2,
            lines: vec![
                review_types::DiffLine {
                    kind: LineKind::Deletion,
                    content: "old line".to_string(),
                    old_lineno: Some(26),
                    new_lineno: None,
                },
                review_types::DiffLine {
                    kind: LineKind::Addition,
                    content: "new line one".to_string(),
                    old_lineno: None,
                    new_lineno: Some(27),
                },
                review_types::DiffLine {
                    kind: LineKind::Addition,
                    content: "new line two".to_string(),
                    old_lineno: None,
                    new_lineno: Some(28),
                },
            ],
        };
        let model_hunk = model_hunk_from_review(&hunk);

        let base_content = (1..=32)
            .map(|n| format!("base {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let head_content = (1..=34)
            .map(|n| format!("head {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let rows = side_by_side_diff_rows(&[hunk], Some(&base_content), Some(&head_content));

        let built = build_side_by_side_diff(
            &DiffStyle::default(),
            Color::Black,
            &[model_hunk],
            &rows,
            Some(&head_content),
            &[],
            &[],
            &markers,
            Color::Blue,
            120,
        );
        let replacement_start: String = built.lines[26]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        let replacement_tail: String = built.lines[27]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();

        assert_eq!(replacement_start.matches('●').count(), 2);
        assert!(replacement_start.contains("● - old line"));
        assert!(replacement_start.contains("● + new line one"));
        assert_eq!(replacement_tail.matches('●').count(), 1);
        assert!(replacement_tail.contains("● + new line two"));
    }

    #[test]
    fn side_by_side_change_rows_do_not_overflow_inner_width() {
        let hunk = review_types::DiffHunk {
            header: "@@ -94 +97,2 @@".to_string(),
            old_start: 94,
            old_lines: 1,
            new_start: 97,
            new_lines: 2,
            lines: vec![
                review_types::DiffLine {
                    kind: LineKind::Deletion,
                    content: "pub fn navigate_to_comment_id(state: &mut AppState) {"
                        .to_string(),
                    old_lineno: Some(94),
                    new_lineno: None,
                },
                review_types::DiffLine {
                    kind: LineKind::Addition,
                    content: "pub fn navigate_to_comment_id(state: &mut AppState, view: &impl AppViewport) {"
                        .to_string(),
                    old_lineno: None,
                    new_lineno: Some(97),
                },
                review_types::DiffLine {
                    kind: LineKind::Addition,
                    content: "    view: &impl AppViewport,".to_string(),
                    old_lineno: None,
                    new_lineno: Some(98),
                },
            ],
        };
        let model_hunk = model_hunk_from_review(&hunk);
        let rows = side_by_side_diff_rows(&[hunk], None, None);
        let inner_w = 50;

        let built = build_side_by_side_diff(
            &DiffStyle::default(),
            Color::Black,
            &[model_hunk],
            &rows,
            None,
            &[],
            &[],
            &CommentMarkerSet::new(&[], None),
            Color::Blue,
            inner_w,
        );

        assert!(built.lines.len() >= 2);
        for line in &built.lines {
            let rendered_width: usize = line
                .spans
                .iter()
                .map(|span| span.content.chars().count())
                .sum();
            assert!(rendered_width <= inner_w, "{rendered_width} > {inner_w}");
        }
    }
}
