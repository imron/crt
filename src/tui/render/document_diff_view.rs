//! Diff/file rendering backed by the Stage 29 document model.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::super::state::TuiState;
use crate::app::document::{
    BlameInfo, DiffDocument, Document, RenderContent, RenderLine, TextRunKind,
};
use crate::app::model::{AppModel, ReviewStatus};
use crate::config::StyleConfig;
use crate::review_types::{ContentMode, LineKind, PaneFocus, RenderVariant};

const BLAME_COL_WIDTH: usize = 30;
const COMMENT_MARKER_WIDTH: usize = 1;
const SIDE_BY_SIDE_DIVIDER: &str = " │ ";

/// Draw the diff/file view pane from `ActiveDocument`.
pub fn draw(
    frame: &mut Frame,
    model: &AppModel,
    tui_state: &mut TuiState,
    styles: &StyleConfig,
    area: Rect,
) {
    let diff = &model.diff;
    let focused = model.focus == PaneFocus::Diff;
    let mut border_style = super::pane_border_style(&styles.panel, focused);
    if focused {
        border_style = border_style.fg(Color::Green);
    }

    let inner_w = area.width.saturating_sub(2) as usize;
    let diff_view_height = area.height.saturating_sub(2) as usize;
    tui_state.diff_view_height = diff_view_height;

    let Some(active_document) = model.active_document.as_ref() else {
        draw_placeholder(frame, area, border_style, " Diff ", "No files changed.");
        reset_tui_document_state(tui_state);
        return;
    };

    if matches!(diff.review_status, Some(ReviewStatus::Reviewed { .. }))
        && !diff.reviewed_diff_expanded
    {
        draw_reviewed_summary(frame, diff, area, border_style, styles);
        reset_tui_document_state(tui_state);
        return;
    }

    if diff.is_binary {
        let title = diff_title(diff, tui_state, 0);
        draw_placeholder(frame, area, border_style, &title, "  Binary file");
        reset_tui_document_state(tui_state);
        return;
    }

    let hunk_spans = active_document.diff.hunk_spans();
    tui_state.hunk_start_rows = hunk_spans
        .iter()
        .map(|span| span.full_span.start.0)
        .collect();
    tui_state.hunk_end_rows = hunk_spans.iter().map(|span| span.full_span.end.0).collect();
    tui_state.hunk_first_change_rows = hunk_spans.iter().map(|span| span.first_change.0).collect();
    tui_state.diff_content_height = active_document.diff.len();

    if tui_state.document_diff_rendered_text_key.as_ref() != Some(&active_document.key) {
        tui_state.diff_rendered_text = rendered_text(&active_document.diff, diff.show_blame);
        tui_state.document_diff_rendered_text_key = Some(active_document.key.clone());
    }

    let layout = DocumentLayout::new(&active_document.diff, diff.show_blame, inner_w);
    tui_state.diff_gutter_cols = layout.gutter_cols;
    tui_state.diff_content_start_col = layout.content_start_col;

    let visible = visible_lines(
        &active_document.diff,
        diff,
        styles,
        &layout,
        diff.scroll,
        diff_view_height,
        inner_w,
    );
    let title = diff_title(diff, tui_state, hunk_spans.len());
    let paragraph = Paragraph::new(visible).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title),
    );

    frame.render_widget(paragraph, area);
}

fn reset_tui_document_state(tui_state: &mut TuiState) {
    tui_state.hunk_start_rows.clear();
    tui_state.hunk_end_rows.clear();
    tui_state.hunk_first_change_rows.clear();
    tui_state.diff_gutter_cols = 0;
    tui_state.diff_content_start_col = 0;
    tui_state.diff_content_height = 0;
    tui_state.diff_rendered_text.clear();
    tui_state.document_diff_rendered_text_key = None;
}

fn draw_placeholder(
    frame: &mut Frame,
    area: Rect,
    border_style: Style,
    title: &str,
    message: &str,
) {
    let paragraph = Paragraph::new(vec![Line::from(message.to_string())]).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title.to_string()),
    );
    frame.render_widget(paragraph, area);
}

