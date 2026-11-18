use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::super::state::TuiState;
use super::comment_visual_lines;
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
            "No selected comment",
            Style::default().fg(*styles.diff.placeholder_fg),
        )));
    } else {
        for comment in &model.comments_panel.comments {
            push_comment_lines(&mut lines, comment, styles, inner_width);
        }
    }

    let title = comments_panel_title(model);
    let paragraph = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title)
            .title_bottom(" Enter expand/jump  e edit  r resolve  d delete "),
    );
    frame.render_widget(paragraph, area);
}

fn push_comment_lines(
    lines: &mut Vec<Line<'static>>,
    comment: &CommentItem,
    styles: &StyleConfig,
    width: usize,
) {
    if comment.expanded {
        for body_line in comment_body_visual_lines(&comment.body, width) {
            lines.push(Line::from(Span::styled(
                body_line,
                Style::default().fg(*styles.status.bar_fg),
            )));
        }
    } else {
        for preview_line in comment_body_visual_lines(&comment.preview, width) {
            lines.push(Line::from(Span::styled(
                preview_line,
                Style::default().fg(*styles.diff.placeholder_fg),
            )));
        }
    }
}

fn comment_body_visual_lines(text: &str, width: usize) -> Vec<String> {
    comment_visual_lines(text, width)
}

fn comments_panel_title(model: &AppModel) -> String {
    let index = model.comments_panel.selected_index.unwrap_or(0);
    let total = model.comments_panel.total;
    let Some(comment) = model.comments_panel.comments.first() else {
        return format!(" Comment ({index}/{total}) ");
    };
    let range = if comment.line_start == comment.line_end {
        format!("L{}", comment.line_start)
    } else {
        format!("L{}-L{}", comment.line_start, comment.line_end)
    };
    let state = if comment.resolved { "resolved" } else { "open" };
    let anchor = match comment.anchor_status {
        AnchorStatus::Anchored => "",
        AnchorStatus::Shifted => " shifted",
        AnchorStatus::Approximate => " approximate",
        AnchorStatus::Orphaned => " orphaned",
    };
    format!(
        " Comment ({index}/{total}) #{} {range} {state}{anchor} ",
        comment.id
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_body_visual_lines_wraps_long_lines() {
        assert_eq!(
            comment_body_visual_lines("abcdef", 4),
            vec!["abcd".to_string(), "ef".to_string()]
        );
    }

    #[test]
    fn comment_body_visual_lines_expands_tabs_like_composer() {
        assert_eq!(
            comment_body_visual_lines("\t\ta", 12),
            vec!["        a".to_string()]
        );
    }
}
