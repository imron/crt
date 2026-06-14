//! Top-level render function and layout management.
//!
//! The TUI has a two-pane layout: file list on the left, diff/file view
//! on the right, with a status bar at the bottom. Either pane can be
//! hidden to give the other full width.

pub mod diff_view;
pub mod file_list;
pub mod word_diff;

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::AppState;
use crate::config::{PanelStyle, StyleConfig};
use crate::git;
use crate::model::ReviewStatus;
use crate::tui::{InputMode, TuiState};

/// Draw the entire UI for the current state.
pub fn draw(frame: &mut Frame, state: &mut AppState, tui_state: &mut TuiState) {
    // Fill the entire frame with the application background color.
    let bg_style = Style::default().bg(*state.styles.bg);
    frame.render_widget(Block::default().style(bg_style), frame.area());

    // Split into main area + status bar.
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    let main_area = vertical[0];
    let status_area = vertical[1];

    // Compute pane layout based on visibility.
    match (state.show_file_list, state.show_diff_pane) {
        (true, true) => {
            let panes = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Max(state.file_list_width), Constraint::Min(1)])
                .split(main_area);
            state.file_list_area = panes[0];
            state.diff_area = panes[1];
            file_list::draw(frame, state, panes[0]);
            diff_view::draw(frame, state, tui_state, panes[1]);
        }
        (true, false) => {
            state.file_list_area = main_area;
            state.diff_area = Rect::default();
            file_list::draw(frame, state, main_area);
        }
        (false, true) => {
            state.file_list_area = Rect::default();
            state.diff_area = main_area;
            diff_view::draw(frame, state, tui_state, main_area);
        }
        (false, false) => {
            // Should never happen — toggle logic prevents it.
            unreachable!("at least one pane must be visible");
        }
    }

    // Draw status bar or input prompt (command/search replace the status bar).
    match tui_state.input_mode {
        InputMode::Command => {
            draw_command_input(frame, state, tui_state, status_area);
        }
        InputMode::DiffSearch => {
            draw_diff_search_input(frame, state, tui_state, status_area);
        }
        InputMode::Normal => {
            draw_status_bar(frame, state, status_area);
        }
    }

    // Render mouse selection highlight on top of everything.
    if tui_state.mouse_selection.is_some() {
        draw_selection_highlight(frame, state, tui_state);
    }

    // Search results overlay.
    if state.search_results.is_some() {
        draw_search_results_overlay(frame, state);
    }

    // Definition results overlay.
    if state.definition_results.is_some() {
        draw_definition_results_overlay(frame, state);
    }

    // Help overlay on top of everything else.
    if state.show_help {
        draw_help_overlay(frame, &state.styles);
    }
}

// ---------------------------------------------------------------------------
// Selection highlight
// ---------------------------------------------------------------------------

fn draw_selection_highlight(frame: &mut Frame, state: &AppState, tui_state: &TuiState) {
    let Some(sel) = &tui_state.mouse_selection else {
        return;
    };

    let area = match sel.pane {
        crate::model::PaneFocus::FileList => state.file_list_area,
        crate::model::PaneFocus::Diff => state.diff_area,
    };
    // Inner area excludes borders.
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    let (scroll, base_col) = match sel.pane {
        crate::model::PaneFocus::FileList => (state.file_list_scroll, 0usize),
        crate::model::PaneFocus::Diff => (state.diff_scroll, state.diff_content_start_col()),
    };

    let visible_start_line = scroll;
    let visible_end_line = visible_start_line.saturating_add(inner.height as usize);
    let (start, end) = sel.normalized();
    if end.line < visible_start_line || start.line >= visible_end_line {
        return;
    }

    let highlight = Style::default()
        .bg(*state.styles.selection.bg)
        .fg(*state.styles.selection.fg);

    let buf = frame.buffer_mut();
    for line_idx in start.line..=end.line {
        if line_idx < visible_start_line || line_idx >= visible_end_line {
            continue;
        }
        let row = inner.y + line_idx.saturating_sub(scroll) as u16;

        let col_start = if line_idx == start.line {
            inner
                .x
                .saturating_add(saturating_u16(base_col.saturating_add(start.column)))
        } else {
            inner.x.saturating_add(saturating_u16(base_col))
        };
        let col_end = if line_idx == end.line {
            inner
                .x
                .saturating_add(saturating_u16(base_col.saturating_add(end.column)))
        } else {
            inner.right().saturating_sub(1)
        }
        .min(inner.right().saturating_sub(1));

        if col_start > col_end {
            continue;
        }

        for col in col_start..=col_end {
            if let Some(cell) = buf.cell_mut(Position { x: col, y: row }) {
                cell.set_style(highlight);
            }
        }
    }
}

