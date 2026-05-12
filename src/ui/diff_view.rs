//! Diff/file rendering: inline diff within full file context, full-file
//! mode with change highlighting, and reviewed-file summary.

use std::collections::HashSet;

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::AppState;
use crate::config::DiffStyle;
use crate::model::{ContentMode, DiffHunk, LineKind, PaneFocus, RenderVariant, ReviewStatus};

// ---------------------------------------------------------------------------
// Diff line cache — avoids rebuilding all Line<'static> every frame
// ---------------------------------------------------------------------------

/// Identity key for cached diff content. When this key matches the previous
/// frame, we can reuse the cached `Line` vectors instead of running the
/// (expensive) word-diff and line-building code again.
#[derive(PartialEq, Eq)]
pub struct DiffCacheKey {
    selected_file: usize,
    content_mode: ContentMode,
    render_variant: RenderVariant,
    show_blame: bool,
    reviewed_diff_expanded: bool,
    inner_w: usize,
    ignore_whitespace: bool,
    diff_algorithm: crate::config::DiffAlgorithm,
    /// Cheap identity for head_content: (ptr, len). The String is heap-
    /// allocated and kept alive in AppState, so ptr+len is stable as long as
    /// the same String is referenced.
    head_content_id: Option<(usize, usize)>,
    /// Same for base_content.
    base_content_id: Option<(usize, usize)>,
    head_blame_len: usize,
    base_blame_len: usize,
    /// Stable hash of the diff content (from DiffContent::diff_hash).
    diff_hash: String,
}

/// Cached output of `build_content()`.
pub struct DiffCache {
    pub key: DiffCacheKey,
    pub lines: Vec<Line<'static>>,
    pub hunk_starts: Vec<usize>,
    pub hunk_ends: Vec<usize>,
    pub hunk_first_changes: Vec<usize>,
    pub gutter_w: usize,
    pub rendered_text: Vec<String>,
}

/// Helper: get cheap identity for an `Option<String>` — pointer + length.
fn string_id(s: &Option<String>) -> Option<(usize, usize)> {
    s.as_ref().map(|s| (s.as_ptr() as usize, s.len()))
}

