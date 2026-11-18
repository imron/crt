//! File list widget: split unreviewed/reviewed sections with headers,
//! status markers, scrolling, and cursor tracking.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::super::state::TuiState;
use crate::app::model::{
    AppModel, FileListRow, FileListRowKind, FileListSectionKind, ReviewStatus,
};
use crate::config::{FilesStyle, StyleConfig};
use crate::review_types::{ChangeKind, PaneFocus};

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
    let mut row_to_comment: Vec<Option<i64>> = Vec::new();

    let inner_width = area.width.saturating_sub(2) as usize; // minus borders

    for (section_index, section) in model.file_list.sections.iter().enumerate() {
        if section_index > 0 {
            lines.push(Line::from(""));
            rendered_text.push(String::new());
            row_to_file.push(None);
            row_to_comment.push(None);
        }

        let label = match section.kind {
            FileListSectionKind::Unreviewed => "Unreviewed",
            FileListSectionKind::Reviewed => "Reviewed",
            FileListSectionKind::UnresolvedComments => "Unresolved Comments",
        };
        let count = match section.kind {
            FileListSectionKind::UnresolvedComments => section
                .rows
                .iter()
                .filter(|row| row.kind == FileListRowKind::Comment)
                .count(),
            _ => section.rows.len(),
        };
        lines.push(section_header(fs, label, count, inner_width));
        rendered_text.push(format!("── {label} ({count}) ──"));
        row_to_file.push(None);
        row_to_comment.push(None);

        for row in &section.rows {
            let (line, text) = match row.kind {
                FileListRowKind::File => file_line(fs, row, focused, inner_width),
                FileListRowKind::Comment => comment_line(fs, row, focused, inner_width),
            };
            lines.push(line);
            rendered_text.push(text);
            row_to_file.push(row.file_index);
            row_to_comment.push(row.comment_id);
        }
    }

    tui_state.file_list_rendered_text = rendered_text;
    tui_state.file_list_row_to_file = row_to_file;
    tui_state.file_list_row_to_comment = row_to_comment;

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
    row: &FileListRow,
    pane_focused: bool,
    max_width: usize,
) -> (Line<'static>, String) {
    // Comment indicator: shown to the left of the review marker.
    let comment_indicator = if row.unresolved_comment_count > 0 {
        "\u{25CF}" // ●
    } else {
        " "
    };

    let marker = match &row.review_status {
        ReviewStatus::Unreviewed => "\u{2717}",      // ✗
        ReviewStatus::Reviewed { .. } => "\u{2713}", // ✓
        ReviewStatus::Changed { .. } => "~",
    };

    let marker_color = match &row.review_status {
        ReviewStatus::Unreviewed => *fs.unreviewed_fg,
        ReviewStatus::Reviewed { .. } => *fs.reviewed_fg,
        ReviewStatus::Changed { .. } => *fs.changed_fg,
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

    // Prefix columns: "C" (comment, 1) + " X " (marker, 3) + "K " (kind, 2) = 6.
    let prefix_cols = 6;
    let path_budget = max_width.saturating_sub(prefix_cols);
    let path_text = truncate_path(&path_raw, path_budget);

    let plain = format!("{comment_indicator} {marker} {kind_indicator} {path_text}");

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
        Span::styled(
            comment_indicator.to_string(),
            Style::default().fg(*fs.comment_fg),
        ),
        Span::styled(format!(" {marker} "), Style::default().fg(marker_color)),
        Span::styled(
            format!("{kind_indicator} "),
            Style::default().fg(*fs.kind_fg),
        ),
        Span::styled(path_text, path_style),
    ]);

    (line, plain)
}

fn comment_line(
    fs: &FilesStyle,
    row: &FileListRow,
    pane_focused: bool,
    max_width: usize,
) -> (Line<'static>, String) {
    let id = row
        .comment_id
        .map(|id| format!("#{id}"))
        .unwrap_or_else(|| "#?".to_string());
    let line = row
        .comment_line_start
        .map(|line| format!("L{line} "))
        .unwrap_or_default();
    let preview = row.comment_preview.as_deref().unwrap_or("");
    let prefix = format!("  {id} {line}");
    let preview_budget = max_width.saturating_sub(prefix.chars().count());
    let preview_text = truncate_text(preview, preview_budget);
    let plain = format!("{prefix}{preview_text}");

    let style = if row.selected {
        let style = Style::default().fg(*fs.selected_fg);
        if pane_focused {
            style.add_modifier(Modifier::BOLD)
        } else {
            style
        }
    } else {
        Style::default().fg(*fs.text_fg)
    };

    (
        Line::from(vec![
            Span::styled("  ".to_string(), Style::default()),
            Span::styled(id, Style::default().fg(*fs.comment_fg)),
            Span::styled(" ".to_string(), Style::default()),
            Span::styled(line, Style::default().fg(*fs.separator_fg)),
            Span::styled(preview_text, style),
        ]),
        plain,
    )
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

fn truncate_text(text: &str, budget: usize) -> String {
    if text.chars().count() <= budget || budget < 2 {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(budget.saturating_sub(1)).collect();
    truncated.push('\u{2026}');
    truncated
}
