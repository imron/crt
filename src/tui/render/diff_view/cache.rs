use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use ratatui::text::Line;

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
        diff_hash: diff.diff_hash.clone().unwrap_or_default(),
    }
}