fn draw_reviewed_summary(
    frame: &mut Frame,
    diff: &crate::app::model::DiffPanel,
    area: Rect,
    border_style: Style,
    styles: &StyleConfig,
) {
    let path = diff.path.as_deref().unwrap_or("Diff");
    let at = match &diff.review_status {
        Some(ReviewStatus::Reviewed { at, .. }) => at.as_str(),
        _ => "",
    };
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {path} - reviewed at {at}"),
            Style::default().fg(*styles.diff.reviewed_fg),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Press Enter to view diff.",
            Style::default().fg(*styles.diff.placeholder_fg),
        )),
    ];
    let paragraph = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(format!(" {path} ")),
    );
    frame.render_widget(paragraph, area);
}

#[derive(Debug, Clone, Copy)]
struct DocumentLayout {
    gutter_w: usize,
    base_gutter_w: usize,
    head_gutter_w: usize,
    gutter_cols: usize,
    content_start_col: usize,
}

impl DocumentLayout {
    fn new(diff: &DiffDocument, show_blame: bool, inner_w: usize) -> Self {
        match diff {
            DiffDocument::SideBySide(document) => {
                let base_gutter_w = gutter_width(document.base());
                let head_gutter_w = gutter_width(document.head());
                let side_w = side_width(inner_w);
                let content_start_col = side_w
                    + SIDE_BY_SIDE_DIVIDER.chars().count()
                    + side_content_start(head_gutter_w, show_blame);
                Self {
                    gutter_w: base_gutter_w.max(head_gutter_w),
                    base_gutter_w,
                    head_gutter_w,
                    gutter_cols: content_start_col,
                    content_start_col,
                }
            }
            DiffDocument::Unified(document)
            | DiffDocument::Base(document)
            | DiffDocument::Head(document) => {
                let gutter_w = gutter_width(document);
                let content_start_col = side_content_start(gutter_w, show_blame);
                Self {
                    gutter_w,
                    base_gutter_w: gutter_w,
                    head_gutter_w: gutter_w,
                    gutter_cols: content_start_col,
                    content_start_col,
                }
            }
        }
    }
}

fn side_content_start(gutter_w: usize, show_blame: bool) -> usize {
    blame_cols(show_blame) + gutter_w + 1 + COMMENT_MARKER_WIDTH + 3
}

fn blame_cols(show_blame: bool) -> usize {
    if show_blame { BLAME_COL_WIDTH + 1 } else { 0 }
}

fn side_width(inner_w: usize) -> usize {
    inner_w.saturating_sub(SIDE_BY_SIDE_DIVIDER.chars().count()) / 2
}

fn gutter_width(document: &Document) -> usize {
    document
        .rows()
        .iter()
        .filter_map(|row| match row {
            crate::app::document::DocumentRow::Content(content) => Some(content.gutter.text.len()),
            crate::app::document::DocumentRow::Spacer => None,
        })
        .max()
        .unwrap_or(0)
}

fn visible_lines(
    document: &DiffDocument,
    diff: &crate::app::model::DiffPanel,
    styles: &StyleConfig,
    layout: &DocumentLayout,
    scroll: usize,
    height: usize,
    inner_w: usize,
) -> Vec<Line<'static>> {
    (scroll..scroll.saturating_add(height))
        .filter_map(|row| {
            let is_cursor = row == diff.cursor.line;
            match document {
                DiffDocument::Unified(document)
                | DiffDocument::Base(document)
                | DiffDocument::Head(document) => document
                    .line(crate::app::document::RowIndex(row))
                    .map(|line| {
                        render_single_line(
                            line,
                            styles,
                            layout.gutter_w,
                            inner_w,
                            diff.show_blame,
                            is_cursor,
                        )
                    }),
                DiffDocument::SideBySide(document) => {
                    if row >= document.len() {
                        return None;
                    }
                    Some(render_side_by_side_line(
                        document.base().line(crate::app::document::RowIndex(row)),
                        document.head().line(crate::app::document::RowIndex(row)),
                        styles,
                        layout,
                        inner_w,
                        diff.show_blame,
                        is_cursor,
                    ))
                }
            }
        })
        .collect()
}

fn render_single_line(
    line: RenderLine<'_>,
    styles: &StyleConfig,
    gutter_w: usize,
    inner_w: usize,
    show_blame: bool,
    is_cursor: bool,
) -> Line<'static> {
    match line {
        RenderLine::Content(content) => {
            render_content_line(content, styles, gutter_w, inner_w, show_blame, is_cursor)
        }
        RenderLine::Spacer { marker } => render_spacer_line(marker_text(marker), inner_w, styles),
    }
}