/// Draw the diff/file view pane.
pub fn draw(frame: &mut Frame, state: &mut AppState, area: Rect) {
    let focused = state.pane_focus == PaneFocus::Diff;
    let border_style = super::pane_border_style(&state.styles.panel, focused);

    // Inner width excludes left and right borders.
    let inner_w = area.width.saturating_sub(2) as usize;

    // Build the cache key from current state.
    let new_key = DiffCacheKey {
        selected_file: state.selected_file,
        content_mode: state.content_mode,
        render_variant: state.render_variant,
        show_blame: state.show_blame,
        reviewed_diff_expanded: state.reviewed_diff_expanded,
        inner_w,
        ignore_whitespace: state.ignore_whitespace,
        diff_algorithm: state.diff_algorithm,
        head_content_id: string_id(&state.head_content),
        base_content_id: string_id(&state.base_content),
        head_blame_len: state.head_blame.len(),
        base_blame_len: state.base_blame.len(),
        diff_hash: state
            .selected_file_entry()
            .map(|e| e.diff.diff_hash.clone())
            .unwrap_or_default(),
    };

    // Check if the cache is still valid.
    let cache_hit = state
        .diff_cache
        .as_ref()
        .map_or(false, |c| c.key == new_key);

    if !cache_hit {
        // Cache miss — rebuild everything.
        let (_title, content, hunk_starts, hunk_ends, hunk_first_changes, gutter_w) =
            build_content(state, inner_w);

        // Pre-compute rendered text for clipboard.
        let rendered_text: Vec<String> = content
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect();

        state.diff_cache = Some(DiffCache {
            key: new_key,
            lines: content,
            hunk_starts,
            hunk_ends,
            hunk_first_changes,
            gutter_w,
            rendered_text,
        });
    }

    // From here we know the cache is populated.
    // Extract values we need, then drop the immutable borrow on state.
    let (hunk_starts, hunk_ends, hunk_first_changes, gutter_w, content_height) = {
        let cache = state.diff_cache.as_ref().unwrap();
        (
            cache.hunk_starts.clone(),
            cache.hunk_ends.clone(),
            cache.hunk_first_changes.clone(),
            cache.gutter_w,
            cache.lines.len(),
        )
    };

    state.hunk_start_rows = hunk_starts;
    state.hunk_end_rows = hunk_ends;
    state.hunk_first_change_rows = hunk_first_changes;
    // Two gutter columns (old + new) plus separator, plus optional blame.
    let blame_cols = if state.show_blame {
        BLAME_COL_WIDTH + 1
    } else {
        0
    };
    state.diff_gutter_cols = if gutter_w > 0 {
        blame_cols + gutter_w * 2 + 1
    } else {
        0
    };

    // Store plain text for clipboard extraction.
    // Only update on cache miss — the value persists across frames.
    if !cache_hit {
        let rendered_text = {
            let cache = state.diff_cache.as_ref().unwrap();
            cache.rendered_text.clone()
        };
        state.diff_rendered_text = rendered_text;
    }

    // Update content/viewport dimensions for scroll clamping.
    state.diff_content_height = content_height;
    state.diff_view_height = area.height.saturating_sub(2) as usize;
    state.clamp_cursor_and_scroll();

    // Build the title fresh each frame — it depends on diff_scroll for hunk
    // navigation info (e.g. "3/5") so it can't be cached.
    let total_hunks = state.hunk_start_rows.len();
    let title = match state.selected_file_entry() {
        Some(entry) => build_title(state, entry, total_hunks),
        None => " Diff ".to_string(),
    };

    // Recompute diff search matches when content has changed (cache miss).
    if !cache_hit && state.diff_search_query.is_some() {
        if let Some(_err) = state.recompute_diff_search_matches() {
            // Invalid regex — clear search silently on content change.
            state.diff_search_query = None;
        }
    }

    // Extract only the visible slice from the cache — clone ~viewport lines.
    // Apply cursor line highlight and search match highlighting.
    let cursor_line_bg = *state.styles.diff.cursor_line_bg;
    let search_match_bg = *state.styles.diff.search_match_bg;
    let search_current_bg = *state.styles.diff.search_current_match_bg;
    let cursor_visible_idx = state.diff_line_cursor.saturating_sub(state.diff_scroll);
    let visible: Vec<Line> = {
        let cache = state.diff_cache.as_ref().unwrap();
        cache
            .lines
            .iter()
            .skip(state.diff_scroll)
            .take(state.diff_view_height)
            .enumerate()
            .map(|(i, line)| {
                let display_row = state.diff_scroll + i;
                let is_cursor_line = i == cursor_visible_idx
                    && state.diff_line_cursor >= state.diff_scroll
                    && state.diff_line_cursor < state.diff_scroll + state.diff_view_height;

                // Collect search matches on this row.
                let row_matches: Vec<(usize, usize, bool)> = state
                    .diff_search_matches
                    .iter()
                    .enumerate()
                    .filter(|(_, (row, _, _))| *row == display_row)
                    .map(|(idx, (_, start, end))| (*start, *end, idx == state.diff_search_current))
                    .collect();

                let mut result_line = if row_matches.is_empty() && is_cursor_line {
                    // Simple case: cursor line, no search matches.
                    let spans: Vec<Span> = line
                        .spans
                        .iter()
                        .map(|span| {
                            Span::styled(
                                span.content.clone(),
                                span.style.bg(cursor_line_bg),
                            )
                        })
                        .collect();
                    Line::from(spans)
                } else if row_matches.is_empty() {
                    line.clone()
                } else {
                    // Apply search match highlights by splitting spans.
                    let highlighted = apply_search_highlights(
                        line,
                        &row_matches,
                        search_match_bg,
                        search_current_bg,
                        if is_cursor_line {
                            Some(cursor_line_bg)
                        } else {
                            None
                        },
                    );
                    Line::from(highlighted)
                };

                // Apply column cursor overlay on the cursor line.
                if is_cursor_line {
                    let content_start = state.diff_content_start_col();
                    result_line = apply_col_cursor(
                        &result_line,
                        content_start,
                        state.diff_col_cursor,
                    );
                }

                result_line
            })
            .collect()
    };

    let paragraph = Paragraph::new(visible).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title),
    );

    frame.render_widget(paragraph, area);
}

