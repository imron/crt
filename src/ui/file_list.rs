//! File list widget: split unreviewed/reviewed sections with headers,
//! status markers, scrolling, and cursor tracking.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::AppState;
use crate::model::{ChangeKind, PaneFocus, ReviewStatus};

/// Draw the file list pane with split unreviewed/reviewed sections.
pub fn draw(frame: &mut Frame, state: &mut AppState, area: Rect) {
    let focused = state.pane_focus == PaneFocus::FileList;
    let border_style = super::pane_border_style(focused);

    let unreviewed_count = state.unreviewed_count();
    let reviewed_count = state.files.len() - unreviewed_count;

    // Build the display list: headers + file entries.
    // Track which display row each file index maps to.
    let mut lines: Vec<Line> = Vec::new();
    let mut rendered_text: Vec<String> = Vec::new();
    let mut file_display_rows: Vec<usize> = vec![0; state.files.len()];

    let inner_width = area.width.saturating_sub(2) as usize; // minus borders

    // -- Unreviewed section --
    lines.push(section_header("Unreviewed", unreviewed_count, inner_width));
    rendered_text.push(format!("── Unreviewed ({unreviewed_count}) ──"));

    for i in 0..unreviewed_count {
        file_display_rows[i] = lines.len();
        let (line, text) = file_line(&state.files[i], i == state.selected_file, focused);
        lines.push(line);
        rendered_text.push(text);
    }

    // -- Reviewed section --
    lines.push(section_header("Reviewed", reviewed_count, inner_width));
    rendered_text.push(format!("── Reviewed ({reviewed_count}) ──"));

    for i in unreviewed_count..state.files.len() {
        file_display_rows[i] = lines.len();
        let (line, text) = file_line(&state.files[i], i == state.selected_file, focused);
        lines.push(line);
        rendered_text.push(text);
    }

    state.file_list_rendered_text = rendered_text;

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
fn section_header(label: &str, count: usize, width: usize) -> Line<'static> {
    let text = format!("\u{2500}\u{2500} {label} ({count}) ");
    let fill_count = width.saturating_sub(text.chars().count());
    let fill: String = "\u{2500}".repeat(fill_count);
    let full = format!("{text}{fill}");
    Line::from(Span::styled(
        full,
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    ))
}

/// Build a file entry line with status marker and path.
/// Returns both the styled Line and the plain text (for clipboard).
fn file_line(
    entry: &crate::model::FileEntry,
    selected: bool,
    pane_focused: bool,
) -> (Line<'static>, String) {
    let marker = match &entry.status {
        ReviewStatus::Unreviewed => "\u{2717}",      // ✗
        ReviewStatus::Reviewed { .. } => "\u{2713}", // ✓
        ReviewStatus::Changed { .. } => "~",
    };

    let marker_color = match &entry.status {
        ReviewStatus::Unreviewed => Color::Red,
        ReviewStatus::Reviewed { .. } => Color::Green,
        ReviewStatus::Changed { .. } => Color::Yellow,
    };

    let kind_indicator = match entry.change.kind {
        ChangeKind::Added => "+",
        ChangeKind::Deleted => "-",
        ChangeKind::Modified => " ",
        ChangeKind::Renamed => "R",
    };

    let path_text = match &entry.change.old_path {
        Some(old) => format!("{} \u{2190} {old}", entry.change.path),
        None => entry.change.path.clone(),
    };

    let plain = format!(" {marker} {kind_indicator} {path_text}");

    let path_style = if selected {
        if pane_focused {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        }
    } else {
        Style::default().fg(Color::Gray)
    };

    let line = Line::from(vec![
        Span::styled(format!(" {marker} "), Style::default().fg(marker_color)),
        Span::styled(
            format!("{kind_indicator} "),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(path_text, path_style),
    ]);

    (line, plain)
}