fn render_side_by_side_line(
    base: Option<RenderLine<'_>>,
    head: Option<RenderLine<'_>>,
    styles: &StyleConfig,
    layout: &DocumentLayout,
    inner_w: usize,
    show_blame: bool,
    is_cursor: bool,
) -> Line<'static> {
    let side_w = side_width(inner_w);
    let base_line = render_side_line(
        base,
        styles,
        layout.base_gutter_w,
        side_w,
        show_blame,
        is_cursor,
    );
    let head_line = render_side_line(
        head,
        styles,
        layout.head_gutter_w,
        side_w,
        show_blame,
        is_cursor,
    );
    let mut spans = base_line.spans;
    spans.push(Span::styled(
        SIDE_BY_SIDE_DIVIDER.to_string(),
        Style::default().fg(*styles.diff.gutter_fg),
    ));
    spans.extend(head_line.spans);
    Line::from(spans)
}

fn render_side_line(
    line: Option<RenderLine<'_>>,
    styles: &StyleConfig,
    gutter_w: usize,
    width: usize,
    show_blame: bool,
    is_cursor: bool,
) -> Line<'static> {
    match line {
        Some(RenderLine::Content(content)) => {
            render_content_line(content, styles, gutter_w, width, show_blame, is_cursor)
        }
        Some(RenderLine::Spacer { marker }) => {
            render_spacer_line(marker_text(marker), width, styles)
        }
        None => render_spacer_line(String::new(), width, styles),
    }
}

fn render_content_line(
    content: RenderContent<'_>,
    styles: &StyleConfig,
    gutter_w: usize,
    width: usize,
    show_blame: bool,
    is_cursor: bool,
) -> Line<'static> {
    let base_style = line_style(content.kind, styles, is_cursor);
    let bg = base_style.bg.unwrap_or(*styles.bg);
    let gutter_style = Style::default().fg(*styles.diff.gutter_fg).bg(bg);
    let marker_style = if content.marker.is_current() {
        gutter_style.fg(*styles.diff.current_comment_marker_fg)
    } else {
        gutter_style
    };

    let mut spans = Vec::new();
    let blame_width = if show_blame {
        spans.push(Span::styled(
            format_blame(content.blame),
            Style::default().fg(*styles.diff.blame_fg).bg(bg),
        ));
        spans.push(Span::styled(" ", gutter_style));
        BLAME_COL_WIDTH + 1
    } else {
        0
    };
    spans.push(Span::styled(
        format!("{:>gutter_w$}", content.gutter.as_ref()),
        gutter_style,
    ));
    spans.push(Span::styled(" ", gutter_style));
    spans.push(Span::styled(marker_text(content.marker), marker_style));

    let prefix = match content.kind {
        LineKind::Addition => "+",
        LineKind::Deletion => "-",
        LineKind::Context => " ",
    };
    spans.push(Span::styled(format!(" {prefix} "), base_style));

    let fixed_width = blame_width + gutter_w + 1 + COMMENT_MARKER_WIDTH + 3;
    let mut content_width = 0;
    for run in content.runs {
        let run_style = match run.kind {
            TextRunKind::Plain => base_style,
            TextRunKind::SearchMatch => base_style.bg(*styles.diff.search_match_bg),
        };
        content_width += run.text.chars().count();
        spans.push(Span::styled(run.text.to_string(), run_style));
    }
    let pad = width.saturating_sub(fixed_width + content_width);
    spans.push(Span::styled(" ".repeat(pad), base_style));
    Line::from(spans)
}

fn render_spacer_line(marker: String, width: usize, styles: &StyleConfig) -> Line<'static> {
    let gutter_style = Style::default().fg(*styles.diff.gutter_fg);
    let marker_width = marker.chars().count();
    let mut spans = Vec::new();
    spans.push(Span::styled(marker, gutter_style));
    spans.push(Span::raw(" ".repeat(width.saturating_sub(marker_width))));
    Line::from(spans)
}

fn line_style(kind: LineKind, styles: &StyleConfig, is_cursor: bool) -> Style {
    let style = match kind {
        LineKind::Addition => Style::default()
            .fg(*styles.diff.addition_fg)
            .bg(*styles.diff.addition_bg),
        LineKind::Deletion => Style::default()
            .fg(*styles.diff.deletion_fg)
            .bg(*styles.diff.deletion_bg),
        LineKind::Context => Style::default().fg(*styles.diff.context_fg),
    };
    if is_cursor {
        style.bg(*styles.diff.cursor_line_bg)
    } else {
        style
    }
}

