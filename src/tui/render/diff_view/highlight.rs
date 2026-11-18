use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

// ---------------------------------------------------------------------------
// Column cursor overlay
// ---------------------------------------------------------------------------

/// Apply a block cursor (inverted colors) at a specific character position
/// within the content portion of a line.
///
/// `content_start_col` is the number of fixed columns (gutter + prefix) before
/// actual content begins. `col_cursor` is the character offset within content.
pub(super) fn apply_col_cursor(
    line: &Line,
    content_start_col: usize,
    col_cursor: usize,
) -> Line<'static> {
    // Compute the target character index in the flattened line text.
    // We need to count characters (not bytes) through the spans to find
    // the right position.
    let target_char = content_start_col + col_cursor;

    let mut result: Vec<Span<'static>> = Vec::new();
    let mut char_pos: usize = 0;
    let mut applied = false;

    for span in &line.spans {
        let text = span.content.as_ref();
        let span_char_len = text.chars().count();
        let span_char_start = char_pos;
        let span_char_end = char_pos + span_char_len;

        if !applied && target_char >= span_char_start && target_char < span_char_end {
            // The cursor character is in this span. Split it.
            let local_char_idx = target_char - span_char_start;

            // Before the cursor.
            if local_char_idx > 0 {
                let before: String = text.chars().take(local_char_idx).collect();
                result.push(Span::styled(before, span.style));
            }

            // The cursor character — invert fg/bg.
            let cursor_char: String = text.chars().skip(local_char_idx).take(1).collect();
            let cursor_style = Style::default()
                .fg(span.style.bg.unwrap_or(Color::Black))
                .bg(span.style.fg.unwrap_or(Color::White));
            result.push(Span::styled(cursor_char, cursor_style));

            // After the cursor.
            if local_char_idx + 1 < span_char_len {
                let after: String = text.chars().skip(local_char_idx + 1).collect();
                result.push(Span::styled(after, span.style));
            }

            applied = true;
        } else {
            result.push(Span::styled(text.to_string(), span.style));
        }

        char_pos = span_char_end;
    }

    // If the cursor is past the end of the line (empty line), add a block cursor.
    if !applied {
        let cursor_style = Style::default().fg(Color::Black).bg(Color::White);
        result.push(Span::styled(" ", cursor_style));
    }

    Line::from(result)
}

// ---------------------------------------------------------------------------
// Search match highlighting
// ---------------------------------------------------------------------------

/// Apply search match highlights to a line by splitting spans at match
/// boundaries and overriding the background color.
///
/// `matches` is a list of (byte_start, byte_end, is_current_match) relative
/// to the flattened span text. `cursor_line_bg` is applied to non-match
/// portions if the line is the cursor line.
pub(super) fn apply_search_highlights(
    line: &Line,
    matches: &[(usize, usize, bool)],
    match_bg: Color,
    current_bg: Color,
    cursor_line_bg: Option<Color>,
) -> Vec<Span<'static>> {
    // Flatten spans into (text, style, byte_offset) segments.
    let mut result: Vec<Span<'static>> = Vec::new();
    let mut byte_pos: usize = 0;

    for span in &line.spans {
        let text = span.content.as_ref();
        let span_start = byte_pos;
        let span_end = byte_pos + text.len();

        // Find all match regions that overlap this span.
        let mut cursor = span_start;
        for &(m_start, m_end, is_current) in matches {
            if m_end <= span_start || m_start >= span_end {
                continue; // no overlap
            }
            let overlap_start = m_start.max(span_start);
            let overlap_end = m_end.min(span_end);

            // Emit text before this match overlap.
            if cursor < overlap_start {
                let before = &text[(cursor - span_start)..(overlap_start - span_start)];
                let style = if let Some(bg) = cursor_line_bg {
                    span.style.bg(bg)
                } else {
                    span.style
                };
                result.push(Span::styled(before.to_string(), style));
            }

            // Emit the match highlight.
            let match_text = &text[(overlap_start - span_start)..(overlap_end - span_start)];
            let bg = if is_current { current_bg } else { match_bg };
            let style = span.style.bg(bg).fg(Color::Black);
            result.push(Span::styled(match_text.to_string(), style));

            cursor = overlap_end;
        }

        // Emit remaining text after all matches.
        if cursor < span_end {
            let remaining = &text[(cursor - span_start)..];
            let style = if let Some(bg) = cursor_line_bg {
                span.style.bg(bg)
            } else {
                span.style
            };
            result.push(Span::styled(remaining.to_string(), style));
        }

        byte_pos = span_end;
    }

    result
}