fn saturating_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

/// Status message timeout.
const STATUS_MSG_TIMEOUT: Duration = Duration::from_secs(3);

fn draw_status_bar(frame: &mut Frame, state: &mut AppState, area: Rect) {
    let ss = &state.styles.status;

    // Expire old status messages.
    if let Some((_, when)) = &state.status_message {
        if when.elapsed() > STATUS_MSG_TIMEOUT {
            state.status_message = None;
        }
    }

    let reviewed_count = state
        .files
        .iter()
        .filter(|f| matches!(f.status, ReviewStatus::Reviewed { .. }))
        .count();
    let total = state.files.len();

    let left = format!(
        " {} \u{2192} {} \u{2192} {} | {reviewed_count}/{total} reviewed",
        state.context.base_ref,
        git::short_hash(&state.context.merge_base),
        state.context.head_ref,
    );

    // Ctrl-C warning takes over the full bar.
    if let Some((msg, _)) = &state.status_message {
        if msg.contains("Ctrl-C") {
            let bar = format!(" {msg} ");
            let status =
                Paragraph::new(bar).style(Style::default().bg(*ss.warning_bg).fg(*ss.warning_fg));
            frame.render_widget(status, area);
            return;
        }
    }

    // Right side: transient message or default hint.
    let (right, right_style) = if let Some((msg, _)) = &state.status_message {
        (
            format!(" {msg} "),
            Style::default().bg(*ss.message_bg).fg(*ss.message_fg),
        )
    } else {
        (
            " ? help ".to_string(),
            Style::default().bg(*ss.hint_bg).fg(*ss.hint_fg),
        )
    };

    let bar_style = Style::default().bg(*ss.bar_bg).fg(*ss.bar_fg);

    // Split the status bar into left and right sections.
    // Cap the right section so it never consumes more than half the bar,
    // preventing long status messages from squeezing the left text into
    // illegibility.
    let max_right = (area.width / 2) as usize;
    let right_display_len = right.chars().count();
    let (right, right_width) = if right_display_len > max_right && max_right > 3 {
        // Truncate the right text to fit, adding "…" indicator.
        let truncated: String = right.chars().take(max_right - 1).collect();
        let truncated = format!("{truncated}\u{2026}");
        let w = truncated.chars().count() as u16;
        (truncated, w)
    } else {
        (right, right_display_len as u16)
    };
    let left_width = area.width.saturating_sub(right_width);

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(left_width),
            Constraint::Length(right_width),
        ])
        .split(area);

    let left_bar = Paragraph::new(left).style(bar_style);
    frame.render_widget(left_bar, chunks[0]);

    let right_bar = Paragraph::new(right).style(right_style);
    frame.render_widget(right_bar, chunks[1]);
}

// ---------------------------------------------------------------------------
// Help overlay
// ---------------------------------------------------------------------------