// ---------------------------------------------------------------------------
// Content dispatch
// ---------------------------------------------------------------------------

/// Return type for content builders: lines, hunk start rows, hunk end rows.
struct BuiltContent {
    lines: Vec<Line<'static>>,
    hunk_starts: Vec<usize>,
    hunk_ends: Vec<usize>,
    /// Row of the first actual change (+/-) in each hunk.
    hunk_first_changes: Vec<usize>,
    /// Width of one line-number gutter column (in characters).
    gutter_w: usize,
}

fn build_content(
    state: &AppState,
    inner_w: usize,
) -> (String, Vec<Line<'static>>, Vec<usize>, Vec<usize>, Vec<usize>, usize) {
    let ds = &state.styles.diff;

    let entry = match state.selected_file_entry() {
        None => {
            return (
                " Diff ".to_string(),
                vec![Line::from("No files changed.")],
                vec![],
                vec![],
                vec![],
                0,
            )
        }
        Some(e) => e,
    };

    // Reviewed file summary mode.
    if matches!(entry.status, ReviewStatus::Reviewed { .. }) && !state.reviewed_diff_expanded {
        let at = match &entry.status {
            ReviewStatus::Reviewed { at, .. } => at.as_str(),
            _ => "",
        };
        let title = format!(" {} ", entry.change.path);
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  {} \u{2014} reviewed at {at}", entry.change.path),
                Style::default()
                    .fg(*ds.reviewed_fg)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Press Enter to view diff.",
                Style::default().fg(*ds.placeholder_fg),
            )),
        ];
        return (title, lines, vec![], vec![], vec![], 0);
    }

    // Binary file.
    if entry.diff.is_binary {
        let title = format!(" {} ", entry.change.path);
        return (
            title,
            vec![Line::from(Span::styled(
                "  Binary file",
                Style::default().fg(*ds.placeholder_fg),
            ))],
            vec![],
            vec![],
            vec![],
            0,
        );
    }

    let empty_blame: Vec<crate::git::BlameLine> = Vec::new();
    let head_blame: &[crate::git::BlameLine] = if state.show_blame {
        &state.head_blame
    } else {
        &empty_blame
    };
    let base_blame: &[crate::git::BlameLine] = if state.show_blame {
        &state.base_blame
    } else {
        &empty_blame
    };

    let default_bg = *state.styles.bg;

    let built = match (state.content_mode, state.render_variant) {
        (ContentMode::Diff, RenderVariant::SideBySide) => build_side_by_side_diff(
            ds,
            default_bg,
            &entry.diff.hunks,
            state.head_content.as_deref(),
            head_blame,
            base_blame,
            inner_w,
        ),
        (ContentMode::Diff, _) => build_inline_diff(
            ds,
            default_bg,
            &entry.diff.hunks,
            state.head_content.as_deref(),
            head_blame,
            base_blame,
            inner_w,
        ),
        (ContentMode::FullFile, RenderVariant::HeadVersion) => build_full_file_head(
            ds,
            default_bg,
            &entry.diff.hunks,
            state.head_content.as_deref(),
            head_blame,
            inner_w,
        ),
        (ContentMode::FullFile, RenderVariant::BaseVersion) => build_full_file_base(
            ds,
            default_bg,
            &entry.diff.hunks,
            state.base_content.as_deref(),
            base_blame,
            inner_w,
        ),
        _ => BuiltContent {
            lines: vec![Line::from("  (unknown variant)")],
            hunk_starts: vec![],
            hunk_ends: vec![],
            hunk_first_changes: vec![],
            gutter_w: 0,
        },
    };

    // Build title with hunk navigation info.
    let total_hunks = built.hunk_starts.len();
    let title = build_title(state, entry, total_hunks);

    (
        title,
        built.lines,
        built.hunk_starts,
        built.hunk_ends,
        built.hunk_first_changes,
        built.gutter_w,
    )
}

