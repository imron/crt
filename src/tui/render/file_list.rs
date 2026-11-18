//! File list widget: split unreviewed/reviewed sections with headers,
//! status markers, scrolling, and cursor tracking.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::AppState;
use crate::config::{FilesStyle, StyleConfig};
use crate::model::{ChangeKind, PaneFocus, ReviewStatus};

/// Draw the file list pane with split unreviewed/reviewed sections.
pub fn draw(frame: &mut Frame, state: &mut AppState, styles: &StyleConfig, area: Rect) {
    let focused = state.pane_focus == PaneFocus::FileList;
    let border_style = super::pane_border_style(&styles.panel, focused);
    let fs = &styles.files;

    let unreviewed_count = state.unreviewed_count();
    let reviewed_count = state.files.len() - unreviewed_count;

    // Build the display list: headers + file entries.
    // Track which display row each file index maps to.
    let mut lines: Vec<Line> = Vec::new();
    let mut rendered_text: Vec<String> = Vec::new();
    let mut row_to_file: Vec<Option<usize>> = Vec::new();
    let mut file_display_rows: Vec<usize> = vec![0; state.files.len()];

    let inner_width = area.width.saturating_sub(2) as usize; // minus borders

    // -- Unreviewed section --
    lines.push(section_header(
        fs,
        "Unreviewed",
        unreviewed_count,
        inner_width,
    ));
    rendered_text.push(format!("── Unreviewed ({unreviewed_count}) ──"));
    row_to_file.push(None); // header row

    for i in 0..unreviewed_count {
        file_display_rows[i] = lines.len();
        let (line, text) = file_line(
            fs,
            &state.files[i],
            i == state.selected_file,
            focused,
            inner_width,
        );
        lines.push(line);
        rendered_text.push(text);
        row_to_file.push(Some(i));
    }

    // -- Reviewed section --
    lines.push(section_header(fs, "Reviewed", reviewed_count, inner_width));
    rendered_text.push(format!("── Reviewed ({reviewed_count}) ──"));
    row_to_file.push(None); // header row

    for i in unreviewed_count..state.files.len() {
        file_display_rows[i] = lines.len();
        let (line, text) = file_line(
            fs,
            &state.files[i],
            i == state.selected_file,
            focused,
            inner_width,
        );
        lines.push(line);
        rendered_text.push(text);
        row_to_file.push(Some(i));
    }

    state.file_list_rendered_text = rendered_text;
    state.file_list_row_to_file = row_to_file;

    // -- Scroll to keep the cursor visible --
    let inner_height = area.height.saturating_sub(2) as usize;
    if !state.files.is_empty() {
        let cursor_row = file_display_rows[state.selected_file];
        if cursor_row < state.file_list_scroll {
            state.file_list_scroll = cursor_row;
        } else if cursor_row >= state.file_list_scroll + inner_height {
            state.file_list_scroll = cursor_row.saturating_sub(inner_height) + 1;
        }
    }

    // Apply scroll offset.
    let visible: Vec<Line> = lines.into_iter().skip(state.file_list_scroll).collect();

    let title = format!(" Files ({}) ", state.files.len());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let paragraph = Paragraph::new(visible).block(block);
    frame.render_widget(paragraph, area);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a section header line like "── Unreviewed (3) ──────────".
fn section_header(fs: &FilesStyle, label: &str, count: usize, width: usize) -> Line<'static> {
    let text = format!("\u{2500}\u{2500} {label} ({count}) ");
    let fill_count = width.saturating_sub(text.chars().count());
    let fill: String = "\u{2500}".repeat(fill_count);
    let full = format!("{text}{fill}");
    Line::from(Span::styled(
        full,
        Style::default()
            .fg(*fs.separator_fg)
            .add_modifier(Modifier::BOLD),
    ))
}

