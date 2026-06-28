use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::super::state::TuiState;
use crate::app::model::{AppModel, CommentItem};
use crate::config::StyleConfig;
use crate::review_types::{AnchorStatus, PaneFocus};

pub fn draw(
    frame: &mut Frame,
    model: &AppModel,
    tui_state: &mut TuiState,
    styles: &StyleConfig,
    area: Rect,
) {
    tui_state.comments_area = area;
    let focused = model.focus == PaneFocus::Comments;
    let border_style = super::pane_border_style(&styles.panel, focused);
    let inner_width = area.width.saturating_sub(2) as usize;
    let mut lines = Vec::new();

    if model.comments_panel.comments.is_empty() {
        lines.push(Line::from(Span::styled(
            "No comments for this file",
            Style::default().fg(*styles.diff.placeholder_fg),
        )));
    } else {
        for comment in &model.comments_panel.comments {
            push_comment_lines(&mut lines, comment, styles, inner_width);
        }
    }

    let title = format!(" Comments ({}) ", model.comments_panel.comments.len());
    let paragraph = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title)
            .title_bottom(" j/k select  Enter expand/jump  e edit  r resolve  d delete "),
    );
    frame.render_widget(paragraph, area);
}

fn push_comment_lines(
    lines: &mut Vec<Line<'static>>,
    comment: &CommentItem,
    styles: &StyleConfig,
    width: usize,
) {
    let selected_prefix = if comment.selected { ">" } else { " " };
    let state = if comment.resolved { "resolved" } else { "open" };
    let range = if comment.line_start == comment.line_end {
        format!("L{}", comment.line_start)
    } else {
        format!("L{}-L{}", comment.line_start, comment.line_end)
    };
    let anchor = match comment.anchor_status {
        AnchorStatus::Anchored => "",
        AnchorStatus::Shifted => " shifted",
        AnchorStatus::Approximate => " approximate",
        AnchorStatus::Orphaned => " orphaned",
    };
    let header = format!("{selected_prefix} #{} {range} {state}{anchor}", comment.id);
    let header_style = if comment.current {
        Style::default()
            .fg(*styles.diff.current_comment_marker_fg)
            .add_modifier(Modifier::BOLD)
    } else if comment.resolved {
        Style::default().fg(*styles.diff.placeholder_fg)
    } else {
        Style::default()
            .fg(*styles.status.bar_fg)
            .add_modifier(Modifier::BOLD)
    };
    lines.push(Line::from(Span::styled(
        truncate_chars(&header, width),
        header_style,
    )));

    if comment.expanded {
        for body_line in comment.body.lines() {
            lines.push(Line::from(Span::styled(
                format!("  {}", truncate_chars(body_line, width.saturating_sub(2))),
                Style::default().fg(*styles.status.bar_fg),
            )));
        }
    } else {
        lines.push(Line::from(Span::styled(
            format!(
                "  {}",
                truncate_chars(&comment.preview, width.saturating_sub(2))
            ),
            Style::default().fg(*styles.diff.placeholder_fg),
        )));
    }
}

fn truncate_chars(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    let take = width.saturating_sub(1);
    format!("{}\u{2026}", value.chars().take(take).collect::<String>())
}
