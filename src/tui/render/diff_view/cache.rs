use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use ratatui::text::Line;

use crate::app::model::{CommentAttachment, CommentMarkerSet};
use crate::review_types::{ContentMode, RenderVariant};

// ---------------------------------------------------------------------------
// Diff line cache — avoids rebuilding all Line<'static> every frame
// ---------------------------------------------------------------------------

/// Identity key for cached diff content. When this key matches the previous
/// frame, we can reuse the cached `Line` vectors instead of running the
/// (expensive) word-diff and line-building code again.
#[derive(PartialEq, Eq)]
pub struct DiffCacheKey {
    selected_file: Option<String>,
    content_mode: ContentMode,
    render_variant: RenderVariant,
    show_blame: bool,
    reviewed_diff_expanded: bool,
    inner_w: usize,
    ignore_whitespace: bool,
    diff_algorithm: crate::config::DiffAlgorithm,
    head_content_id: Option<(u64, usize)>,
    base_content_id: Option<(u64, usize)>,
    head_blame_len: usize,
    base_blame_len: usize,
    comments: Vec<CommentAttachment>,
    comment_markers: CommentMarkerSet,
    /// Stable hash of the diff content (from DiffContent::diff_hash).
    diff_hash: String,
}

/// Cached output of `build_content()`.
pub struct DiffCache {
    pub key: DiffCacheKey,
    pub lines: Vec<Line<'static>>,
    pub hunk_starts: Vec<usize>,
    pub hunk_ends: Vec<usize>,
    pub hunk_first_changes: Vec<usize>,
    pub gutter_w: usize,
    pub comment_marker_w: usize,
    pub rendered_text: Vec<String>,
}

fn string_signature(s: &Option<String>) -> Option<(u64, usize)> {
    s.as_ref().map(|s| {
        let mut hasher = DefaultHasher::new();
        s.hash(&mut hasher);
        (hasher.finish(), s.len())
    })
}

pub fn build_key(diff: &crate::app::model::DiffPanel, inner_w: usize) -> DiffCacheKey {
    DiffCacheKey {
        selected_file: diff.file_id.clone(),
        content_mode: diff.content_mode,
        render_variant: diff.render_variant,
        show_blame: diff.show_blame,
        reviewed_diff_expanded: diff.reviewed_diff_expanded,
        inner_w,
        ignore_whitespace: diff.ignore_whitespace,
        diff_algorithm: diff.diff_algorithm,
        head_content_id: string_signature(&diff.head_content),
        base_content_id: string_signature(&diff.base_content),
        head_blame_len: diff.head_blame.len(),
        base_blame_len: diff.base_blame.len(),
        comments: diff.comments.clone(),
        comment_markers: diff.comment_markers.clone(),
        diff_hash: diff.diff_hash.clone().unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::{DiffPanel, ReviewStatus};
    use crate::config::DiffAlgorithm;
    use crate::core::TextAnchor;
    use crate::review_types::AnchorStatus;

    fn diff_panel(cursor_line: usize, render_variant: RenderVariant) -> DiffPanel {
        let comments = vec![CommentAttachment {
            id: 1,
            line_start: 2,
            line_end: 2,
            resolved: false,
            anchor_status: AnchorStatus::Anchored,
            side_ranges: Vec::new(),
        }];
        let current_line = match render_variant {
            RenderVariant::HeadVersion => Some(cursor_line.saturating_add(1) as u32),
            RenderVariant::BaseVersion => None,
            _ => None,
        };
        DiffPanel {
            selected_file_index: Some(0),
            file_id: Some("file.rs".to_string()),
            path: Some("file.rs".to_string()),
            review_status: Some(ReviewStatus::Unreviewed),
            content_mode: ContentMode::FullFile,
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
            inline_rows: Default::default(),
            full_file_head_rows: Default::default(),
            full_file_base_rows: Default::default(),
            side_by_side_rows: Default::default(),
            head_content: Some("one\ntwo\nthree\n".to_string()),
            base_content: Some("one\ntwo\nthree\n".to_string()),
            head_blame: vec![],
            base_blame: vec![],
            scroll: 0,
            cursor: TextAnchor {
                line: cursor_line,
                column: 0,
            },
            search_query: None,
            search_highlights: vec![],
            current_search_highlight: None,
            visual_selection: None,
            pending_comment_anchor: None,
            comment_markers: CommentMarkerSet::new(&comments, current_line),
            comments,
        }
    }

    #[test]
    fn cache_key_changes_when_current_comment_line_changes() {
        let before_comment = diff_panel(0, RenderVariant::HeadVersion);
        let on_comment = diff_panel(1, RenderVariant::HeadVersion);

        assert!(build_key(&before_comment, 80) != build_key(&on_comment, 80));
    }

    #[test]
    fn cache_key_ignores_cursor_line_for_base_comment_markers() {
        let first_line = diff_panel(0, RenderVariant::BaseVersion);
        let second_line = diff_panel(1, RenderVariant::BaseVersion);

        assert!(build_key(&first_line, 80) == build_key(&second_line, 80));
    }
}
