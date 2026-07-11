//! Top-level render function and layout management.
//!
//! The TUI has a two-pane layout: file list on the left, diff/file view
//! on the right, with a status bar at the bottom. Either pane can be
//! hidden to give the other full width.

mod comments_panel;
mod document_diff_view;
mod file_list;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use super::state::{InputMode, TuiState};
use crate::app::model::{AppModel, ReviewStatus};
use crate::config::{PanelStyle, StyleConfig};
use crate::git;

const COMMENT_TAB_WIDTH: usize = 4;

/// Draw the entire UI for the current state.
pub fn draw(frame: &mut Frame, model: &AppModel, tui_state: &mut TuiState, styles: &StyleConfig) {
    // Fill the entire frame with the application background color.
    let bg_style = Style::default().bg(*styles.bg);
    frame.render_widget(Block::default().style(bg_style), frame.area());

    // Split into main area + status bar.
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    let main_area = vertical[0];
    let status_area = vertical[1];

    // Compute pane layout based on visibility.
    match (model.layout.file_list_visible, model.layout.diff_visible) {
        (true, true) => {
            let panes = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Max(model.layout.file_list_width),
                    Constraint::Min(1),
                ])
                .split(main_area);
            tui_state.file_list_area = panes[0];
            tui_state.comments_area = Rect::default();
            file_list::draw(frame, model, tui_state, styles, panes[0]);
            draw_diff_region(frame, model, tui_state, styles, panes[1]);
        }
        (true, false) => {
            tui_state.file_list_area = main_area;
            tui_state.diff_area = Rect::default();
            file_list::draw(frame, model, tui_state, styles, main_area);
        }
        (false, true) => {
            tui_state.file_list_area = Rect::default();
            tui_state.comments_area = Rect::default();
            draw_diff_region(frame, model, tui_state, styles, main_area);
        }
        (false, false) => {
            // Should never happen — toggle logic prevents it.
            unreachable!("at least one pane must be visible");
        }
    }

    // Draw status bar or input prompt (command/search replace the status bar).
    match tui_state.input_mode {
        InputMode::Command => {
            draw_command_input(frame, tui_state, styles, status_area);
        }
        InputMode::DiffSearch => {
            draw_diff_search_input(frame, tui_state, styles, status_area);
        }
        InputMode::Comment => {
            draw_status_bar(frame, model, tui_state, styles, status_area);
        }
        InputMode::Normal => {
            draw_status_bar(frame, model, tui_state, styles, status_area);
        }
    }

    // Render mouse selection highlight on top of everything.
    if tui_state.mouse_selection.is_some() {
        draw_selection_highlight(frame, model, tui_state, styles);
    }

    // Search results overlay.
    if model.search_results.is_some() {
        draw_search_results_overlay(frame, model, tui_state, styles);
    }

    // Definition results overlay.
    if model.definition_results.is_some() {
        draw_definition_results_overlay(frame, model, styles);
    }

    if tui_state.input_mode == InputMode::Comment {
        draw_comment_input(frame, tui_state, styles);
    }

    // Help overlay on top of everything else.
    if tui_state.show_help {
        draw_help_overlay(frame, styles);
    }
}

fn draw_diff_region(
    frame: &mut Frame,
    model: &AppModel,
    tui_state: &mut TuiState,
    styles: &StyleConfig,
    area: Rect,
) {
    if model.comments_panel.visible && area.height >= 8 {
        let panel_height = TuiState::comments_panel_height(area.height);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(panel_height)])
            .split(area);
        tui_state.diff_area = chunks[0];
        document_diff_view::draw(frame, model, tui_state, styles, chunks[0]);
        comments_panel::draw(frame, model, tui_state, styles, chunks[1]);
    } else {
        tui_state.diff_area = area;
        tui_state.comments_area = Rect::default();
        document_diff_view::draw(frame, model, tui_state, styles, area);
    }
}

// ---------------------------------------------------------------------------
// Selection highlight
// ---------------------------------------------------------------------------