/// Build the border title with hunk navigation context.
fn build_title(state: &AppState, entry: &crate::model::FileEntry, total_hunks: usize) -> String {
    let path = &entry.change.path;

    let mode_label: String = match state.content_mode {
        ContentMode::FullFile => match state.render_variant {
            RenderVariant::HeadVersion => " (HEAD)".into(),
            RenderVariant::BaseVersion => " (base)".into(),
            _ => String::new(),
        },
        ContentMode::Diff => {
            let sbs = if state.render_variant == RenderVariant::SideBySide {
                " sbs"
            } else {
                ""
            };
            if state.diff_algorithm != state.default_diff_algorithm {
                let algo = state.diff_algorithm.label();
                format!(" (diff:{algo}{sbs})")
            } else if !sbs.is_empty() {
                format!(" (diff{sbs})")
            } else {
                " (diff)".into()
            }
        }
    };

    let ws_label = if state.ignore_whitespace { " -w" } else { "" };

    // Show diff base indicator for reviewed files when not using merge base.
    let base_label = if !state.show_merge_base {
        let has_reviewed_commit = matches!(
            &entry.status,
            ReviewStatus::Reviewed {
                reviewed_commit: Some(_),
                ..
            } | ReviewStatus::Changed {
                reviewed_commit: Some(_),
                ..
            }
        );
        if has_reviewed_commit {
            " [since review]"
        } else {
            ""
        }
    } else {
        ""
    };

    let hunk_info = if total_hunks == 0 {
        String::new()
    } else if let Some(idx) = state.current_hunk_index() {
        format!(" \u{2014} {}/{total_hunks}", idx + 1)
    } else {
        String::new()
    };

    format!(" {path}{mode_label}{ws_label}{base_label}{hunk_info} ")
}

// ---------------------------------------------------------------------------
// Inline diff: full file with additions + deletions interleaved
// ---------------------------------------------------------------------------

fn build_inline_diff(
    ds: &DiffStyle,
    default_bg: Color,
    hunks: &[DiffHunk],
    head_content: Option<&str>,
    head_blame: &[crate::git::BlameLine],
    base_blame: &[crate::git::BlameLine],
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
    let hblame = |lineno: Option<u32>| -> Option<&crate::git::BlameLine> {
        lineno.and_then(|n| head_blame.get((n as usize).wrapping_sub(1)))
    };
    let bblame = |lineno: Option<u32>| -> Option<&crate::git::BlameLine> {
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
        let leading_context = hunk.lines.iter().take_while(|l| l.kind == LineKind::Context).count();
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
                    super::word_diff::compute(dc, ac)
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
    }
}

// ---------------------------------------------------------------------------
// Side-by-side diff
// ---------------------------------------------------------------------------

