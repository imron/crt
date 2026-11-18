//! Diff/file rendering backed by the document model.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::super::state::TuiState;
use super::diff_view::apply_col_cursor;
use crate::app::document::{
    BlameInfo, DiffDocument, Document, DocumentRow, RenderContent, RenderLine, SourceLocation,
    TextRunKind,
};
use crate::app::model::{AppModel, ReviewStatus};
use crate::config::StyleConfig;
use crate::review_types::{CommentAnchorSide, ContentMode, LineKind, PaneFocus, RenderVariant};

const BLAME_COL_WIDTH: usize = 30;
const COMMENT_MARKER_WIDTH: usize = 1;
const SIDE_BY_SIDE_DIVIDER: &str = " │ ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GutterMode {
    Unified,
    Single,
    ReservedBase,
    ReservedHead,
}

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
                let base_gutter_w = single_gutter_width(document.base());
                let head_gutter_w = single_gutter_width(document.head());
                let side_w = side_width(inner_w);
                let content_start_col = side_w
                    + SIDE_BY_SIDE_DIVIDER.chars().count()
                    + single_content_start(head_gutter_w, show_blame);
                Self {
                    gutter_w: base_gutter_w.max(head_gutter_w),
                    base_gutter_w,
                    head_gutter_w,
                    gutter_cols: content_start_col,
                    content_start_col,
                }
            }
            DiffDocument::Unified(document) => {
                let gutter_w = unified_gutter_width(document);
                let content_start_col = unified_content_start(gutter_w, show_blame);
                Self {
                    gutter_w,
                    base_gutter_w: gutter_w,
                    head_gutter_w: gutter_w,
                    gutter_cols: content_start_col,
                    content_start_col,
                }
            }
            DiffDocument::Base(document) | DiffDocument::Head(document) => {
                let gutter_w = single_gutter_width(document);
                let content_start_col = unified_content_start(gutter_w, show_blame);
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

fn unified_content_start(gutter_w: usize, show_blame: bool) -> usize {
    blame_cols(show_blame) + gutter_w + 1 + gutter_w + 1 + COMMENT_MARKER_WIDTH + 3
}

fn single_content_start(gutter_w: usize, show_blame: bool) -> usize {
    blame_cols(show_blame) + gutter_w + 1 + COMMENT_MARKER_WIDTH + 3
}

fn blame_cols(show_blame: bool) -> usize {
    if show_blame { BLAME_COL_WIDTH + 1 } else { 0 }
}

fn side_width(inner_w: usize) -> usize {
    inner_w.saturating_sub(SIDE_BY_SIDE_DIVIDER.chars().count()) / 2
}

fn unified_gutter_width(document: &Document) -> usize {
    document
        .rows()
        .iter()
        .filter_map(|row| match row {
            DocumentRow::Content(content) => Some(content.source),
            DocumentRow::Spacer => None,
        })
        .flat_map(|source| [source.base, source.head])
        .flatten()
        .map(decimal_width)
        .max()
        .unwrap_or(0)
        .max(3)
}

fn single_gutter_width(document: &Document) -> usize {
    document
        .rows()
        .iter()
        .filter_map(|row| match row {
            DocumentRow::Content(content) => single_source_line(content.source),
            DocumentRow::Spacer => None,
        })
        .map(decimal_width)
        .max()
        .unwrap_or(0)
        .max(3)
}

fn decimal_width(line: u32) -> usize {
    line.to_string().len()
}

fn single_source_line(source: SourceLocation) -> Option<u32> {
    source.head.or(source.base)
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
                DiffDocument::Unified(document) => document
                    .line(crate::app::document::RowIndex(row))
                    .map(|line| {
                        with_column_cursor(
                            render_single_line(
                                line,
                                styles,
                                layout.gutter_w,
                                GutterMode::Unified,
                                inner_w,
                                diff.show_blame,
                                is_cursor,
                            ),
                            layout.content_start_col,
                            diff.cursor.column,
                            is_cursor,
                        )
                    }),
                DiffDocument::Base(document) => document
                    .line(crate::app::document::RowIndex(row))
                    .map(|line| {
                        with_column_cursor(
                            render_single_line(
                                line,
                                styles,
                                layout.gutter_w,
                                GutterMode::ReservedBase,
                                inner_w,
                                diff.show_blame,
                                is_cursor,
                            ),
                            layout.content_start_col,
                            diff.cursor.column,
                            is_cursor,
                        )
                    }),
                DiffDocument::Head(document) => document
                    .line(crate::app::document::RowIndex(row))
                    .map(|line| {
                        with_column_cursor(
                            render_single_line(
                                line,
                                styles,
                                layout.gutter_w,
                                GutterMode::ReservedHead,
                                inner_w,
                                diff.show_blame,
                                is_cursor,
                            ),
                            layout.content_start_col,
                            diff.cursor.column,
                            is_cursor,
                        )
                    }),
                DiffDocument::SideBySide(document) => {
                    if row >= document.len() {
                        return None;
                    }
                    Some(with_column_cursor(
                        render_side_by_side_line(
                            document.base().line(crate::app::document::RowIndex(row)),
                            document.head().line(crate::app::document::RowIndex(row)),
                            styles,
                            layout,
                            inner_w,
                            diff.show_blame,
                            is_cursor,
                        ),
                        layout.content_start_col,
                        diff.cursor.column,
                        is_cursor,
                    ))
                }
            }
        })
        .collect()
}

