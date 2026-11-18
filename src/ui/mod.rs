//! Top-level render function and layout management.
//!
//! The TUI has a two-pane layout: file list on the left, diff/file view
//! on the right, with a status bar at the bottom. Either pane can be
//! hidden to give the other full width.

pub mod comments;
pub mod diff_view;
pub mod file_list;

use std::time::Duration;

use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::AppState;
use crate::git;
use crate::model::ReviewStatus;

/// File list pane width as a percentage of terminal width.
const FILE_LIST_PCT: u16 = 30;

/// Draw the entire UI for the current state.
pub fn draw(frame: &mut Frame, state: &mut AppState) {
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
                .constraints([
                    Constraint::Percentage(FILE_LIST_PCT),
                    Constraint::Percentage(100 - FILE_LIST_PCT),
                ])
                .split(main_area);
            state.file_list_area = panes[0];
            state.diff_area = panes[1];
            file_list::draw(frame, state, panes[0]);
            diff_view::draw(frame, state, panes[1]);
        }
        (true, false) => {
            state.file_list_area = main_area;
            state.diff_area = Rect::default();
            file_list::draw(frame, state, main_area);
        }
        (false, true) => {
            state.file_list_area = Rect::default();
            state.diff_area = main_area;
            diff_view::draw(frame, state, main_area);
        }
        (false, false) => {
            // Should never happen — toggle logic prevents it.
            unreachable!("at least one pane must be visible");
        }
    }

    draw_status_bar(frame, state, status_area);

    // Render mouse selection highlight on top of everything.
    if let Some(sel) = &state.mouse_selection {
        draw_selection_highlight(frame, sel);
    }

    // Help overlay on top of everything else.
    if state.show_help {
        draw_help_overlay(frame);
    }
}

// ---------------------------------------------------------------------------
// Selection highlight
// ---------------------------------------------------------------------------

fn draw_selection_highlight(frame: &mut Frame, sel: &crate::app::MouseSelection) {
    let area = sel.pane_area;
    // Inner area excludes borders.
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    let (start_col, start_row, end_col, end_row) = sel.normalized();

    let highlight = Style::default().bg(Color::Indexed(238)).fg(Color::White);

    let buf = frame.buffer_mut();
    for row in start_row..=end_row {
        if row < inner.y || row >= inner.bottom() {
            continue;
        }

        let col_start = if row == start_row {
            start_col.max(inner.x)
        } else {
            inner.x
        };
        let col_end = if row == end_row {
            end_col.min(inner.right().saturating_sub(1))
        } else {
            inner.right().saturating_sub(1)
        };

        for col in col_start..=col_end {
            if let Some(cell) = buf.cell_mut(Position { x: col, y: row }) {
                cell.set_style(highlight);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

/// Status message timeout.
const STATUS_MSG_TIMEOUT: Duration = Duration::from_secs(3);

fn draw_status_bar(frame: &mut Frame, state: &mut AppState, area: Rect) {
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
                Paragraph::new(bar).style(Style::default().bg(Color::Red).fg(Color::White));
            frame.render_widget(status, area);
            return;
        }
    }

    // Right side: transient message or default hint.
    let (right, right_style) = if let Some((msg, _)) = &state.status_message {
        (
            format!(" {msg} "),
            Style::default().bg(Color::Yellow).fg(Color::Black),
        )
    } else {
        (
            " ? help ".to_string(),
            Style::default().bg(Color::DarkGray).fg(Color::Gray),
        )
    };

    let bar_style = Style::default().bg(Color::DarkGray).fg(Color::White);

    // Split the status bar into left and right sections.
    let right_width = right.len() as u16;
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

fn draw_help_overlay(frame: &mut Frame) {
    let area = frame.area();

    // Center the help box, capped at reasonable dimensions.
    let help_width = 56u16.min(area.width.saturating_sub(4));
    let help_height = 28u16.min(area.height.saturating_sub(2));
    let x = (area.width.saturating_sub(help_width)) / 2;
    let y = (area.height.saturating_sub(help_height)) / 2;
    let help_area = Rect::new(x, y, help_width, help_height);

    // Dim the background.
    let buf = frame.buffer_mut();
    for row in area.y..area.bottom() {
        for col in area.x..area.right() {
            if let Some(cell) = buf.cell_mut(Position { x: col, y: row }) {
                cell.set_style(Style::default().fg(Color::DarkGray));
            }
        }
    }

    let help_text = vec![
        Line::from(Span::styled(
            " Keyboard Shortcuts ",
            Style::default()
                .fg(Color::Cyan)
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
        Line::from("  s             Cycle: diff / HEAD / base"),
        Line::from("  Ctrl-i / o    Next / previous diff hunk"),
        Line::from("  1 / 2         Toggle file list / diff pane"),
        Line::from("  Ctrl-c        Press twice to quit"),
        Line::from(""),
        Line::from(Span::styled(
            " File List",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  j / k         Move down / up"),
        Line::from("  g / G         First / last file"),
        Line::from("  Enter         Focus diff pane"),
        Line::from(""),
        Line::from(Span::styled(
            " Diff View",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  j / k         Scroll down / up"),
        Line::from("  Space / Ctrl-b Page down / up"),
        Line::from("  Ctrl-d / u    Half page down / up"),
        Line::from("  g / G         Top / bottom"),
        Line::from("  Enter         Expand reviewed file diff"),
        Line::from(""),
        Line::from(Span::styled(
            " Mouse",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  Drag          Select text (auto-copy)"),
        Line::from("  Scroll        Scroll diff pane"),
    ];

    // Clear the area first so no old content bleeds through.
    frame.render_widget(Clear, help_area);

    let help = Paragraph::new(help_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" Help ")
                .title_bottom(" ? / q / Esc to close "),
        )
        .style(Style::default().fg(Color::White).bg(Color::Black));

    frame.render_widget(help, help_area);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn pane_border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}
