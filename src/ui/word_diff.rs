//! Word-level diff highlighting within changed lines.
//!
//! Given a pair of old/new lines, computes which spans within each line
//! changed and which are common. This is used to apply an emphasized
//! background to the parts that actually differ.

/// A span of text that is either common (unchanged) or changed.
#[derive(Debug, PartialEq)]
pub enum DiffSpan<'a> {
    Common(&'a str),
    Changed(&'a str),
}

/// A token is a (byte_start, byte_end) range in the original string.
#[derive(Debug, Clone, Copy)]
struct Token {
    start: usize,
    end: usize,
}

/// Tokenize a string into words and non-word runs.
/// Each token preserves its byte offsets into the original string.
/// Handles multi-byte UTF-8 characters correctly.
fn tokenize(s: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let start = chars[i].0;
        if chars[i].1.is_alphanumeric() || chars[i].1 == '_' {
            // Word token: consecutive alphanumeric/underscore chars.
            while i < chars.len() && (chars[i].1.is_alphanumeric() || chars[i].1 == '_') {
                i += 1;
            }
        } else {
            // Single non-word character (whitespace, punctuation, emoji, etc.).
            i += 1;
        }
        let end = if i < chars.len() { chars[i].0 } else { s.len() };
        tokens.push(Token { start, end });
    }
    tokens
}

/// Build the LCS table, comparing tokens by their text content.
fn lcs_table_with_strings(
    old: &str,
    new: &str,
    old_tokens: &[Token],
    new_tokens: &[Token],
) -> Vec<Vec<u16>> {
    let m = old_tokens.len();
    let n = new_tokens.len();
    let mut table = vec![vec![0u16; n + 1]; m + 1];

    for i in 1..=m {
        for j in 1..=n {
            let ot = &old[old_tokens[i - 1].start..old_tokens[i - 1].end];
            let nt = &new[new_tokens[j - 1].start..new_tokens[j - 1].end];
            if ot == nt {
                table[i][j] = table[i - 1][j - 1] + 1;
            } else {
                table[i][j] = table[i - 1][j].max(table[i][j - 1]);
            }
        }
    }

    table
}

