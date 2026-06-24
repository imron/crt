//! Diff/file rendering: inline diff within full file context, full-file
//! mode with change highlighting, and reviewed-file summary.

mod cache;
mod content;
mod full_file;
mod highlight;
mod inline;
mod line;
mod side_by_side;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use self::cache::build_key;
use self::content::{build_content, build_title};
use self::highlight::{apply_col_cursor, apply_search_highlights};
use self::line::BLAME_COL_WIDTH;
use super::super::state::TuiState;
use crate::app::model::{AppModel, TextRange};
use crate::config::StyleConfig;
use crate::review_types::PaneFocus;

pub use self::cache::DiffCache;

/// Draw the diff/file view pane.
pub fn draw(
    frame: &mut Frame,
    model: &AppModel,
    tui_state: &mut TuiState,
    styles: &StyleConfig,
    area: Rect,
) {
    let diff = &model.diff;
    let focused = model.focus == PaneFocus::Diff;
    let border_style = super::pane_border_style(&styles.panel, focused);

    // Inner width excludes left and right borders.
    let inner_w = area.width.saturating_sub(2) as usize;

    // Build the cache key from current state.
    let new_key = build_key(diff, inner_w);

    let cache_hit = match tui_state.diff_cache.as_ref() {
        Some(cache) if cache.key == new_key => true,
        _ => {
            // Cache miss — rebuild everything.
            let (_title, content, hunk_starts, hunk_ends, hunk_first_changes, gutter_w) =
                build_content(diff, styles, inner_w);

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

            tui_state.diff_cache = Some(DiffCache {
                key: new_key,
                lines: content,
                hunk_starts,
                hunk_ends,
                hunk_first_changes,
                gutter_w,
                rendered_text,
            });
            false
        }
    };

    // From here we know the cache is populated.
    // Extract values we need without keeping state mutations tied to cache shape.
    let (hunk_starts, hunk_ends, hunk_first_changes, gutter_w, content_height, rendered_text) = {
        let cache = tui_state
            .diff_cache
            .as_ref()
            .expect("diff cache should be populated before rendering");
        (
            cache.hunk_starts.clone(),
            cache.hunk_ends.clone(),
            cache.hunk_first_changes.clone(),
            cache.gutter_w,
            cache.lines.len(),
            cache.rendered_text.clone(),
        )
    };

    tui_state.hunk_start_rows = hunk_starts;
    tui_state.hunk_end_rows = hunk_ends;
    tui_state.hunk_first_change_rows = hunk_first_changes;
    // Two gutter columns (old + new) plus separator, plus optional blame.
    let blame_cols = if diff.show_blame {
        BLAME_COL_WIDTH + 1
    } else {
        0
    };
    tui_state.diff_gutter_cols = if gutter_w > 0 {
        blame_cols + gutter_w * 2 + 1
    } else {
        0
    };

    // Store plain text for clipboard extraction.
    // Only update on cache miss — the value persists across frames.
    if !cache_hit {
        tui_state.diff_rendered_text = rendered_text;
    }

    // Update content/viewport dimensions for scroll clamping.
    tui_state.diff_content_height = content_height;
    tui_state.diff_view_height = area.height.saturating_sub(2) as usize;
    // Build the title fresh each frame — it depends on diff_scroll for hunk
    // navigation info (e.g. "3/5") so it can't be cached.
    let total_hunks = tui_state.hunk_start_rows.len();
    let title = match &diff.path {
        Some(_) => build_title(diff, tui_state, total_hunks),
        None => " Diff ".to_string(),
    };

    // Extract only the visible slice from the cache — clone ~viewport lines.
    // Apply cursor line highlight and search match highlighting.
    let cursor_line_bg = *styles.diff.cursor_line_bg;
    let search_match_bg = *styles.diff.search_match_bg;
    let search_current_bg = *styles.diff.search_current_match_bg;
    let cursor_visible_idx = diff.cursor.line.saturating_sub(diff.scroll);
    let diff_view_height = tui_state.diff_view_height;
    let content_start_col = tui_state.diff_content_start_col();
    let search_highlights = render_search_highlights(
        diff.search_query.as_deref(),
        &tui_state.diff_rendered_text,
        tui_state.diff_gutter_cols,
    )
    .unwrap_or_else(|| diff.search_highlights.clone());
    let current_search_highlight = current_search_highlight(diff, &search_highlights);
    let visible_source: Vec<Line> = tui_state
        .diff_cache
        .as_ref()
        .expect("diff cache should be populated before rendering")
        .lines
        .iter()
        .skip(diff.scroll)
        .take(diff_view_height)
        .cloned()
        .collect();
    let visible: Vec<Line> = {
        visible_source
            .into_iter()
            .enumerate()
            .map(|(i, line)| {
                let display_row = diff.scroll + i;
                let is_cursor_line = i == cursor_visible_idx
                    && diff.cursor.line >= diff.scroll
                    && diff.cursor.line < diff.scroll + diff_view_height;

                // Collect search matches on this row.
                let row_matches: Vec<(usize, usize, bool)> = search_highlights
                    .iter()
                    .enumerate()
                    .filter(|(_, range)| range.line == display_row)
                    .map(|(idx, range)| {
                        (
                            range.column_start,
                            range.column_end,
                            Some(idx) == current_search_highlight,
                        )
                    })
                    .collect();

                let mut result_line = if row_matches.is_empty() && is_cursor_line {
                    // Simple case: cursor line, no search matches.
                    let spans: Vec<Span> = line
                        .spans
                        .iter()
                        .map(|span| {
                            Span::styled(span.content.clone(), span.style.bg(cursor_line_bg))
                        })
                        .collect();
                    Line::from(spans)
                } else if row_matches.is_empty() {
                    line.clone()
                } else {
                    // Apply search match highlights by splitting spans.
                    let highlighted = apply_search_highlights(
                        &line,
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
                    result_line =
                        apply_col_cursor(&result_line, content_start_col, diff.cursor.column);
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

fn render_search_highlights(
    query: Option<&str>,
    rendered_text: &[String],
    gutter_cols: usize,
) -> Option<Vec<TextRange>> {
    let query = query.filter(|query| !query.is_empty())?;
    let re = regex::RegexBuilder::new(query)
        .case_insensitive(true)
        .build()
        .ok()?;
    let mut matches = Vec::new();
    for (row, line) in rendered_text.iter().enumerate() {
        let search_start = gutter_cols.min(line.len());
        let content = &line[search_start..];
        for m in re.find_iter(content) {
            if m.start() == m.end() {
                continue;
            }
            matches.push(TextRange {
                line: row,
                column_start: search_start + m.start(),
                column_end: search_start + m.end(),
            });
        }
    }
    Some(matches)
}

fn current_search_highlight(
    diff: &crate::app::model::DiffPanel,
    search_highlights: &[TextRange],
) -> Option<usize> {
    if search_highlights.is_empty() {
        return None;
    }

    if diff.search_highlights == search_highlights {
        if let Some(current) = diff.current_search_highlight {
            if current < search_highlights.len() {
                return Some(current);
            }
        }
    }

    search_highlights
        .iter()
        .position(|range| range.line >= diff.cursor.line)
        .or(Some(0))
}
