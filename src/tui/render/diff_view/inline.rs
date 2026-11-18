use ratatui::style::{Color, Style};
use ratatui::text::Line;

use super::comment_markers::CommentMarkerSet;
use super::content::BuiltContent;
use super::line::{digit_width, make_line, make_line_with_emphasis};
use crate::app::model::{BlameLine, DiffHunk};
use crate::config::DiffStyle;
use crate::review_types::LineKind;

// ---------------------------------------------------------------------------
// Inline diff: full file with additions + deletions interleaved
// ---------------------------------------------------------------------------

pub fn build_inline_diff(
    ds: &DiffStyle,
    default_bg: Color,
    hunks: &[DiffHunk],
    head_content: Option<&str>,
    head_blame: &[BlameLine],
    base_blame: &[BlameLine],
    comment_markers: &CommentMarkerSet,
    inner_w: usize,
) -> BuiltContent {
    let head_lines: Vec<&str> = head_content
        .map(|c| c.lines().collect())
        .unwrap_or_default();

    // No hunks (e.g. whitespace-only changes with -w): show the HEAD file.
    if hunks.is_empty() {
        if head_lines.is_empty() {
            return BuiltContent {
                lines: vec![Line::from("  No changes.")],
                hunk_starts: vec![],
                hunk_ends: vec![],
                hunk_first_changes: vec![],
                gutter_w: 0,
                comment_marker_w: 0,
            };
        }
        let gutter_w = digit_width(head_lines.len() as u32);
        let context_style = Style::default().fg(*ds.context_fg);
        let gutter_fg = *ds.gutter_fg;
        let blame_fg = *ds.blame_fg;
        let lines: Vec<Line<'static>> = head_lines
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let lineno = (i + 1) as u32;
                let bl = head_blame.get(i);
                make_line(
                    Some(lineno),
                    Some(lineno),
                    &comment_markers.marker_for_line(Some(lineno)),
                    " ",
                    text,
                    context_style,
                    gutter_fg,
                    blame_fg,
                    default_bg,
                    gutter_w,
                    inner_w,
                    bl,
                )
            })
            .collect();
        return BuiltContent {
            lines,
            hunk_starts: vec![],
            hunk_ends: vec![],
            hunk_first_changes: vec![],
            gutter_w,
            comment_marker_w: comment_markers.width(),
        };
    }

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

    let mut result = Vec::new();
    let mut hunk_starts = Vec::new();
    let mut hunk_ends = Vec::new();
    let mut hunk_first_changes = Vec::new();
    let mut old_cursor: u32 = 1;
    let mut new_cursor: u32 = 1;

    for hunk in hunks {
        // Gap before this hunk: unchanged lines from HEAD.
        while new_cursor < hunk.new_start && (new_cursor as usize) <= head_lines.len() {
            let content = head_lines.get((new_cursor - 1) as usize).unwrap_or(&"");
            result.push(make_line(
                Some(old_cursor),
                Some(new_cursor),
                &comment_markers.marker_for_line(Some(new_cursor)),
                " ",
                content,
                context_style,
                gutter_fg,
                blame_fg,
                default_bg,
                gutter_w,
                inner_w,
                hblame(Some(new_cursor)),
            ));
            old_cursor += 1;
            new_cursor += 1;
        }

        // Record hunk start.
        hunk_starts.push(result.len());

        // Find the display row of the first actual change in this hunk.
        // Count leading context lines to compute the offset.
        let leading_context = hunk
            .lines
            .iter()
            .take_while(|l| l.kind == LineKind::Context)
            .count();
        hunk_first_changes.push(result.len() + leading_context);

        // Hunk lines — faint background on changed lines to delineate hunks.
        // Process lines in blocks: consecutive deletions followed by
        // consecutive additions form a "change pair" that gets word-level
        // diff highlighting.
        let hunk_lines = &hunk.lines;
        let mut li = 0;
        while li < hunk_lines.len() {
            let diff_line = &hunk_lines[li];

            if diff_line.kind == LineKind::Context {
                let content = diff_line.content.trim_end_matches('\n');
                result.push(make_line(
                    diff_line.old_lineno,
                    diff_line.new_lineno,
                    &comment_markers.marker_for_line(diff_line.new_lineno),
                    " ",
                    content,
                    context_style,
                    gutter_fg,
                    blame_fg,
                    default_bg,
                    gutter_w,
                    inner_w,
                    hblame(diff_line.new_lineno),
                ));
                old_cursor += 1;
                new_cursor += 1;
                li += 1;
                continue;
            }

            // Collect a block of consecutive deletions then additions.
            let block_start = li;
            let mut del_end = li;
            while del_end < hunk_lines.len() && hunk_lines[del_end].kind == LineKind::Deletion {
                del_end += 1;
            }
            let mut add_end = del_end;
            while add_end < hunk_lines.len() && hunk_lines[add_end].kind == LineKind::Addition {
                add_end += 1;
            }

            let dels = &hunk_lines[block_start..del_end];
            let adds = &hunk_lines[del_end..add_end];

            // Pair deletions with additions for word-level diff.
            let pair_count = dels.len().min(adds.len());

            // Pre-compute word diffs for paired lines.
            let word_diffs: Vec<_> = (0..pair_count)
                .map(|idx| {
                    let dc = dels[idx].content.trim_end_matches('\n');
                    let ac = adds[idx].content.trim_end_matches('\n');
                    super::super::word_diff::compute(dc, ac)
                })
                .collect();

            for (idx, dl) in dels.iter().enumerate() {
                let content = dl.content.trim_end_matches('\n');
                let bl = bblame(dl.old_lineno);
                if idx < pair_count {
                    if let Some((ref old_spans, _)) = word_diffs[idx] {
                        result.push(make_line_with_emphasis(
                            dl.old_lineno,
                            dl.new_lineno,
                            &comment_markers.marker_for_line(dl.new_lineno),
                            "-",
                            old_spans,
                            Style::default().fg(*ds.deletion_fg).bg(*ds.deletion_bg),
                            Style::default()
                                .fg(*ds.deletion_fg)
                                .bg(*ds.deletion_emphasis_bg),
                            gutter_fg,
                            blame_fg,
                            default_bg,
                            gutter_w,
                            inner_w,
                            bl,
                        ));
                    } else {
                        result.push(make_line(
                            dl.old_lineno,
                            dl.new_lineno,
                            &comment_markers.marker_for_line(dl.new_lineno),
                            "-",
                            content,
                            Style::default().fg(*ds.deletion_fg).bg(*ds.deletion_bg),
                            gutter_fg,
                            blame_fg,
                            default_bg,
                            gutter_w,
                            inner_w,
                            bl,
                        ));
                    }
                } else {
                    result.push(make_line(
                        dl.old_lineno,
                        dl.new_lineno,
                        &comment_markers.marker_for_line(dl.new_lineno),
                        "-",
                        content,
                        Style::default().fg(*ds.deletion_fg).bg(*ds.deletion_bg),
                        gutter_fg,
                        blame_fg,
                        default_bg,
                        gutter_w,
                        inner_w,
                        bl,
                    ));
                }
                old_cursor += 1;
            }

            for (idx, al) in adds.iter().enumerate() {
                let content = al.content.trim_end_matches('\n');
                let bl = hblame(al.new_lineno);
                if idx < pair_count {
                    if let Some((_, ref new_spans)) = word_diffs[idx] {
                        result.push(make_line_with_emphasis(
                            al.old_lineno,
                            al.new_lineno,
                            &comment_markers.marker_for_line(al.new_lineno),
                            "+",
                            new_spans,
                            Style::default().fg(*ds.addition_fg).bg(*ds.addition_bg),
                            Style::default()
                                .fg(*ds.addition_fg)
                                .bg(*ds.addition_emphasis_bg),
                            gutter_fg,
                            blame_fg,
                            default_bg,
                            gutter_w,
                            inner_w,
                            bl,
                        ));
                    } else {
                        result.push(make_line(
                            al.old_lineno,
                            al.new_lineno,
                            &comment_markers.marker_for_line(al.new_lineno),
                            "+",
                            content,
                            Style::default().fg(*ds.addition_fg).bg(*ds.addition_bg),
                            gutter_fg,
                            blame_fg,
                            default_bg,
                            gutter_w,
                            inner_w,
                            bl,
                        ));
                    }
                } else {
                    result.push(make_line(
                        al.old_lineno,
                        al.new_lineno,
                        &comment_markers.marker_for_line(al.new_lineno),
                        "+",
                        content,
                        Style::default().fg(*ds.addition_fg).bg(*ds.addition_bg),
                        gutter_fg,
                        blame_fg,
                        default_bg,
                        gutter_w,
                        inner_w,
                        bl,
                    ));
                }
                new_cursor += 1;
            }

            li = add_end;
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
            &comment_markers.marker_for_line(Some(new_cursor)),
            " ",
            content,
            context_style,
            gutter_fg,
            blame_fg,
            default_bg,
            gutter_w,
            inner_w,
            hblame(Some(new_cursor)),
        ));
        old_cursor += 1;
        new_cursor += 1;
    }

    BuiltContent {
        lines: result,
        hunk_starts,
        hunk_ends,
        hunk_first_changes,
        gutter_w,
        comment_marker_w: comment_markers.width(),
    }
}