fn build_side_by_side_diff(
    ds: &DiffStyle,
    default_bg: Color,
    hunks: &[DiffHunk],
    head_content: Option<&str>,
    head_blame: &[crate::git::BlameLine],
    base_blame: &[crate::git::BlameLine],
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
    let hblame = |lineno: Option<u32>| -> Option<&crate::git::BlameLine> {
        lineno.and_then(|n| head_blame.get((n as usize).wrapping_sub(1)))
    };
    let bblame = |lineno: Option<u32>| -> Option<&crate::git::BlameLine> {
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
                     emphasis: Option<(&[super::word_diff::DiffSpan<'_>], Style)>,
                     blame: Option<&crate::git::BlameLine>|
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
                    super::word_diff::DiffSpan::Common(t) => (*t, style),
                    super::word_diff::DiffSpan::Changed(t) => (*t, em_style),
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
        let leading_context = hunk.lines.iter().take_while(|l| l.kind == LineKind::Context).count();
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
                    super::word_diff::compute(dc, ac)
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

// ---------------------------------------------------------------------------
// Full-file HEAD: HEAD version with additions highlighted
// ---------------------------------------------------------------------------

fn build_full_file_head(
    ds: &DiffStyle,
    default_bg: Color,
    hunks: &[DiffHunk],
    head_content: Option<&str>,
    blame: &[crate::git::BlameLine],
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
            };
        }
    };

    let file_lines: Vec<&str> = content.lines().collect();
    if file_lines.is_empty() {
        return BuiltContent {
            lines: vec![Line::from(Span::styled(
                "  (empty file)",
                Style::default().fg(*ds.placeholder_fg),
            ))],
            hunk_starts: vec![],
            hunk_ends: vec![],
            hunk_first_changes: vec![],
            gutter_w: 0,
        };
    }

    // Collect all new-file line numbers that are additions.
    let addition_lines: HashSet<u32> = hunks
        .iter()
        .flat_map(|h| h.lines.iter())
        .filter(|l| l.kind == LineKind::Addition)
        .filter_map(|l| l.new_lineno)
        .collect();

    // Hunk boundaries in new-file line numbers.
    let hunk_new_ranges: Vec<(u32, u32)> = hunks
        .iter()
        .map(|h| (h.new_start, h.new_start + h.new_lines))
        .collect();

    let context_style = Style::default().fg(*ds.context_fg);
    let addition_style = Style::default().fg(*ds.addition_fg).bg(*ds.addition_bg);
    let gutter_fg = *ds.gutter_fg;
    let blame_fg = *ds.blame_fg;
    let gutter_w = digit_width(file_lines.len() as u32);
    let mut result = Vec::new();
    let mut hunk_starts = Vec::new();
    let mut hunk_ends = Vec::new();
    let mut hunk_first_changes = Vec::new();
    let mut current_hunk_first_change_recorded = false;

    for (i, text) in file_lines.iter().enumerate() {
        let lineno = (i + 1) as u32;

        // Track hunk boundaries.
        for &(start, end) in &hunk_new_ranges {
            if lineno == start {
                hunk_starts.push(result.len());
                current_hunk_first_change_recorded = false;
            }
            if lineno == end {
                hunk_ends.push(result.len());
            }
        }

        let is_addition = addition_lines.contains(&lineno);

        // Record first change row for the current hunk.
        if is_addition && !current_hunk_first_change_recorded {
            hunk_first_changes.push(result.len());
            current_hunk_first_change_recorded = true;
        }

        let style = if is_addition {
            addition_style
        } else {
            context_style
        };
        let prefix = if is_addition { "+" } else { " " };

        let bl = blame.get((lineno as usize).wrapping_sub(1));
        result.push(make_line(
            None,
            Some(lineno),
            prefix,
            text,
            style,
            gutter_fg,
            blame_fg,
            default_bg,
            gutter_w,
            inner_w,
            bl,
        ));
    }

    // Close any unclosed hunks (hunk extends to end of file).
    while hunk_ends.len() < hunk_starts.len() {
        hunk_ends.push(result.len());
    }
    // Ensure hunk_first_changes has an entry for every hunk.
    while hunk_first_changes.len() < hunk_starts.len() {
        hunk_first_changes.push(*hunk_starts.last().unwrap_or(&0));
    }

    BuiltContent {
        lines: result,
        hunk_starts,
        hunk_ends,
        hunk_first_changes,
        gutter_w,
    }
}

// ---------------------------------------------------------------------------
// Full-file base: base version with deletions highlighted
// ---------------------------------------------------------------------------

fn build_full_file_base(
    ds: &DiffStyle,
    default_bg: Color,
    hunks: &[DiffHunk],
    base_content: Option<&str>,
    blame: &[crate::git::BlameLine],
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
            };
        }
    };

    let file_lines: Vec<&str> = content.lines().collect();
    if file_lines.is_empty() {
        return BuiltContent {
            lines: vec![Line::from(Span::styled(
                "  (empty file)",
                Style::default().fg(*ds.placeholder_fg),
            ))],
            hunk_starts: vec![],
            hunk_ends: vec![],
            hunk_first_changes: vec![],
            gutter_w: 0,
        };
    }

    // Collect all old-file line numbers that are deletions.
    let deletion_lines: HashSet<u32> = hunks
        .iter()
        .flat_map(|h| h.lines.iter())
        .filter(|l| l.kind == LineKind::Deletion)
        .filter_map(|l| l.old_lineno)
        .collect();

    // Hunk boundaries in old-file line numbers.
    let hunk_old_ranges: Vec<(u32, u32)> = hunks
        .iter()
        .map(|h| (h.old_start, h.old_start + h.old_lines))
        .collect();

    let context_style = Style::default().fg(*ds.context_fg);
    let deletion_style = Style::default().fg(*ds.deletion_fg).bg(*ds.deletion_bg);
    let gutter_fg = *ds.gutter_fg;
    let blame_fg = *ds.blame_fg;
    let gutter_w = digit_width(file_lines.len() as u32);
    let mut result = Vec::new();
    let mut hunk_starts = Vec::new();
    let mut hunk_ends = Vec::new();
    let mut hunk_first_changes = Vec::new();
    let mut current_hunk_first_change_recorded = false;

    for (i, text) in file_lines.iter().enumerate() {
        let lineno = (i + 1) as u32;

        for &(start, end) in &hunk_old_ranges {
            if lineno == start {
                hunk_starts.push(result.len());
                current_hunk_first_change_recorded = false;
            }
            if lineno == end {
                hunk_ends.push(result.len());
            }
        }

        let is_deletion = deletion_lines.contains(&lineno);

        if is_deletion && !current_hunk_first_change_recorded {
            hunk_first_changes.push(result.len());
            current_hunk_first_change_recorded = true;
        }

        let style = if is_deletion {
            deletion_style
        } else {
            context_style
        };
        let prefix = if is_deletion { "-" } else { " " };
        let bl = blame.get((lineno as usize).wrapping_sub(1));

        result.push(make_line(
            Some(lineno),
            None,
            prefix,
            text,
            style,
            gutter_fg,
            blame_fg,
            default_bg,
            gutter_w,
            inner_w,
            bl,
        ));
    }

    while hunk_ends.len() < hunk_starts.len() {
        hunk_ends.push(result.len());
    }
    while hunk_first_changes.len() < hunk_starts.len() {
        hunk_first_changes.push(*hunk_starts.last().unwrap_or(&0));
    }

    BuiltContent {
        lines: result,
        hunk_starts,
        hunk_ends,
        hunk_first_changes,
        gutter_w,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Width of the blame annotation column (hash + date + author).
const BLAME_COL_WIDTH: usize = 30;

/// Format a blame annotation for display, padded/truncated to `BLAME_COL_WIDTH`.
fn format_blame(blame: Option<&crate::git::BlameLine>) -> String {
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
fn make_line(
    old_lineno: Option<u32>,
    new_lineno: Option<u32>,
    prefix: &str,
    content: &str,
    content_style: Style,
    gutter_fg: Color,
    blame_fg: Color,
    default_bg: Color,
    gutter_w: usize,
    inner_w: usize,
    blame: Option<&crate::git::BlameLine>,
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
    parts.push(Span::styled(format!(" {prefix} "), content_style));

    // Width consumed by blame + gutters + separator + prefix.
    let fixed_cols = blame_cols + gutter_w + 1 + gutter_w + 3;
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
fn make_line_with_emphasis(
    old_lineno: Option<u32>,
    new_lineno: Option<u32>,
    prefix: &str,
    spans: &[super::word_diff::DiffSpan<'_>],
    base_style: Style,
    emphasis_style: Style,
    gutter_fg: Color,
    blame_fg: Color,
    default_bg: Color,
    gutter_w: usize,
    inner_w: usize,
    blame: Option<&crate::git::BlameLine>,
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
    parts.push(Span::styled(format!(" {prefix} "), base_style));

    let fixed_cols = blame_cols + gutter_w + 1 + gutter_w + 3;
    let content_cols = inner_w.saturating_sub(fixed_cols);

    let mut char_count = 0;
    for span in spans {
        let (text, style) = match span {
            super::word_diff::DiffSpan::Common(t) => (*t, base_style),
            super::word_diff::DiffSpan::Changed(t) => (*t, emphasis_style),
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
fn digit_width(n: u32) -> usize {
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

// ---------------------------------------------------------------------------
// Column cursor overlay
// ---------------------------------------------------------------------------

/// Apply a block cursor (inverted colors) at a specific character position
/// within the content portion of a line.
///
/// `content_start_col` is the number of fixed columns (gutter + prefix) before
/// actual content begins. `col_cursor` is the character offset within content.
fn apply_col_cursor(
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
        let cursor_style = Style::default()
            .fg(Color::Black)
            .bg(Color::White);
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
fn apply_search_highlights(
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
            let match_text =
                &text[(overlap_start - span_start)..(overlap_end - span_start)];
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