fn draw_selection_highlight(
    frame: &mut Frame,
    model: &AppModel,
    tui_state: &TuiState,
    styles: &StyleConfig,
) {
    let Some(sel) = &tui_state.mouse_selection else {
        return;
    };

    let area = match sel.pane {
        crate::review_types::PaneFocus::FileList => tui_state.file_list_area,
        crate::review_types::PaneFocus::Diff => tui_state.diff_area,
        crate::review_types::PaneFocus::Comments => return,
    };
    // Inner area excludes borders.
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    let (scroll, base_col) = match sel.pane {
        crate::review_types::PaneFocus::FileList => (model.file_list.scroll, 0usize),
        crate::review_types::PaneFocus::Diff => {
            (model.diff.scroll, tui_state.diff_content_start_col())
        }
        crate::review_types::PaneFocus::Comments => return,
    };

    let visible_start_line = scroll;
    let visible_end_line = visible_start_line.saturating_add(inner.height as usize);
    let (start, end) = sel.normalized();
    if end.line < visible_start_line || start.line >= visible_end_line {
        return;
    }

    let highlight = Style::default()
        .bg(*styles.selection.bg)
        .fg(*styles.selection.fg);

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

fn draw_status_bar(
    frame: &mut Frame,
    model: &AppModel,
    tui_state: &mut TuiState,
    styles: &StyleConfig,
    area: Rect,
) {
    let ss = &styles.status;

    let reviewed_count = model
        .file_list
        .sections
        .iter()
        .filter(|section| {
            section.kind != crate::app::model::FileListSectionKind::UnresolvedComments
        })
        .flat_map(|section| &section.rows)
        .filter(|row| {
            row.kind == crate::app::model::FileListRowKind::File
                && matches!(row.review_status, ReviewStatus::Reviewed { .. })
        })
        .count();
    let total = model
        .file_list
        .sections
        .iter()
        .filter(|section| {
            section.kind != crate::app::model::FileListSectionKind::UnresolvedComments
        })
        .flat_map(|section| &section.rows)
        .filter(|row| row.kind == crate::app::model::FileListRowKind::File)
        .count();

    let left = format!(
        " {} \u{2192} {} \u{2192} {} | {reviewed_count}/{total} reviewed",
        model.context.base_ref,
        git::short_hash(&model.context.merge_base),
        model.context.head_ref,
    );

    // Ctrl-C warning takes over the full bar.
    if let Some((msg, _)) = &tui_state.status_message {
        if msg.contains("Ctrl-C") {
            let bar = format!(" {msg} ");
            let status =
                Paragraph::new(bar).style(Style::default().bg(*ss.warning_bg).fg(*ss.warning_fg));
            frame.render_widget(status, area);
            return;
        }
    }

    // Right side: transient message or default hint.
    let (right, right_style) = if let Some((msg, _)) = &tui_state.status_message {
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
        Line::from("  j/k h/l       Move line / column"),
        Line::from("  Space         Page down"),
        Line::from("  Ctrl-d/u/e/y  Half-page down/up; scroll down/up"),
        Line::from("  g/G H/M/L     Top/bottom; view top/mid/bottom"),
        Line::from("  0/$ w/b W/B    Line start/end; word/big-word"),
        Line::from("  ] / [         Next / previous diff hunk"),
        Line::from("  } / {         Next / previous comment"),
        Line::from("  c             Comment current diff line"),
        Line::from("  Shift-C       Toggle comments panel"),
        Line::from("  e             Edit current comment"),
        Line::from("  r / d         Resolve or delete current comment"),
        Line::from("  A / a         Toggle approved / unapproved"),
        Line::from("  i             Toggle inline / side-by-side"),
        Line::from("  s             Cycle: diff / HEAD / base"),
        Line::from("  d             Cycle diff algorithm"),
        Line::from("  m             Toggle merge base / since review"),
        Line::from("  Ctrl-w        Toggle ignore whitespace"),
        Line::from("  Ctrl-b        Toggle blame annotations"),
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
        Line::from("  /             Search rendered diff"),
        Line::from("  n / N         Next / previous search match"),
        Line::from("  Esc           Clear active diff search"),
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
fn draw_command_input(frame: &mut Frame, tui_state: &TuiState, styles: &StyleConfig, area: Rect) {
    let ss = &styles.status;
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
fn draw_diff_search_input(
    frame: &mut Frame,
    tui_state: &TuiState,
    styles: &StyleConfig,
    area: Rect,
) {
    let ss = &styles.status;
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

/// Draw the multi-line review comment composer.
fn draw_comment_input(frame: &mut Frame, tui_state: &mut TuiState, styles: &StyleConfig) {
    let area = frame.area();
    let width = area.width.saturating_sub(4).clamp(20, 100).min(area.width);
    let wrap_width = width.saturating_sub(2).max(1) as usize;
    let body_lines = if tui_state.comment_input.is_empty() {
        vec!["Write a comment".to_string()]
    } else {
        comment_visual_lines(&tui_state.comment_input, wrap_width)
    };
    let line_count = body_lines.len();
    let max_height = area.height.saturating_div(2).max(3).min(area.height);
    let min_height = 7.min(max_height);
    let desired_height = saturating_u16(line_count.saturating_add(2));
    let height = desired_height.clamp(min_height, max_height);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height + 1);
    let popup = Rect::new(x, y, width, height);
    let inner_height = popup.height.saturating_sub(2) as usize;

    let (line, col) = comment_cursor_position(
        &tui_state.comment_input,
        tui_state.comment_cursor,
        wrap_width,
    );
    keep_comment_cursor_visible(tui_state, line, inner_height, line_count);
    let hs = &styles.help;
    let composer = Paragraph::new(
        body_lines
            .into_iter()
            .map(Line::from)
            .collect::<Vec<Line>>(),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(*hs.border_fg))
            .title(format!(" {} ", tui_state.comment_title))
            .title_bottom(" Ctrl-Space submit / Ctrl-Shift-E editor / Esc cancel "),
    )
    .scroll((saturating_u16(tui_state.comment_scroll), 0))
    .style(Style::default().fg(*hs.text_fg).bg(*hs.bg));

    frame.render_widget(Clear, popup);
    frame.render_widget(composer, popup);

    let inner_x = popup.x.saturating_add(1);
    let inner_y = popup.y.saturating_add(1);
    frame.set_cursor_position(Position {
        x: inner_x + col.min(popup.width.saturating_sub(2) as usize) as u16,
        y: inner_y
            + line
                .saturating_sub(tui_state.comment_scroll)
                .min(inner_height.saturating_sub(1)) as u16,
    });
}

fn keep_comment_cursor_visible(
    tui_state: &mut TuiState,
    cursor_line: usize,
    inner_height: usize,
    line_count: usize,
) {
    if inner_height == 0 {
        tui_state.comment_scroll = 0;
        return;
    }

    if cursor_line < tui_state.comment_scroll {
        tui_state.comment_scroll = cursor_line;
    } else if cursor_line >= tui_state.comment_scroll.saturating_add(inner_height) {
        tui_state.comment_scroll = cursor_line.saturating_sub(inner_height - 1);
    }

    let max_scroll = line_count.saturating_sub(inner_height);
    tui_state.comment_scroll = tui_state.comment_scroll.min(max_scroll);
}

pub fn comment_visual_lines(text: &str, wrap_width: usize) -> Vec<String> {
    let wrap_width = wrap_width.max(1);
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut col = 0;
    let mut wrapped_at_boundary = false;
    for ch in text.chars() {
        match ch {
            '\n' => {
                lines.push(std::mem::take(&mut current));
                col = 0;
                wrapped_at_boundary = false;
            }
            '\t' => {
                let spaces = tab_spaces(col);
                for _ in 0..spaces {
                    push_wrapped_char(
                        &mut lines,
                        &mut current,
                        &mut col,
                        &mut wrapped_at_boundary,
                        wrap_width,
                        ' ',
                    );
                }
            }
            ch => {
                push_wrapped_char(
                    &mut lines,
                    &mut current,
                    &mut col,
                    &mut wrapped_at_boundary,
                    wrap_width,
                    ch,
                );
            }
        }
    }
    if !wrapped_at_boundary || !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn comment_cursor_position(text: &str, cursor: usize, wrap_width: usize) -> (usize, usize) {
    let wrap_width = wrap_width.max(1);
    let before = &text[..cursor.min(text.len())];
    let mut row = 0;
    let mut col = 0;
    for ch in before.chars() {
        match ch {
            '\n' => {
                row += 1;
                col = 0;
            }
            '\t' => {
                for _ in 0..tab_spaces(col) {
                    advance_wrapped_cursor(&mut row, &mut col, wrap_width);
                }
            }
            _ => {
                advance_wrapped_cursor(&mut row, &mut col, wrap_width);
            }
        }
    }
    (row, col)
}

fn push_wrapped_char(
    lines: &mut Vec<String>,
    current: &mut String,
    col: &mut usize,
    wrapped_at_boundary: &mut bool,
    wrap_width: usize,
    ch: char,
) {
    if *col >= wrap_width {
        lines.push(std::mem::take(current));
        *col = 0;
        *wrapped_at_boundary = true;
    }
    current.push(ch);
    *col += 1;
    *wrapped_at_boundary = false;
    if *col >= wrap_width {
        lines.push(std::mem::take(current));
        *col = 0;
        *wrapped_at_boundary = true;
    }
}

fn advance_wrapped_cursor(row: &mut usize, col: &mut usize, wrap_width: usize) {
    *col += 1;
    if *col >= wrap_width {
        *row += 1;
        *col = 0;
    }
}

fn tab_spaces(col: usize) -> usize {
    COMMENT_TAB_WIDTH - (col % COMMENT_TAB_WIDTH)
}

// ---------------------------------------------------------------------------
// Search results overlay
// ---------------------------------------------------------------------------

/// Draw the search results overlay as a centered popup.
fn draw_search_results_overlay(
    frame: &mut Frame,
    model: &AppModel,
    tui_state: &mut TuiState,
    styles: &StyleConfig,
) {
    let results = match &model.search_results {
        Some(r) => r,
        None => return,
    };
    let hs = &styles.help;
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
    if total == 0 {
        tui_state.search_results_scroll = 0;
    } else if results.selected < tui_state.search_results_scroll {
        tui_state.search_results_scroll = results.selected;
    } else if results.selected >= tui_state.search_results_scroll + inner_height {
        tui_state.search_results_scroll = results.selected.saturating_sub(inner_height - 1);
    }
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
        let visible_start = tui_state.search_results_scroll;
        let visible_end = (visible_start + inner_height).min(total);
        for i in visible_start..visible_end {
            let m = &results.matches[i];
            let is_selected = m.selected;
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
fn draw_definition_results_overlay(frame: &mut Frame, model: &AppModel, styles: &StyleConfig) {
    let results = match &model.definition_results {
        Some(r) => r,
        None => return,
    };
    let hs = &styles.help;
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
        for def in &results.definitions {
            let is_selected = def.selected;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_cursor_position_accounts_for_wrapped_rows() {
        let text = "abcdef\nghi";

        assert_eq!(
            comment_visual_lines(text, 4),
            vec!["abcd".to_string(), "ef".to_string(), "ghi".to_string()]
        );
        assert_eq!(comment_cursor_position(text, 4, 4), (1, 0));
        assert_eq!(comment_cursor_position(text, 6, 4), (1, 2));
        assert_eq!(comment_cursor_position(text, 10, 4), (2, 3));
    }

    #[test]
    fn comment_visual_lines_expand_tabs() {
        let text = "\t\ta";

        assert_eq!(
            comment_visual_lines(text, 10),
            vec!["        a".to_string()]
        );
        assert_eq!(comment_cursor_position(text, 1, 10), (0, 4));
        assert_eq!(comment_cursor_position(text, 2, 10), (0, 8));
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