fn marker_text(marker: crate::app::model::CommentMarker) -> String {
    match marker.kind() {
        None => " ".to_string(),
        Some(crate::app::model::CommentMarkerKind::Join) => "┃".to_string(),
        Some(
            crate::app::model::CommentMarkerKind::SingleLine
            | crate::app::model::CommentMarkerKind::Start
            | crate::app::model::CommentMarkerKind::End,
        ) => {
            if marker.is_resolved() {
                "○".to_string()
            } else {
                "●".to_string()
            }
        }
    }
}

fn format_blame(blame: Option<&BlameInfo>) -> String {
    match blame {
        Some(blame) => {
            let hash: String = blame.hash.chars().take(7).collect();
            let author_budget = BLAME_COL_WIDTH.saturating_sub(19);
            let author: String = blame.author.chars().take(author_budget).collect();
            let text = format!("{hash:<7} {} {author}", blame.date);
            format!("{text:<BLAME_COL_WIDTH$}")
        }
        None => " ".repeat(BLAME_COL_WIDTH),
    }
}

fn rendered_text(document: &DiffDocument, show_blame: bool) -> Vec<String> {
    match document {
        DiffDocument::Unified(document)
        | DiffDocument::Base(document)
        | DiffDocument::Head(document) => (0..document.len())
            .filter_map(|row| document.line(crate::app::document::RowIndex(row)))
            .map(|line| render_line_text(line, show_blame))
            .collect(),
        DiffDocument::SideBySide(document) => (0..document.len())
            .map(|row| {
                let row = crate::app::document::RowIndex(row);
                format!(
                    "{}{}{}",
                    document
                        .base()
                        .line(row)
                        .map(|line| render_line_text(line, show_blame))
                        .unwrap_or_default(),
                    SIDE_BY_SIDE_DIVIDER,
                    document
                        .head()
                        .line(row)
                        .map(|line| render_line_text(line, show_blame))
                        .unwrap_or_default()
                )
            })
            .collect(),
    }
}

fn render_line_text(line: RenderLine<'_>, show_blame: bool) -> String {
    match line {
        RenderLine::Content(content) => {
            let mut text = String::new();
            if show_blame {
                text.push_str(&format_blame(content.blame));
                text.push(' ');
            }
            text.push_str(content.gutter.as_ref());
            text.push(' ');
            text.push_str(&marker_text(content.marker));
            text.push(' ');
            text.push(match content.kind {
                LineKind::Addition => '+',
                LineKind::Deletion => '-',
                LineKind::Context => ' ',
            });
            text.push(' ');
            for run in content.runs {
                text.push_str(run.text.as_ref());
            }
            text
        }
        RenderLine::Spacer { marker } => marker_text(marker),
    }
}

fn diff_title(
    diff: &crate::app::model::DiffPanel,
    tui_state: &TuiState,
    total_hunks: usize,
) -> String {
    let path = diff.path.as_deref().unwrap_or("Diff");
    let mode_label: String = match diff.content_mode {
        ContentMode::FullFile => match diff.render_variant {
            RenderVariant::HeadVersion => " (HEAD)".into(),
            RenderVariant::BaseVersion => " (base)".into(),
            _ => String::new(),
        },
        ContentMode::Diff => {
            let sbs = if diff.render_variant == RenderVariant::SideBySide {
                " sbs"
            } else {
                ""
            };
            if diff.diff_algorithm != diff.default_diff_algorithm {
                let algo = diff.diff_algorithm.label();
                format!(" (diff:{algo}{sbs})")
            } else if !sbs.is_empty() {
                format!(" (diff{sbs})")
            } else {
                " (diff)".into()
            }
        }
    };
    let ws_label = if diff.ignore_whitespace { " -w" } else { "" };
    let base_label = if !diff.show_merge_base {
        match &diff.review_status {
            Some(ReviewStatus::Reviewed {
                reviewed_commit: Some(_),
                ..
            })
            | Some(ReviewStatus::Changed {
                reviewed_commit: Some(_),
                ..
            }) => " [since review]",
            _ => "",
        }
    } else {
        ""
    };
    let hunk_info = if total_hunks == 0 {
        String::new()
    } else if let Some(idx) = tui_state.current_hunk_index_at(diff.cursor.line) {
        format!(" - {}/{total_hunks}", idx + 1)
    } else {
        String::new()
    };
    format!(" {path}{mode_label}{ws_label}{base_label}{hunk_info} ")
}