fn with_column_cursor(
    line: Line<'static>,
    content_start_col: usize,
    cursor_col: usize,
    is_cursor: bool,
) -> Line<'static> {
    if is_cursor {
        apply_col_cursor(&line, content_start_col, cursor_col)
    } else {
        line
    }
}

fn render_single_line(
    line: RenderLine<'_>,
    styles: &StyleConfig,
    gutter_w: usize,
    gutter_mode: GutterMode,
    inner_w: usize,
    show_blame: bool,
    is_cursor: bool,
) -> Line<'static> {
    match line {
        RenderLine::Content(content) => render_content_line(
            content,
            styles,
            gutter_w,
            gutter_mode,
            inner_w,
            show_blame,
            is_cursor,
        ),
        RenderLine::Spacer { marker } => {
            render_spacer_line(marker_text(marker), inner_w, styles, is_cursor)
        }
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
        spacer_style(styles, is_cursor).fg(*styles.diff.gutter_fg),
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
    let rendered = match line {
        Some(RenderLine::Content(content)) => render_content_line(
            content,
            styles,
            gutter_w,
            GutterMode::Single,
            width,
            show_blame,
            is_cursor,
        ),
        Some(RenderLine::Spacer { marker }) => {
            render_spacer_line(marker_text(marker), width, styles, is_cursor)
        }
        None => render_spacer_line(String::new(), width, styles, is_cursor),
    };
    clip_line(rendered, width)
}

fn render_content_line(
    content: RenderContent<'_>,
    styles: &StyleConfig,
    gutter_w: usize,
    gutter_mode: GutterMode,
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
    let gutter_width = match gutter_mode {
        GutterMode::Unified => {
            push_unified_gutter(&mut spans, content.source, gutter_w, gutter_style);
            gutter_w + 1 + gutter_w + 1
        }
        GutterMode::Single => {
            push_single_gutter(&mut spans, &content, gutter_w, gutter_style);
            gutter_w + 1
        }
        GutterMode::ReservedBase => {
            push_reserved_gutter(
                &mut spans,
                &content,
                gutter_w,
                gutter_style,
                CommentAnchorSide::Base,
            );
            gutter_w + 1 + gutter_w + 1
        }
        GutterMode::ReservedHead => {
            push_reserved_gutter(
                &mut spans,
                &content,
                gutter_w,
                gutter_style,
                CommentAnchorSide::Head,
            );
            gutter_w + 1 + gutter_w + 1
        }
    };
    spans.push(Span::styled(marker_text(content.marker), marker_style));

    let prefix = match content.kind {
        LineKind::Addition => "+",
        LineKind::Deletion => "-",
        LineKind::Context => " ",
    };
    spans.push(Span::styled(format!(" {prefix} "), base_style));

    let fixed_width = blame_width + gutter_width + COMMENT_MARKER_WIDTH + 3;
    let mut content_width = 0;
    for run in content.runs {
        let run_style = match run.kind {
            TextRunKind::Plain => base_style,
            TextRunKind::Changed => match content.kind {
                LineKind::Addition => base_style.bg(*styles.diff.addition_emphasis_bg),
                LineKind::Deletion => base_style.bg(*styles.diff.deletion_emphasis_bg),
                LineKind::Context => base_style,
            },
            TextRunKind::SearchMatch => base_style.bg(*styles.diff.search_match_bg),
        };
        content_width += run.text.chars().count();
        spans.push(Span::styled(run.text.to_string(), run_style));
    }
    let pad = width.saturating_sub(fixed_width + content_width);
    spans.push(Span::styled(" ".repeat(pad), base_style));
    Line::from(spans)
}

