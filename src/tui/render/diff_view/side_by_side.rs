use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use super::content::BuiltContent;
use super::line::{BLAME_COL_WIDTH, digit_width, format_blame};
use crate::app::model::{BlameLine, DiffHunk};
use crate::config::DiffStyle;
use crate::review_types::LineKind;

// ---------------------------------------------------------------------------
// Side-by-side diff
// ---------------------------------------------------------------------------

pub(super) fn build_side_by_side_diff(
    ds: &DiffStyle,
    default_bg: Color,
    hunks: &[DiffHunk],
    head_content: Option<&str>,
    head_blame: &[BlameLine],
    base_blame: &[BlameLine],
    inner_w: usize,
) -> BuiltContent {
    let head_lines: Vec<&str> = head_content
        .map(|c| c.lines().collect())
        .unwrap_or_default();

    if hunks.is_empty() && head_lines.is_empty() {
        return BuiltContent {
            lines: vec![Line::from("  No changes.")],
            hunk_starts: vec![],
            hunk_ends: vec![],
            hunk_first_changes: vec![],
            gutter_w: 0,
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

    let has_blame = !head_blame.is_empty() || !base_blame.is_empty();
    let blame_w = if has_blame { BLAME_COL_WIDTH + 1 } else { 0 };

    // Each column: [blame + " "] + gutter + " " + content.  Middle divider is " │ ".
    let col_fixed = blame_w + gutter_w + 1; // blame + gutter + space
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
    let mut hunk_starts = Vec::new();
    let mut hunk_ends = Vec::new();
    let mut hunk_first_changes = Vec::new();
    let mut old_cursor: u32 = 1;
    let mut new_cursor: u32 = 1;

    // Helper: build one side of a row (blame + gutter + content), padded to col_w.
    let make_half = |lineno: Option<u32>,
                     content: &str,
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

        if let Some((word_spans, em_style)) = emphasis {
            let mut chars = 0;
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
            let truncated: String = content.chars().take(col_w).collect();
            let pad = col_w.saturating_sub(truncated.chars().count());
            spans.push(Span::styled(truncated, style));
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

    for hunk in hunks {
        // Gap before hunk: context lines.
        while new_cursor < hunk.new_start && (new_cursor as usize) <= head_lines.len() {
            let content = head_lines.get((new_cursor - 1) as usize).unwrap_or(&"");
            let left = make_half(
                Some(old_cursor),
                content,
                context_style,
                None,
                bblame(Some(old_cursor)),
            );
            let right = make_half(
                Some(new_cursor),
                content,
                context_style,
                None,
                hblame(Some(new_cursor)),
            );
            result.push(make_row(left, right));
            old_cursor += 1;
            new_cursor += 1;
        }

        hunk_starts.push(result.len());
        let leading_context = hunk
            .lines
            .iter()
            .take_while(|l| l.kind == LineKind::Context)
            .count();
        hunk_first_changes.push(result.len() + leading_context);

        // Process hunk lines: collect deletion/addition blocks and pair them.
        let hunk_lines = &hunk.lines;
        let mut li = 0;
        while li < hunk_lines.len() {
            let dl = &hunk_lines[li];

            if dl.kind == LineKind::Context {
                let content = dl.content.trim_end_matches('\n');
                let left = make_half(
                    dl.old_lineno,
                    content,
                    context_style,
                    None,
                    bblame(dl.old_lineno),
                );
                let right = make_half(
                    dl.new_lineno,
                    content,
                    context_style,
                    None,
                    hblame(dl.new_lineno),
                );
                result.push(make_row(left, right));
                old_cursor += 1;
                new_cursor += 1;
                li += 1;
                continue;
            }

            // Collect consecutive deletions then additions.
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
            let pair_count = dels.len().min(adds.len());
            let max_count = dels.len().max(adds.len());

            // Pre-compute word diffs for paired lines.
            let word_diffs: Vec<_> = (0..pair_count)
                .map(|idx| {
                    let dc = dels[idx].content.trim_end_matches('\n');
                    let ac = adds[idx].content.trim_end_matches('\n');
                    super::super::word_diff::compute(dc, ac)
                })
                .collect();

            for idx in 0..max_count {
                let left = if idx < dels.len() {
                    let dc = dels[idx].content.trim_end_matches('\n');
                    if idx < pair_count {
                        if let Some((ref old_spans, _)) = word_diffs[idx] {
                            make_half(
                                dels[idx].old_lineno,
                                dc,
                                deletion_style,
                                Some((old_spans, deletion_emphasis)),
                                bblame(dels[idx].old_lineno),
                            )
                        } else {
                            make_half(
                                dels[idx].old_lineno,
                                dc,
                                deletion_style,
                                None,
                                bblame(dels[idx].old_lineno),
                            )
                        }
                    } else {
                        make_half(
                            dels[idx].old_lineno,
                            dc,
                            deletion_style,
                            None,
                            bblame(dels[idx].old_lineno),
                        )
                    }
                } else {
                    make_empty_half()
                };

                let right = if idx < adds.len() {
                    let ac = adds[idx].content.trim_end_matches('\n');
                    if idx < pair_count {
                        if let Some((_, ref new_spans)) = word_diffs[idx] {
                            make_half(
                                adds[idx].new_lineno,
                                ac,
                                addition_style,
                                Some((new_spans, addition_emphasis)),
                                hblame(adds[idx].new_lineno),
                            )
                        } else {
                            make_half(
                                adds[idx].new_lineno,
                                ac,
                                addition_style,
                                None,
                                hblame(adds[idx].new_lineno),
                            )
                        }
                    } else {
                        make_half(
                            adds[idx].new_lineno,
                            ac,
                            addition_style,
                            None,
                            hblame(adds[idx].new_lineno),
                        )
                    }
                } else {
                    make_empty_half()
                };

                result.push(make_row(left, right));
                if idx < dels.len() {
                    old_cursor += 1;
                }
                if idx < adds.len() {
                    new_cursor += 1;
                }
            }

            li = add_end;
        }

        hunk_ends.push(result.len());
    }

    // Gap after last hunk.
    while (new_cursor as usize) <= head_lines.len() {
        let content = head_lines.get((new_cursor - 1) as usize).unwrap_or(&"");
        let left = make_half(
            Some(old_cursor),
            content,
            context_style,
            None,
            bblame(Some(old_cursor)),
        );
        let right = make_half(
            Some(new_cursor),
            content,
            context_style,
            None,
            hblame(Some(new_cursor)),
        );
        result.push(make_row(left, right));
        old_cursor += 1;
        new_cursor += 1;
    }

    BuiltContent {
        lines: result,
        hunk_starts,
        hunk_ends,
        hunk_first_changes,
        gutter_w,
    }
}