/// Walk the LCS table backwards to produce diff spans for both strings.
fn build_spans_from_table<'a>(
    old: &'a str,
    new: &'a str,
    old_tokens: &[Token],
    new_tokens: &[Token],
    table: &[Vec<u16>],
) -> (Vec<DiffSpan<'a>>, Vec<DiffSpan<'a>>) {
    let mut old_spans: Vec<DiffSpan<'a>> = Vec::new();
    let mut new_spans: Vec<DiffSpan<'a>> = Vec::new();

    let mut i = old_tokens.len();
    let mut j = new_tokens.len();

    // Collect operations in reverse, then reverse at the end.
    #[derive(Debug)]
    enum Op {
        Common,
        OldOnly,
        NewOnly,
    }
    let mut ops = Vec::new();

    while i > 0 && j > 0 {
        let ot = &old[old_tokens[i - 1].start..old_tokens[i - 1].end];
        let nt = &new[new_tokens[j - 1].start..new_tokens[j - 1].end];
        if ot == nt {
            ops.push((Op::Common, i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if table[i - 1][j] >= table[i][j - 1] {
            ops.push((Op::OldOnly, i - 1, 0));
            i -= 1;
        } else {
            ops.push((Op::NewOnly, 0, j - 1));
            j -= 1;
        }
    }
    while i > 0 {
        ops.push((Op::OldOnly, i - 1, 0));
        i -= 1;
    }
    while j > 0 {
        ops.push((Op::NewOnly, 0, j - 1));
        j -= 1;
    }
    ops.reverse();

    // Merge consecutive same-type spans for cleaner output.
    for (op, oi, ni) in &ops {
        match op {
            Op::Common => {
                let tok = &old_tokens[*oi];
                let text = &old[tok.start..tok.end];
                merge_span(&mut old_spans, DiffSpan::Common(text));
                let tok = &new_tokens[*ni];
                let text = &new[tok.start..tok.end];
                merge_span(&mut new_spans, DiffSpan::Common(text));
            }
            Op::OldOnly => {
                let tok = &old_tokens[*oi];
                let text = &old[tok.start..tok.end];
                merge_span(&mut old_spans, DiffSpan::Changed(text));
            }
            Op::NewOnly => {
                let tok = &new_tokens[*ni];
                let text = &new[tok.start..tok.end];
                merge_span(&mut new_spans, DiffSpan::Changed(text));
            }
        }
    }

    (old_spans, new_spans)
}

/// Merge a new span into the list, coalescing with the previous span
/// if they are the same variant.
fn merge_span<'a>(spans: &mut Vec<DiffSpan<'a>>, span: DiffSpan<'a>) {
    // We can't truly merge &str slices unless they are contiguous.
    // Instead, just push — the rendering will handle consecutive same-type
    // spans efficiently.
    spans.push(span);
}

// ---------------------------------------------------------------------------
// Revised public API (cleaner)
// ---------------------------------------------------------------------------

/// Compute word-level diff between two lines.
///
/// Returns `Some((old_spans, new_spans))` if the lines are similar enough
/// for word-level highlighting to be useful (>40% of tokens in common).
/// Returns `None` if the lines are too different — the caller should
/// render them without emphasis.
pub fn compute<'a>(old: &'a str, new: &'a str) -> Option<(Vec<DiffSpan<'a>>, Vec<DiffSpan<'a>>)> {
    let old_tokens = tokenize(old);
    let new_tokens = tokenize(new);

    // Quick check: if either line is empty, no word-level diff.
    if old_tokens.is_empty() || new_tokens.is_empty() {
        return None;
    }

    // Compute LCS length to check similarity before building full spans.
    let table = lcs_table_with_strings(old, new, &old_tokens, &new_tokens);
    let lcs_len = table[old_tokens.len()][new_tokens.len()] as usize;
    let max_tokens = old_tokens.len().max(new_tokens.len());

    // If less than 40% of tokens are common, the lines are too different.
    if lcs_len * 100 / max_tokens < 40 {
        return None;
    }

    let (old_spans, new_spans) = build_spans_from_table(old, new, &old_tokens, &new_tokens, &table);

    // If everything is common (no changes), no emphasis needed.
    let has_changes = old_spans.iter().any(|s| matches!(s, DiffSpan::Changed(_)))
        || new_spans.iter().any(|s| matches!(s, DiffSpan::Changed(_)));
    if !has_changes {
        return None;
    }

    Some((old_spans, new_spans))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_lines() {
        // Identical lines return None (no emphasis needed).
        assert!(compute("hello world", "hello world").is_none());
    }

    #[test]
    fn test_single_word_change() {
        let (old, new) = compute("hello world", "hello earth").unwrap();
        let old_changed: String = old
            .iter()
            .filter_map(|s| match s {
                DiffSpan::Changed(t) => Some(*t),
                _ => None,
            })
            .collect();
        let new_changed: String = new
            .iter()
            .filter_map(|s| match s {
                DiffSpan::Changed(t) => Some(*t),
                _ => None,
            })
            .collect();
        assert_eq!(old_changed, "world");
        assert_eq!(new_changed, "earth");
    }

    #[test]
    fn test_completely_different() {
        // Completely different lines return None (below similarity threshold).
        assert!(compute("aaa", "bbb").is_none());
    }

    #[test]
    fn test_empty() {
        // Empty line returns None.
        assert!(compute("", "hello").is_none());
    }

    #[test]
    fn test_dissimilar_lines() {
        // Lines that are mostly different should return None.
        assert!(compute(
            "/// This struct groups the reviewer identity",
            "/// Stored in `task_reviews` table. The latest review is joined"
        )
        .is_none());
    }

    #[test]
    fn test_multibyte_utf8() {
        // The em-dash '—' is 3 bytes in UTF-8. This must not panic.
        let result = compute(
            r#"        println!("crt — Code Review Tool\n");"#,
            r#"        println!("crt — Code Review Tool");"#,
        );
        // Should return Some since the lines are very similar.
        let (old, _) = result.unwrap();
        let old_text: String = old
            .iter()
            .map(|s| match s {
                DiffSpan::Common(t) | DiffSpan::Changed(t) => *t,
            })
            .collect();
        assert!(old_text.contains("crt — Code"));
    }

    #[test]
    fn test_unicode_emoji() {
        let (old, _) = compute("hello 🌍 world", "hello 🌎 world").unwrap();
        let old_changed: String = old
            .iter()
            .filter_map(|s| match s {
                DiffSpan::Changed(t) => Some(*t),
                _ => None,
            })
            .collect();
        assert_eq!(old_changed, "🌍");
    }
}