fn clip_line(line: Line<'static>, width: usize) -> Line<'static> {
    if width == 0 {
        return Line::from(Vec::<Span<'static>>::new());
    }

    let mut remaining = width;
    let mut clipped = Vec::new();
    for span in line.spans {
        if remaining == 0 {
            break;
        }

        let text = span.content.as_ref();
        let char_count = text.chars().count();
        if char_count <= remaining {
            remaining -= char_count;
            clipped.push(span);
            continue;
        }

        let truncated: String = text.chars().take(remaining).collect();
        clipped.push(Span::styled(truncated, span.style));
        break;
    }

    Line::from(clipped)
}

fn push_unified_gutter(
    spans: &mut Vec<Span<'static>>,
    source: SourceLocation,
    gutter_w: usize,
    gutter_style: Style,
) {
    spans.push(Span::styled(
        format_optional_line(source.base, gutter_w),
        gutter_style,
    ));
    spans.push(Span::styled(" ", gutter_style));
    spans.push(Span::styled(
        format_optional_line(source.head, gutter_w),
        gutter_style,
    ));
    spans.push(Span::styled(" ", gutter_style));
}

fn push_single_gutter(
    spans: &mut Vec<Span<'static>>,
    content: &RenderContent<'_>,
    gutter_w: usize,
    gutter_style: Style,
) {
    let gutter = single_source_line(content.source)
        .map(|line| format!("{line:>gutter_w$}"))
        .unwrap_or_else(|| format!("{:>gutter_w$}", content.gutter.as_ref()));
    spans.push(Span::styled(gutter, gutter_style));
    spans.push(Span::styled(" ", gutter_style));
}

fn push_reserved_gutter(
    spans: &mut Vec<Span<'static>>,
    content: &RenderContent<'_>,
    gutter_w: usize,
    gutter_style: Style,
    side: CommentAnchorSide,
) {
    let gutter = content
        .source
        .line_for_side(side)
        .or_else(|| single_source_line(content.source))
        .map(|line| format!("{line:>gutter_w$}"))
        .unwrap_or_else(|| format!("{:>gutter_w$}", content.gutter.as_ref()));
    let blank = " ".repeat(gutter_w);
    match side {
        CommentAnchorSide::Base => {
            spans.push(Span::styled(gutter, gutter_style));
            spans.push(Span::styled(" ", gutter_style));
            spans.push(Span::styled(blank, gutter_style));
        }
        CommentAnchorSide::Head => {
            spans.push(Span::styled(blank, gutter_style));
            spans.push(Span::styled(" ", gutter_style));
            spans.push(Span::styled(gutter, gutter_style));
        }
    }
    spans.push(Span::styled(" ", gutter_style));
}

fn format_optional_line(line: Option<u32>, gutter_w: usize) -> String {
    line.map(|line| format!("{line:>gutter_w$}"))
        .unwrap_or_else(|| " ".repeat(gutter_w))
}

fn push_reserved_gutter_text(
    text: &mut String,
    source: SourceLocation,
    gutter: &str,
    gutter_w: usize,
    side: CommentAnchorSide,
) {
    let gutter = source
        .line_for_side(side)
        .or_else(|| single_source_line(source))
        .map(|line| format!("{line:>gutter_w$}"))
        .unwrap_or_else(|| format!("{gutter:>gutter_w$}"));
    let blank = " ".repeat(gutter_w);
    match side {
        CommentAnchorSide::Base => {
            text.push_str(&gutter);
            text.push(' ');
            text.push_str(&blank);
        }
        CommentAnchorSide::Head => {
            text.push_str(&blank);
            text.push(' ');
            text.push_str(&gutter);
        }
    }
    text.push(' ');
}

fn render_spacer_line(
    marker: String,
    width: usize,
    styles: &StyleConfig,
    is_cursor: bool,
) -> Line<'static> {
    let base_style = spacer_style(styles, is_cursor);
    let gutter_style = base_style.fg(*styles.diff.gutter_fg);
    let marker_width = marker.chars().count();
    let mut spans = Vec::new();
    spans.push(Span::styled(marker, gutter_style));
    spans.push(Span::styled(
        " ".repeat(width.saturating_sub(marker_width)),
        base_style,
    ));
    Line::from(spans)
}

fn spacer_style(styles: &StyleConfig, is_cursor: bool) -> Style {
    if is_cursor {
        Style::default().bg(*styles.diff.cursor_line_bg)
    } else {
        Style::default()
    }
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
        DiffDocument::Unified(document) => {
            let gutter_w = unified_gutter_width(document);
            (0..document.len())
                .filter_map(|row| document.line(crate::app::document::RowIndex(row)))
                .map(|line| render_line_text(line, show_blame, GutterMode::Unified, gutter_w))
                .collect()
        }
        DiffDocument::Base(document) => {
            let gutter_w = single_gutter_width(document);
            (0..document.len())
                .filter_map(|row| document.line(crate::app::document::RowIndex(row)))
                .map(|line| render_line_text(line, show_blame, GutterMode::ReservedBase, gutter_w))
                .collect()
        }
        DiffDocument::Head(document) => {
            let gutter_w = single_gutter_width(document);
            (0..document.len())
                .filter_map(|row| document.line(crate::app::document::RowIndex(row)))
                .map(|line| render_line_text(line, show_blame, GutterMode::ReservedHead, gutter_w))
                .collect()
        }
        DiffDocument::SideBySide(document) => {
            let base_gutter_w = single_gutter_width(document.base());
            let head_gutter_w = single_gutter_width(document.head());
            (0..document.len())
                .map(|row| {
                    let row = crate::app::document::RowIndex(row);
                    format!(
                        "{}{}{}",
                        document
                            .base()
                            .line(row)
                            .map(|line| {
                                render_line_text(
                                    line,
                                    show_blame,
                                    GutterMode::Single,
                                    base_gutter_w,
                                )
                            })
                            .unwrap_or_default(),
                        SIDE_BY_SIDE_DIVIDER,
                        document
                            .head()
                            .line(row)
                            .map(|line| {
                                render_line_text(
                                    line,
                                    show_blame,
                                    GutterMode::Single,
                                    head_gutter_w,
                                )
                            })
                            .unwrap_or_default()
                    )
                })
                .collect()
        }
    }
}

