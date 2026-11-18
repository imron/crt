//! File list widget: split unreviewed/reviewed sections with headers,
//! status markers, scrolling, and cursor tracking.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::super::state::TuiState;
use crate::app::model::{AppModel, FileListRowModel, FileListSectionKind, ReviewStatusModel};
use crate::config::{FilesStyle, StyleConfig};
use crate::model::{ChangeKind, PaneFocus};

/// Draw the file list pane with split unreviewed/reviewed sections.
pub fn draw(
    frame: &mut Frame,
    model: &AppModel,
    tui_state: &mut TuiState,
    styles: &StyleConfig,
    area: Rect,
) {
    let focused = model.focus == PaneFocus::FileList;
    let border_style = super::pane_border_style(&styles.panel, focused);
    let fs = &styles.files;

    // Build the display list: headers + file entries.
    // Track which display row each file index maps to.
    let mut lines: Vec<Line> = Vec::new();
    let mut rendered_text: Vec<String> = Vec::new();
    let mut row_to_file: Vec<Option<usize>> = Vec::new();

    let inner_width = area.width.saturating_sub(2) as usize; // minus borders

    for section in &model.file_list.sections {
        let label = match section.kind {
            FileListSectionKind::Unreviewed => "Unreviewed",
            FileListSectionKind::Reviewed => "Reviewed",
        };
        let count = section.rows.len();
        lines.push(section_header(fs, label, count, inner_width));
        rendered_text.push(format!("── {label} ({count}) ──"));
        row_to_file.push(None);

        for row in &section.rows {
            let (line, text) = file_line(fs, row, focused, inner_width);
            lines.push(line);
            rendered_text.push(text);
            row_to_file.push(Some(row.file_index));
        }
    }

    tui_state.file_list_rendered_text = rendered_text;
    tui_state.file_list_row_to_file = row_to_file;

    // Apply scroll offset.
    let visible: Vec<Line> = lines.into_iter().skip(model.file_list.scroll).collect();

    let title = format!(" Files ({}) ", model.file_list_row_count());
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
    row: &FileListRowModel,
    pane_focused: bool,
    max_width: usize,
) -> (Line<'static>, String) {
    let marker = match &row.review_status {
        ReviewStatusModel::Unreviewed => "\u{2717}",      // ✗
        ReviewStatusModel::Reviewed { .. } => "\u{2713}", // ✓
        ReviewStatusModel::Changed { .. } => "~",
    };

    let marker_color = match &row.review_status {
        ReviewStatusModel::Unreviewed => *fs.unreviewed_fg,
        ReviewStatusModel::Reviewed { .. } => *fs.reviewed_fg,
        ReviewStatusModel::Changed { .. } => *fs.changed_fg,
    };

    let kind_indicator = match row.change_kind {
        ChangeKind::Added => "+",
        ChangeKind::Deleted => "-",
        ChangeKind::Modified => " ",
        ChangeKind::Renamed => "R",
    };

    let path_raw = match &row.old_path {
        Some(old) => format!("{} \u{2190} {old}", row.path),
        None => row.path.clone(),
    };

    // Prefix columns: " X " (marker, 4) + "K " (kind, 2) = 6.
    let prefix_cols = 6;
    let path_budget = max_width.saturating_sub(prefix_cols);
    let path_text = truncate_path(&path_raw, path_budget);

    let plain = format!(" {marker} {kind_indicator} {path_text}");

    let path_style = if row.selected {
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
