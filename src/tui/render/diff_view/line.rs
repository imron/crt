use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::app::model::BlameLine;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Width of the blame annotation column (hash + date + author).
pub const BLAME_COL_WIDTH: usize = 30;

/// Format a blame annotation for display, padded/truncated to `BLAME_COL_WIDTH`.
pub fn format_blame(blame: Option<&BlameLine>) -> String {
    match blame {
        Some(bl) => {
            // "abc1234 2024-03-15 Author" — hash(7) + space + date(10) + space + author.
            let author_budget = BLAME_COL_WIDTH.saturating_sub(19); // 7 + 1 + 10 + 1
            let author: String = bl.author.chars().take(author_budget).collect();
            let text = format!("{} {} {author}", bl.hash, bl.date);
            // Pad to fixed width.
            format!("{text:<BLAME_COL_WIDTH$}")
        }
        None => " ".repeat(BLAME_COL_WIDTH),
    }
}

/// Build a styled line with optional old/new line number gutters.
///
/// The gutter inherits the background color from `content_style` so that
/// hunk-highlighted rows have a consistent background across the full width.
/// The line is padded with spaces so the background extends to the panel edge.
pub fn make_line(
    old_lineno: Option<u32>,
    new_lineno: Option<u32>,
    comment_marker: &str,
    prefix: &str,
    content: &str,
    content_style: Style,
    gutter_fg: Color,
    blame_fg: Color,
    default_bg: Color,
    gutter_w: usize,
    inner_w: usize,
    blame: Option<&BlameLine>,
) -> Line<'static> {
    let bg = content_style.bg.unwrap_or(default_bg);
    let gutter_style = Style::default().fg(gutter_fg).bg(bg);

    let mut parts = Vec::new();

    let blame_cols = if blame.is_some() {
        let blame_style = Style::default().fg(blame_fg).bg(bg);
        parts.push(Span::styled(format_blame(blame), blame_style));
        parts.push(Span::styled(" ", gutter_style));
        BLAME_COL_WIDTH + 1
    } else {
        0
    };

    let old_num = match old_lineno {
        Some(n) => format!("{n:>gutter_w$}"),
        None => " ".repeat(gutter_w),
    };
    let new_num = match new_lineno {
        Some(n) => format!("{n:>gutter_w$}"),
        None => " ".repeat(gutter_w),
    };

    parts.push(Span::styled(old_num, gutter_style));
    parts.push(Span::styled(" ", gutter_style));
    parts.push(Span::styled(new_num, gutter_style));
    if !comment_marker.is_empty() {
        parts.push(Span::styled(comment_marker.to_string(), gutter_style));
    }
    parts.push(Span::styled(format!(" {prefix} "), content_style));

    // Width consumed by blame + gutters + separator + prefix.
    let fixed_cols = blame_cols + gutter_w + 1 + gutter_w + comment_marker.chars().count() + 3;
    let content_cols = inner_w.saturating_sub(fixed_cols);
    let visible_len = content.chars().count();
    let pad = content_cols.saturating_sub(visible_len);

    parts.push(Span::styled(content.to_string(), content_style));
    parts.push(Span::styled(" ".repeat(pad), content_style));

    Line::from(parts)
}

/// Build a styled line with word-level emphasis highlighting.
///
/// Like `make_line` but instead of a single content string, takes
/// word-diff spans and uses `emphasis_style` for changed portions.
pub fn make_line_with_emphasis(
    old_lineno: Option<u32>,
    new_lineno: Option<u32>,
    comment_marker: &str,
    prefix: &str,
    spans: &[super::super::word_diff::DiffSpan<'_>],
    base_style: Style,
    emphasis_style: Style,
    gutter_fg: Color,
    blame_fg: Color,
    default_bg: Color,
    gutter_w: usize,
    inner_w: usize,
    blame: Option<&BlameLine>,
) -> Line<'static> {
    let bg = base_style.bg.unwrap_or(default_bg);
    let gutter_style = Style::default().fg(gutter_fg).bg(bg);

    let mut parts = Vec::new();

    let blame_cols = if blame.is_some() {
        let blame_style = Style::default().fg(blame_fg).bg(bg);
        parts.push(Span::styled(format_blame(blame), blame_style));
        parts.push(Span::styled(" ", gutter_style));
        BLAME_COL_WIDTH + 1
    } else {
        0
    };

    let old_num = match old_lineno {
        Some(n) => format!("{n:>gutter_w$}"),
        None => " ".repeat(gutter_w),
    };
    let new_num = match new_lineno {
        Some(n) => format!("{n:>gutter_w$}"),
        None => " ".repeat(gutter_w),
    };

    parts.push(Span::styled(old_num, gutter_style));
    parts.push(Span::styled(" ", gutter_style));
    parts.push(Span::styled(new_num, gutter_style));
    if !comment_marker.is_empty() {
        parts.push(Span::styled(comment_marker.to_string(), gutter_style));
    }
    parts.push(Span::styled(format!(" {prefix} "), base_style));

    let fixed_cols = blame_cols + gutter_w + 1 + gutter_w + comment_marker.chars().count() + 3;
    let content_cols = inner_w.saturating_sub(fixed_cols);

    let mut char_count = 0;
    for span in spans {
        let (text, style) = match span {
            super::super::word_diff::DiffSpan::Common(t) => (*t, base_style),
            super::super::word_diff::DiffSpan::Changed(t) => (*t, emphasis_style),
        };
        char_count += text.chars().count();
        parts.push(Span::styled(text.to_string(), style));
    }

    // Pad to fill the row.
    let pad = content_cols.saturating_sub(char_count);
    if pad > 0 {
        parts.push(Span::styled(" ".repeat(pad), base_style));
    }

    Line::from(parts)
}

/// Number of decimal digits needed to display `n` (minimum 3).
pub fn digit_width(n: u32) -> usize {
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