fn render_line_text(
    line: RenderLine<'_>,
    show_blame: bool,
    gutter_mode: GutterMode,
    gutter_w: usize,
) -> String {
    match line {
        RenderLine::Content(content) => {
            let mut text = String::new();
            if show_blame {
                text.push_str(&format_blame(content.blame));
                text.push(' ');
            }
            match gutter_mode {
                GutterMode::Unified => {
                    text.push_str(&format_optional_line(content.source.base, gutter_w));
                    text.push(' ');
                    text.push_str(&format_optional_line(content.source.head, gutter_w));
                    text.push(' ');
                }
                GutterMode::Single => {
                    let gutter = single_source_line(content.source)
                        .map(|line| format!("{line:>gutter_w$}"))
                        .unwrap_or_else(|| format!("{:>gutter_w$}", content.gutter.as_ref()));
                    text.push_str(&gutter);
                    text.push(' ');
                }
                GutterMode::ReservedBase => {
                    push_reserved_gutter_text(
                        &mut text,
                        content.source,
                        content.gutter.as_ref(),
                        gutter_w,
                        CommentAnchorSide::Base,
                    );
                }
                GutterMode::ReservedHead => {
                    push_reserved_gutter_text(
                        &mut text,
                        content.source,
                        content.gutter.as_ref(),
                        gutter_w,
                        CommentAnchorSide::Head,
                    );
                }
            }
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

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};

    use super::*;
    use crate::app::document::{RenderContent, SourceLocation, TextRun};
    use crate::app::model::CommentMarker;

    fn plain_content(
        source: SourceLocation,
        gutter: &'static str,
        kind: LineKind,
        text: &'static str,
    ) -> RenderLine<'static> {
        RenderLine::Content(RenderContent {
            gutter: Cow::Borrowed(gutter),
            source,
            marker: CommentMarker::none(),
            kind,
            blame: None,
            runs: vec![TextRun {
                text: Cow::Borrowed(text),
                kind: TextRunKind::Plain,
            }],
        })
    }

    fn changed_content(
        source: SourceLocation,
        gutter: &'static str,
        kind: LineKind,
        plain: &'static str,
        changed: &'static str,
    ) -> RenderLine<'static> {
        RenderLine::Content(RenderContent {
            gutter: Cow::Borrowed(gutter),
            source,
            marker: CommentMarker::none(),
            kind,
            blame: None,
            runs: vec![
                TextRun {
                    text: Cow::Borrowed(plain),
                    kind: TextRunKind::Plain,
                },
                TextRun {
                    text: Cow::Borrowed(changed),
                    kind: TextRunKind::Changed,
                },
            ],
        })
    }

    #[test]
    fn unified_render_text_keeps_deletion_numbers_in_base_column() {
        let line = plain_content(
            SourceLocation::paired(Some(437), None),
            "437",
            LineKind::Deletion,
            "deleted",
        );

        let text = render_line_text(line, false, GutterMode::Unified, 3);

        assert_eq!(text, "437       - deleted");
    }

    #[test]
    fn unified_render_text_keeps_addition_numbers_in_head_column() {
        let line = plain_content(
            SourceLocation::paired(None, Some(441)),
            "441",
            LineKind::Addition,
            "added",
        );

        let text = render_line_text(line, false, GutterMode::Unified, 3);

        assert_eq!(text, "    441   + added");
    }

    #[test]
    fn changed_runs_use_line_emphasis_background() {
        let styles = StyleConfig::default();
        let RenderLine::Content(content) = changed_content(
            SourceLocation::paired(None, Some(441)),
            "441",
            LineKind::Addition,
            "hello ",
            "earth",
        ) else {
            panic!("expected content line");
        };

        let line = render_content_line(content, &styles, 3, GutterMode::Unified, 80, false, false);
        let changed = line
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "earth")
            .expect("changed span");

        assert_eq!(changed.style.bg, Some(*styles.diff.addition_emphasis_bg));
    }

    #[test]
    fn reserved_head_render_text_keeps_head_number_in_head_column() {
        let line = plain_content(
            SourceLocation::single(CommentAnchorSide::Head, 423),
            "423",
            LineKind::Context,
            "head",
        );

        let text = render_line_text(line, false, GutterMode::ReservedHead, 3);

        assert_eq!(text, "    423     head");
    }

    #[test]
    fn reserved_base_render_text_keeps_base_number_in_base_column() {
        let line = plain_content(
            SourceLocation::single(CommentAnchorSide::Base, 419),
            "419",
            LineKind::Context,
            "base",
        );

        let text = render_line_text(line, false, GutterMode::ReservedBase, 3);

        assert_eq!(text, "419         base");
    }

    #[test]
    fn base_and_head_layouts_reserve_unified_gutter_width() {
        let head_document = Document::new(
            vec![DocumentRow::Content(crate::app::document::ContentRow {
                gutter: crate::app::document::Gutter {
                    text: "423".to_string(),
                },
                kind: LineKind::Context,
                text: "line".to_string(),
                blame: None,
                source: SourceLocation::single(CommentAnchorSide::Head, 423),
                changed_spans: Vec::new(),
            })],
            Vec::new(),
        );
        let base_document = Document::new(
            vec![DocumentRow::Content(crate::app::document::ContentRow {
                gutter: crate::app::document::Gutter {
                    text: "419".to_string(),
                },
                kind: LineKind::Context,
                text: "line".to_string(),
                blame: None,
                source: SourceLocation::single(CommentAnchorSide::Base, 419),
                changed_spans: Vec::new(),
            })],
            Vec::new(),
        );

        let head_layout =
            DocumentLayout::new(&DiffDocument::Head(head_document.clone()), false, 80);
        let base_layout =
            DocumentLayout::new(&DiffDocument::Base(base_document.clone()), false, 80);
        let unified_layout = DocumentLayout::new(&DiffDocument::Unified(head_document), false, 80);

        assert_eq!(
            head_layout.content_start_col,
            unified_layout.content_start_col
        );
        assert_eq!(
            base_layout.content_start_col,
            unified_layout.content_start_col
        );
    }

    #[test]
    fn cursor_overlay_targets_content_column_after_fixed_gutter() {
        let line = Line::from(vec![Span::styled(
            "012345",
            Style::default().fg(Color::Red).bg(Color::Blue),
        )]);

        let line = with_column_cursor(line, 2, 1, true);

        assert_eq!(line.spans[0].content.as_ref(), "012");
        assert_eq!(line.spans[1].content.as_ref(), "3");
        assert_eq!(line.spans[1].style.fg, Some(Color::Blue));
        assert_eq!(line.spans[1].style.bg, Some(Color::Red));
        assert_eq!(line.spans[2].content.as_ref(), "45");
    }

    #[test]
    fn cursor_overlay_renders_on_padded_spacer_rows() {
        let styles = StyleConfig::default();
        let line = render_spacer_line(" ".to_string(), 8, &styles, true);

        let line = with_column_cursor(line, 4, 0, true);

        assert_eq!(line.spans[0].content.as_ref(), " ");
        assert_eq!(line.spans[0].style.bg, Some(*styles.diff.cursor_line_bg));
        assert_eq!(line.spans[1].content.as_ref(), "   ");
        assert_eq!(line.spans[1].style.bg, Some(*styles.diff.cursor_line_bg));
        assert_eq!(line.spans[2].content.as_ref(), " ");
        assert_eq!(line.spans[2].style.fg, Some(*styles.diff.cursor_line_bg));
        assert_eq!(line.spans[2].style.bg, Some(Color::White));
        assert_eq!(line.spans[3].content.as_ref(), "   ");
        assert_eq!(line.spans[3].style.bg, Some(*styles.diff.cursor_line_bg));
    }

    #[test]
    fn side_by_side_cursor_row_styles_empty_spacer_side() {
        let styles = StyleConfig::default();
        let inner_w = 41;
        let layout = DocumentLayout {
            gutter_w: 3,
            base_gutter_w: 3,
            head_gutter_w: 3,
            gutter_cols: 0,
            content_start_col: 0,
        };
        let head = plain_content(
            SourceLocation::single(CommentAnchorSide::Head, 441),
            "441",
            LineKind::Addition,
            "added",
        );

        let line = render_side_by_side_line(
            Some(RenderLine::Spacer {
                marker: CommentMarker::none(),
            }),
            Some(head),
            &styles,
            &layout,
            inner_w,
            false,
            true,
        );
        let side_w = side_width(inner_w);
        let divider_w = SIDE_BY_SIDE_DIVIDER.chars().count();
        let mut offset = 0;
        for span in &line.spans {
            let span_len = span.content.chars().count();
            if offset < side_w + divider_w {
                assert_eq!(span.style.bg, Some(*styles.diff.cursor_line_bg));
            }
            offset += span_len;
        }
    }

    #[test]
    fn side_by_side_line_clips_each_side_before_joining() {
        let styles = StyleConfig::default();
        let inner_w = 41;
        let side_w = side_width(inner_w);
        let layout = DocumentLayout {
            gutter_w: 3,
            base_gutter_w: 3,
            head_gutter_w: 3,
            gutter_cols: 0,
            content_start_col: 0,
        };
        let base = plain_content(
            SourceLocation::single(CommentAnchorSide::Base, 420),
            "420",
            LineKind::Context,
            "left content that is far too long for the left side",
        );
        let head = plain_content(
            SourceLocation::single(CommentAnchorSide::Head, 424),
            "424",
            LineKind::Context,
            "right",
        );

        let line = render_side_by_side_line(
            Some(base),
            Some(head),
            &styles,
            &layout,
            inner_w,
            false,
            false,
        );
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();

        let divider_w = SIDE_BY_SIDE_DIVIDER.chars().count();
        assert_eq!(text.chars().count(), side_w * 2 + divider_w);
        assert_eq!(
            text.chars()
                .skip(side_w)
                .take(divider_w)
                .collect::<String>(),
            SIDE_BY_SIDE_DIVIDER
        );
        assert!(text.ends_with("right      "));
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
