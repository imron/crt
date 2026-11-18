use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::super::super::state::TuiState;
use super::comment_markers::CommentMarkerSet;
use super::full_file::{build_full_file_base, build_full_file_head};
use super::inline::build_inline_diff;
use super::side_by_side::build_side_by_side_diff;
use crate::app::model::{BlameLine, DiffPanel, ReviewStatus};
use crate::config::StyleConfig;
use crate::review_types::{ContentMode, RenderVariant};

// ---------------------------------------------------------------------------
// Content dispatch
// ---------------------------------------------------------------------------

/// Return type for content builders: lines, hunk start rows, hunk end rows.
pub struct BuiltContent {
    pub lines: Vec<Line<'static>>,
    pub hunk_starts: Vec<usize>,
    pub hunk_ends: Vec<usize>,
    /// Row of the first actual change (+/-) in each hunk.
    pub hunk_first_changes: Vec<usize>,
    /// Width of one line-number gutter column (in characters).
    pub gutter_w: usize,
    /// Width of the comment marker gutter column.
    pub comment_marker_w: usize,
}

pub fn build_content(
    diff: &DiffPanel,
    styles: &StyleConfig,
    inner_w: usize,
) -> (
    String,
    Vec<Line<'static>>,
    Vec<usize>,
    Vec<usize>,
    Vec<usize>,
    usize,
    usize,
) {
    let ds = &styles.diff;

    let path = match &diff.path {
        None => {
            return (
                " Diff ".to_string(),
                vec![Line::from("No files changed.")],
                vec![],
                vec![],
                vec![],
                0,
                0,
            );
        }
        Some(path) => path,
    };

    // Reviewed file summary mode.
    if matches!(diff.review_status, Some(ReviewStatus::Reviewed { .. }))
        && !diff.reviewed_diff_expanded
    {
        let at = match &diff.review_status {
            Some(ReviewStatus::Reviewed { at, .. }) => at.as_str(),
            _ => "",
        };
        let title = format!(" {path} ");
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  {path} \u{2014} reviewed at {at}"),
                Style::default()
                    .fg(*ds.reviewed_fg)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Press Enter to view diff.",
                Style::default().fg(*ds.placeholder_fg),
            )),
        ];
        return (title, lines, vec![], vec![], vec![], 0, 0);
    }

    // Binary file.
    if diff.is_binary {
        let title = format!(" {path} ");
        return (
            title,
            vec![Line::from(Span::styled(
                "  Binary file",
                Style::default().fg(*ds.placeholder_fg),
            ))],
            vec![],
            vec![],
            vec![],
            0,
            0,
        );
    }

    let empty_blame: Vec<BlameLine> = Vec::new();
    let head_blame: &[BlameLine] = if diff.show_blame {
        &diff.head_blame
    } else {
        &empty_blame
    };
    let base_blame: &[BlameLine] = if diff.show_blame {
        &diff.base_blame
    } else {
        &empty_blame
    };

    let default_bg = *styles.bg;
    let current_comment_fg = *styles.files.selected_fg;
    let comment_markers = CommentMarkerSet::new(&diff.comments, current_comment_line(diff));
    let no_comment_markers = CommentMarkerSet::new(&[], None);

    let built = match (diff.content_mode, diff.render_variant) {
        (ContentMode::Diff, RenderVariant::SideBySide) => build_side_by_side_diff(
            ds,
            default_bg,
            &diff.hunks,
            diff.head_content.as_deref(),
            head_blame,
            base_blame,
            &comment_markers,
            current_comment_fg,
            inner_w,
        ),
        (ContentMode::Diff, _) => build_inline_diff(
            ds,
            default_bg,
            &diff.hunks,
            diff.head_content.as_deref(),
            head_blame,
            base_blame,
            &comment_markers,
            current_comment_fg,
            inner_w,
        ),
        (ContentMode::FullFile, RenderVariant::HeadVersion) => build_full_file_head(
            ds,
            default_bg,
            &diff.hunks,
            diff.head_content.as_deref(),
            head_blame,
            &comment_markers,
            current_comment_fg,
            inner_w,
        ),
        (ContentMode::FullFile, RenderVariant::BaseVersion) => build_full_file_base(
            ds,
            default_bg,
            &diff.hunks,
            diff.base_content.as_deref(),
            base_blame,
            &no_comment_markers,
            current_comment_fg,
            inner_w,
        ),
        _ => BuiltContent {
            lines: vec![Line::from("  (unknown variant)")],
            hunk_starts: vec![],
            hunk_ends: vec![],
            hunk_first_changes: vec![],
            gutter_w: 0,
            comment_marker_w: 0,
        },
    };

    (
        " Diff ".to_string(),
        built.lines,
        built.hunk_starts,
        built.hunk_ends,
        built.hunk_first_changes,
        built.gutter_w,
        built.comment_marker_w,
    )
}