/// Build a file entry line with status marker and path.
/// Returns both the styled Line and the plain text (for clipboard).
fn file_line(
    fs: &FilesStyle,
    entry: &crate::model::FileEntry,
    selected: bool,
    pane_focused: bool,
    max_width: usize,
) -> (Line<'static>, String) {
    let marker = match &entry.status {
        ReviewStatus::Unreviewed => "\u{2717}",      // ✗
        ReviewStatus::Reviewed { .. } => "\u{2713}", // ✓
        ReviewStatus::Changed { .. } => "~",
    };

    let marker_color = match &entry.status {
        ReviewStatus::Unreviewed => *fs.unreviewed_fg,
        ReviewStatus::Reviewed { .. } => *fs.reviewed_fg,
        ReviewStatus::Changed { .. } => *fs.changed_fg,
    };

    let kind_indicator = match entry.change.kind {
        ChangeKind::Added => "+",
        ChangeKind::Deleted => "-",
        ChangeKind::Modified => " ",
        ChangeKind::Renamed => "R",
    };

    let path_raw = match &entry.change.old_path {
        Some(old) => format!("{} \u{2190} {old}", entry.change.path),
        None => entry.change.path.clone(),
    };

    // Prefix columns: " X " (marker, 4) + "K " (kind, 2) = 6.
    let prefix_cols = 6;
    let path_budget = max_width.saturating_sub(prefix_cols);
    let path_text = truncate_path(&path_raw, path_budget);

    let plain = format!(" {marker} {kind_indicator} {path_text}");

    let path_style = if selected {
        let style = Style::default().fg(*fs.selected_fg);
        if pane_focused {
            style.add_modifier(Modifier::BOLD)
        } else {
            style
        }
    } else {
        Style::default().fg(*fs.text_fg)
    };

    let line = Line::from(vec![
        Span::styled(format!(" {marker} "), Style::default().fg(marker_color)),
        Span::styled(
            format!("{kind_indicator} "),
            Style::default().fg(*fs.kind_fg),
        ),
        Span::styled(path_text, path_style),
    ]);

    (line, plain)
}

/// Truncate a file path to fit within `budget` characters.
///
/// Strategy (in order of preference):
/// 1. Full path fits → return as-is.
/// 2. Drop leading directories one at a time, replacing with `…`, keeping
///    as much trailing path as possible. E.g. `src/a/b/file.rs` → `…a/b/file.rs`
///    → `…b/file.rs` → `…file.rs`.
/// 3. If even the filename alone exceeds the budget, truncate the filename
///    from the end with `…`.
///
/// The `…` prefix is placed directly against the next path component (no
/// slash) so it doesn't look like `../`.
fn truncate_path(path: &str, budget: usize) -> String {
    let len = path.chars().count();
    if len <= budget || budget < 2 {
        return path.to_string();
    }

    // Reserve 1 char for the `…` prefix.
    let tail_budget = budget - 1;

    // Collect byte offsets of each `/` so we can try progressively
    // shorter suffixes: after the first `/`, after the second, etc.
    let slash_positions: Vec<usize> = path
        .bytes()
        .enumerate()
        .filter(|&(_, b)| b == b'/')
        .map(|(i, _)| i)
        .collect();

    // Try dropping leading directories one at a time.
    for &pos in &slash_positions {
        let tail = &path[pos + 1..];
        if tail.chars().count() <= tail_budget {
            return format!("\u{2026}{tail}");
        }
    }

    // Even the filename alone (after last `/`) is too long.
    // Show `…` + as much of the filename end as fits, so the extension
    // is visible.
    let filename = slash_positions
        .last()
        .map(|&pos| &path[pos + 1..])
        .unwrap_or(path);

    let fname_len = filename.chars().count();
    if fname_len <= tail_budget {
        return format!("\u{2026}{filename}");
    }

    // Truncate the filename from the start, keeping the tail (extension).
    let skip = fname_len - tail_budget;
    let truncated: String = filename.chars().skip(skip).collect();
    format!("\u{2026}{truncated}")
}