fn draw_help_overlay(frame: &mut Frame, styles: &StyleConfig) {
    let hs = &styles.help;
    let area = frame.area();

    // Center the help box, capped at reasonable dimensions.
    let help_width = 56u16.min(area.width.saturating_sub(4));
    let help_height = 40u16.min(area.height.saturating_sub(2));
    let x = (area.width.saturating_sub(help_width)) / 2;
    let y = (area.height.saturating_sub(help_height)) / 2;
    let help_area = Rect::new(x, y, help_width, help_height);

    // Dim the background.
    let buf = frame.buffer_mut();
    for row in area.y..area.bottom() {
        for col in area.x..area.right() {
            if let Some(cell) = buf.cell_mut(Position { x: col, y: row }) {
                cell.set_style(Style::default().fg(*hs.dim_fg));
            }
        }
    }

    let help_text = vec![
        Line::from(Span::styled(
            " Keyboard Shortcuts ",
            Style::default()
                .fg(*hs.border_fg)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            " Global",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  q             Quit"),
        Line::from("  ?             Toggle this help"),
        Line::from("  Tab           Switch pane focus"),
        Line::from("  Ctrl-n / p    Next / previous file"),
        Line::from("  Ctrl-w h/l    Focus left / right pane"),
        Line::from("  j / k         Scroll diff down / up"),
        Line::from("  Space / Ctrl-b Page down / up"),
        Line::from("  Ctrl-d / u    Half page down / up"),
        Line::from("  g / G         Top / bottom of diff"),
        Line::from("  ] / [         Next / previous diff hunk"),
        Line::from("  r             Toggle reviewed / unreviewed"),
        Line::from("  i             Toggle inline / side-by-side"),
        Line::from("  s             Cycle: diff / HEAD / base"),
        Line::from("  d             Cycle diff algorithm"),
        Line::from("  m             Toggle merge base / since review"),
        Line::from("  w             Toggle ignore whitespace"),
        Line::from("  b             Toggle blame annotations"),
        Line::from("  1 / 2         Toggle file list / diff pane"),
        Line::from("  Ctrl-c        Press twice to quit"),
        Line::from("  Enter         Expand reviewed / focus diff"),
        Line::from(""),
        Line::from(Span::styled(
            " Navigation & Search",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  :             Enter command mode"),
        Line::from("  :gr <regex>   Search all files"),
        Line::from("  :grd <regex>  Search diff files only"),
        Line::from("  :gd [symbol]  Go to definition"),
        Line::from("  :q            Quit"),
        Line::from("  Ctrl-]        Go to definition (word)"),
        Line::from("  Ctrl-t        Jump back (pop stack)"),
        Line::from(""),
        Line::from(Span::styled(
            " Mouse",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  Click         Select file / set focus"),
        Line::from("  Double-click  Select word (auto-copy)"),
        Line::from("  Drag          Select text (auto-copy)"),
        Line::from("  Drag border   Resize file list pane"),
        Line::from("  Scroll        Scroll diff pane"),
    ];

    // Clear the area first so no old content bleeds through.
    frame.render_widget(Clear, help_area);

    let help = Paragraph::new(help_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(*hs.border_fg))
                .title(" Help ")
                .title_bottom(" ? / q / Esc to close "),
        )
        .style(Style::default().fg(*hs.text_fg).bg(*hs.bg));

    frame.render_widget(help, help_area);
}

// ---------------------------------------------------------------------------
// Command input line
// ---------------------------------------------------------------------------

/// Draw the `:` command input line, replacing the status bar.
fn draw_command_input(frame: &mut Frame, state: &AppState, tui_state: &TuiState, area: Rect) {
    let ss = &state.styles.status;
    let input = format!(":{}", tui_state.command_input);
    let bar_style = Style::default().bg(*ss.bar_bg).fg(*ss.bar_fg);
    let input_line = Paragraph::new(input.clone()).style(bar_style);
    frame.render_widget(input_line, area);

    // Place cursor at the correct position within the command input.
    // The `:` prefix is 1 char, so cursor_x = area.x + 1 + command_cursor.
    let cursor_x = area.x + 1 + tui_state.command_cursor as u16;
    let cursor_y = area.y;
    frame.set_cursor_position(Position {
        x: cursor_x,
        y: cursor_y,
    });
}

/// Draw the `/` diff search input line, replacing the status bar.
fn draw_diff_search_input(frame: &mut Frame, state: &AppState, tui_state: &TuiState, area: Rect) {
    let ss = &state.styles.status;
    let input = format!("/{}", tui_state.diff_search_input);
    let bar_style = Style::default().bg(*ss.bar_bg).fg(*ss.bar_fg);
    let input_line = Paragraph::new(input.clone()).style(bar_style);
    frame.render_widget(input_line, area);

    // Place cursor at the correct position within the search input.
    let cursor_x = area.x + 1 + tui_state.diff_search_cursor as u16;
    let cursor_y = area.y;
    frame.set_cursor_position(Position {
        x: cursor_x,
        y: cursor_y,
    });
}

// ---------------------------------------------------------------------------
// Search results overlay
// ---------------------------------------------------------------------------

/// Draw the search results overlay as a centered popup.
fn draw_search_results_overlay(frame: &mut Frame, state: &AppState) {
    let results = match &state.search_results {
        Some(r) => r,
        None => return,
    };
    let hs = &state.styles.help;
    let area = frame.area();

    // Size the overlay.
    let overlay_width = (area.width * 4 / 5)
        .min(100)
        .max(40)
        .min(area.width.saturating_sub(4));
    let overlay_height = (area.height * 3 / 4)
        .max(10)
        .min(area.height.saturating_sub(2));
    let x = (area.width.saturating_sub(overlay_width)) / 2;
    let y = (area.height.saturating_sub(overlay_height)) / 2;
    let overlay_area = Rect::new(x, y, overlay_width, overlay_height);

    // Dim background.
    let buf = frame.buffer_mut();
    for row in area.y..area.bottom() {
        for col in area.x..area.right() {
            if let Some(cell) = buf.cell_mut(Position { x: col, y: row }) {
                cell.set_style(Style::default().fg(*hs.dim_fg));
            }
        }
    }

    // Build content lines.
    let inner_height = overlay_height.saturating_sub(2) as usize; // borders
    let total = results.matches.len();
    let scope_label = if results.diff_only { " (diff)" } else { "" };
    let title = format!(
        " Search: /{}/{} — {} matches ",
        results.query, scope_label, total
    );

    let mut lines: Vec<Line> = Vec::new();
    if total == 0 {
        lines.push(Line::from(Span::styled(
            "  No matches found",
            Style::default().fg(*hs.text_fg),
        )));
    } else {
        let visible_start = results.scroll;
        let visible_end = (visible_start + inner_height).min(total);
        for i in visible_start..visible_end {
            let m = &results.matches[i];
            let is_selected = i == results.selected;
            let style = if is_selected {
                Style::default()
                    .fg(ratatui::style::Color::Black)
                    .bg(ratatui::style::Color::Cyan)
            } else {
                Style::default().fg(*hs.text_fg)
            };
            let prefix = if is_selected { "> " } else { "  " };
            let display = format!(
                "{}{} :{} {}",
                prefix,
                m.file_path,
                m.line_number,
                truncate_line(&m.line_content, overlay_width as usize - 8),
            );
            lines.push(Line::from(Span::styled(display, style)));
        }
    }

    frame.render_widget(Clear, overlay_area);

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(*hs.border_fg))
                .title(title)
                .title_bottom(" j/k navigate  Enter open  q/Esc close "),
        )
        .style(Style::default().fg(*hs.text_fg).bg(*hs.bg));

    frame.render_widget(paragraph, overlay_area);
}

// ---------------------------------------------------------------------------
// Definition results overlay
// ---------------------------------------------------------------------------

/// Draw the definition results overlay as a centered popup.
fn draw_definition_results_overlay(frame: &mut Frame, state: &AppState) {
    let results = match &state.definition_results {
        Some(r) => r,
        None => return,
    };
    let hs = &state.styles.help;
    let area = frame.area();

    let total = results.definitions.len();
    let overlay_width = (area.width * 3 / 5)
        .min(80)
        .max(40)
        .min(area.width.saturating_sub(4));
    let overlay_height = (total as u16 + 4).max(6).min(area.height.saturating_sub(2));
    let x = (area.width.saturating_sub(overlay_width)) / 2;
    let y = (area.height.saturating_sub(overlay_height)) / 2;
    let overlay_area = Rect::new(x, y, overlay_width, overlay_height);

    // Dim background.
    let buf = frame.buffer_mut();
    for row in area.y..area.bottom() {
        for col in area.x..area.right() {
            if let Some(cell) = buf.cell_mut(Position { x: col, y: row }) {
                cell.set_style(Style::default().fg(*hs.dim_fg));
            }
        }
    }

    let title = format!(" Definition: {} — {} found ", results.symbol, total);

    let mut lines: Vec<Line> = Vec::new();
    if total == 0 {
        lines.push(Line::from(Span::styled(
            "  No definitions found",
            Style::default().fg(*hs.text_fg),
        )));
    } else {
        for (i, def) in results.definitions.iter().enumerate() {
            let is_selected = i == results.selected;
            let style = if is_selected {
                Style::default()
                    .fg(ratatui::style::Color::Black)
                    .bg(ratatui::style::Color::Cyan)
            } else {
                Style::default().fg(*hs.text_fg)
            };
            let prefix = if is_selected { "> " } else { "  " };
            let display = format!(
                "{}{} :{} {}",
                prefix,
                def.file_path,
                def.line_number,
                truncate_line(&def.line_content, overlay_width as usize - 8),
            );
            lines.push(Line::from(Span::styled(display, style)));
        }
    }

    frame.render_widget(Clear, overlay_area);

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(*hs.border_fg))
                .title(title)
                .title_bottom(" j/k navigate  Enter open  q/Esc close "),
        )
        .style(Style::default().fg(*hs.text_fg).bg(*hs.bg));

    frame.render_widget(paragraph, overlay_area);
}

/// Truncate a line to a maximum width, adding "..." if truncated.
fn truncate_line(s: &str, max: usize) -> String {
    let trimmed = s.trim();
    if trimmed.len() <= max {
        trimmed.to_string()
    } else if max > 3 {
        format!("{}...", &trimmed[..max - 3])
    } else {
        trimmed[..max].to_string()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn pane_border_style(panel: &PanelStyle, focused: bool) -> Style {
    if focused {
        Style::default().fg(*panel.focused_fg)
    } else {
        Style::default().fg(*panel.unfocused_fg)
    }
}