fn current_comment_line(diff: &DiffPanel) -> Option<u32> {
    match diff.content_mode {
        ContentMode::FullFile => match diff.render_variant {
            RenderVariant::HeadVersion => Some(diff.cursor.line.saturating_add(1) as u32),
            RenderVariant::BaseVersion => None,
            _ => None,
        },
        ContentMode::Diff => diff_new_line_at_row(diff, diff.cursor.line),
    }
}

fn diff_new_line_at_row(diff: &DiffPanel, row: usize) -> Option<u32> {
    let head_lines = diff
        .head_content
        .as_deref()
        .map(|content| content.lines().count())
        .unwrap_or(0);
    if diff.hunks.is_empty() {
        return (row < head_lines).then_some(row.saturating_add(1) as u32);
    }

    let mut display_row = 0usize;
    let mut new_cursor = 1u32;

    for hunk in &diff.hunks {
        while new_cursor < hunk.new_start && (new_cursor as usize) <= head_lines {
            if display_row == row {
                return Some(new_cursor);
            }
            display_row = display_row.saturating_add(1);
            new_cursor = new_cursor.saturating_add(1);
        }

        for line in &hunk.lines {
            if display_row == row {
                return line.new_lineno;
            }
            display_row = display_row.saturating_add(1);
            if line.new_lineno.is_some() {
                new_cursor = new_cursor.saturating_add(1);
            }
        }
    }

    while (new_cursor as usize) <= head_lines {
        if display_row == row {
            return Some(new_cursor);
        }
        display_row = display_row.saturating_add(1);
        new_cursor = new_cursor.saturating_add(1);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::CommentAttachment;
    use crate::config::DiffAlgorithm;
    use crate::core::TextAnchor;
    use crate::review_types::AnchorStatus;

    fn diff_panel(content_mode: ContentMode, render_variant: RenderVariant) -> DiffPanel {
        DiffPanel {
            selected_file_index: Some(0),
            file_id: Some("file.rs".to_string()),
            path: Some("file.rs".to_string()),
            review_status: None,
            content_mode,
            render_variant,
            diff_algorithm: DiffAlgorithm::Myers,
            default_diff_algorithm: DiffAlgorithm::Myers,
            ignore_whitespace: false,
            show_blame: false,
            show_merge_base: true,
            reviewed_diff_expanded: false,
            is_binary: false,
            diff_hash: Some("diff".to_string()),
            hunks: vec![],
            head_content: Some("one\ntwo\nthree\n".to_string()),
            base_content: Some("one\ntwo\nthree\n".to_string()),
            head_blame: vec![],
            base_blame: vec![],
            scroll: 0,
            cursor: TextAnchor { line: 1, column: 0 },
            search_query: None,
            search_highlights: vec![],
            current_search_highlight: None,
            visual_selection: None,
            pending_comment_anchor: None,
            comments: vec![CommentAttachment {
                id: 1,
                line_start: 2,
                line_end: 2,
                resolved: false,
                anchor_status: AnchorStatus::Anchored,
            }],
        }
    }

    #[test]
    fn full_file_base_view_hides_comment_markers() {
        let styles = StyleConfig::default();
        let diff = diff_panel(ContentMode::FullFile, RenderVariant::BaseVersion);

        let (_, _, _, _, _, _, comment_marker_w) = build_content(&diff, &styles, 80);

        assert_eq!(comment_marker_w, 0);
    }

    #[test]
    fn full_file_head_view_keeps_comment_markers() {
        let styles = StyleConfig::default();
        let diff = diff_panel(ContentMode::FullFile, RenderVariant::HeadVersion);

        let (_, _, _, _, _, _, comment_marker_w) = build_content(&diff, &styles, 80);

        assert_eq!(comment_marker_w, 1);
    }
}

/// Build the border title with hunk navigation context.
pub fn build_title(diff: &DiffPanel, tui_state: &TuiState, total_hunks: usize) -> String {
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

    // Show diff base indicator for reviewed files when not using merge base.
    let base_label = if !diff.show_merge_base {
        let has_reviewed_commit = match &diff.review_status {
            Some(ReviewStatus::Reviewed {
                reviewed_commit: Some(_),
                ..
            })
            | Some(ReviewStatus::Changed {
                reviewed_commit: Some(_),
                ..
            }) => true,
            _ => false,
        };
        if has_reviewed_commit {
            " [since review]"
        } else {
            ""
        }
    } else {
        ""
    };

    let hunk_info = if total_hunks == 0 {
        String::new()
    } else if let Some(idx) = tui_state.current_hunk_index_at(diff.cursor.line) {
        format!(" \u{2014} {}/{total_hunks}", idx + 1)
    } else {
        String::new()
    };

    format!(" {path}{mode_label}{ws_label}{base_label}{hunk_info} ")
}
