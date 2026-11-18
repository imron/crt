use std::collections::BTreeMap;

use crate::app::model::CommentAttachment;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentMarkerSet {
    markers_by_line: BTreeMap<u32, MarkerCandidate>,
    current_comment: Option<CurrentComment>,
    width: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentMarker {
    text: String,
    current: bool,
}

impl CommentMarker {
    pub fn blank(width: usize) -> Self {
        Self {
            text: " ".repeat(width),
            current: false,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn width(&self) -> usize {
        self.text.chars().count()
    }

    pub fn is_current(&self) -> bool {
        self.current
    }
}

impl CommentMarkerSet {
    pub fn new(comments: &[CommentAttachment], current_line: Option<u32>) -> Self {
        let mut marker_candidates: BTreeMap<u32, MarkerCandidate> = BTreeMap::new();

        for comment in comments {
            let start = comment.line_start.max(1) as u32;
            let end = comment.line_end.max(comment.line_start).max(1) as u32;
            for line in start..=end {
                let candidate = MarkerCandidate::new(comment, start, end, line);
                marker_candidates
                    .entry(line)
                    .and_modify(|existing| {
                        if candidate.is_preferred_to(existing) {
                            *existing = candidate;
                        }
                    })
                    .or_insert(candidate);
            }
        }

        let markers_by_line = marker_candidates;
        let width = if markers_by_line.is_empty() { 0 } else { 1 };
        let current_comment = current_comment(comments, current_line);

        Self {
            markers_by_line,
            current_comment,
            width,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn marker_for_line(&self, line: Option<u32>) -> CommentMarker {
        let Some(line) = line else {
            return CommentMarker::blank(self.width);
        };
        let (marker, current) = if let Some(current_comment) = self.current_comment {
            if current_comment.contains(line)
                && self.line_uses_current_marker(line, current_comment)
            {
                (String::from(current_comment.glyph_for_line(line)), true)
            } else {
                self.inactive_marker_for_line(line)
            }
        } else {
            self.inactive_marker_for_line(line)
        };
        let pad = self.width.saturating_sub(marker.chars().count());
        let mut marker = marker;
        marker.push_str(&" ".repeat(pad));
        CommentMarker {
            text: marker,
            current,
        }
    }

    fn inactive_marker_for_line(&self, line: u32) -> (String, bool) {
        self.markers_by_line
            .get(&line)
            .copied()
            .map(|marker| (String::from(marker.glyph()), false))
            .unwrap_or_else(|| (String::new(), false))
    }

    fn line_uses_current_marker(&self, line: u32, current_comment: CurrentComment) -> bool {
        if current_comment.is_boundary(line) {
            return true;
        }

        !self
            .markers_by_line
            .get(&line)
            .is_some_and(|marker| marker.kind.is_boundary_marker())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CurrentComment {
    id: i64,
    start: u32,
    end: u32,
    resolved: bool,
}

impl CurrentComment {
    fn contains(self, line: u32) -> bool {
        line >= self.start && line <= self.end
    }

    fn is_boundary(self, line: u32) -> bool {
        line == self.start || line == self.end
    }

    fn glyph_for_line(self, line: u32) -> char {
        if self.start == self.end || self.is_boundary(line) {
            if self.resolved { '○' } else { '●' }
        } else {
            '┃'
        }
    }
}

fn current_comment(
    comments: &[CommentAttachment],
    current_line: Option<u32>,
) -> Option<CurrentComment> {
    let current_line = current_line?;
    comments
        .iter()
        .filter_map(|comment| {
            let start = comment.line_start.max(1) as u32;
            let end = comment.line_end.max(comment.line_start).max(1) as u32;
            if current_line < start || current_line > end {
                return None;
            }
            Some(CurrentComment {
                id: comment.id,
                start,
                end,
                resolved: comment.resolved,
            })
        })
        .min_by_key(|comment| {
            (
                comment.end.saturating_sub(comment.start),
                std::cmp::Reverse(comment.id),
            )
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MarkerCandidate {
    kind: MarkerKind,
    resolved: bool,
    range_len: u32,
    comment_id: i64,
}

impl MarkerCandidate {
    fn new(comment: &CommentAttachment, start: u32, end: u32, line: u32) -> Self {
        let kind = if start == end {
            MarkerKind::SingleLine
        } else if line == start || line == end {
            MarkerKind::MultilineBoundary
        } else {
            MarkerKind::Continuation
        };

        Self {
            kind,
            resolved: comment.resolved,
            range_len: end.saturating_sub(start).saturating_add(1),
            comment_id: comment.id,
        }
    }

    fn glyph(self) -> char {
        match self.kind {
            MarkerKind::Continuation => '┃',
            MarkerKind::MultilineBoundary | MarkerKind::SingleLine => {
                if self.resolved {
                    '○'
                } else {
                    '●'
                }
            }
        }
    }

    fn is_preferred_to(self, other: &Self) -> bool {
        self.kind.priority() > other.kind.priority()
            || (self.kind.priority() == other.kind.priority()
                && (self.range_len < other.range_len
                    || (self.range_len == other.range_len && self.comment_id > other.comment_id)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkerKind {
    Continuation,
    SingleLine,
    MultilineBoundary,
}

impl MarkerKind {
    fn priority(self) -> u8 {
        match self {
            MarkerKind::Continuation => 0,
            MarkerKind::SingleLine => 1,
            MarkerKind::MultilineBoundary => 2,
        }
    }

    fn is_boundary_marker(self) -> bool {
        matches!(self, Self::SingleLine | Self::MultilineBoundary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_types::AnchorStatus;

    fn comment(id: i64, start: i64, end: i64, resolved: bool) -> CommentAttachment {
        CommentAttachment {
            id,
            line_start: start,
            line_end: end,
            resolved,
            anchor_status: AnchorStatus::Anchored,
        }
    }

    fn assert_marker(markers: &CommentMarkerSet, line: u32, text: &str, current: bool) {
        let marker = markers.marker_for_line(Some(line));
        assert_eq!(marker.text(), text, "line {line}");
        assert_eq!(marker.is_current(), current, "line {line}");
    }

    #[test]
    fn markers_show_single_line_resolution_state() {
        let markers =
            CommentMarkerSet::new(&[comment(1, 3, 3, false), comment(2, 5, 5, true)], None);

        assert_eq!(markers.width(), 1);
        assert_eq!(markers.marker_for_line(Some(3)).text(), "●");
        assert_eq!(markers.marker_for_line(Some(5)).text(), "○");
        assert_eq!(markers.marker_for_line(Some(4)).text(), " ");
    }

    #[test]
    fn markers_connect_multiline_ranges() {
        let markers = CommentMarkerSet::new(&[comment(1, 3, 5, false)], None);

        assert_eq!(markers.marker_for_line(Some(3)).text(), "●");
        assert_eq!(markers.marker_for_line(Some(4)).text(), "┃");
        assert_eq!(markers.marker_for_line(Some(5)).text(), "●");
    }

    #[test]
    fn markers_merge_nested_comments_to_one_column() {
        let markers =
            CommentMarkerSet::new(&[comment(1, 3, 5, false), comment(2, 4, 4, true)], None);

        assert_eq!(markers.width(), 1);
        assert_eq!(markers.marker_for_line(Some(3)).text(), "●");
        assert_eq!(markers.marker_for_line(Some(4)).text(), "○");
        assert_eq!(markers.marker_for_line(Some(5)).text(), "●");
    }

    #[test]
    fn multiline_boundary_wins_over_nested_single_line_marker() {
        let markers = CommentMarkerSet::new(
            &[
                comment(1, 31, 41, false),
                comment(2, 36, 40, true),
                comment(3, 36, 36, false),
            ],
            None,
        );

        assert_eq!(markers.width(), 1);
        assert_eq!(markers.marker_for_line(Some(36)).text(), "○");
        assert_eq!(markers.marker_for_line(Some(37)).text(), "┃");
        assert_eq!(markers.marker_for_line(Some(40)).text(), "○");
    }

    #[test]
    fn current_comment_highlights_innermost_comment_range() {
        let markers = CommentMarkerSet::new(
            &[
                comment(1, 31, 41, false),
                comment(2, 36, 40, true),
                comment(3, 36, 36, false),
            ],
            Some(38),
        );

        let opening = markers.marker_for_line(Some(36));
        let middle = markers.marker_for_line(Some(38));
        let outer_end = markers.marker_for_line(Some(41));

        assert_eq!(opening.text(), "○");
        assert!(opening.is_current());
        assert_eq!(middle.text(), "┃");
        assert!(middle.is_current());
        assert_eq!(outer_end.text(), "●");
        assert!(!outer_end.is_current());
    }

    #[test]
    fn current_comment_highlights_only_the_selected_nested_comment() {
        let markers = CommentMarkerSet::new(
            &[
                comment(1, 31, 41, false),
                comment(2, 36, 40, true),
                comment(3, 36, 36, false),
            ],
            Some(36),
        );

        let outer_start = markers.marker_for_line(Some(31));
        let nested_single_line = markers.marker_for_line(Some(36));
        let nested_multiline_middle = markers.marker_for_line(Some(38));

        assert_eq!(outer_start.text(), "●");
        assert!(!outer_start.is_current());
        assert_eq!(nested_single_line.text(), "●");
        assert!(nested_single_line.is_current());
        assert_eq!(nested_multiline_middle.text(), "┃");
        assert!(!nested_multiline_middle.is_current());
    }

    #[test]
    fn current_outer_comment_leaves_nested_boundaries_inactive() {
        let markers = CommentMarkerSet::new(
            &[
                comment(1, 31, 41, false),
                comment(2, 36, 40, true),
                comment(3, 37, 37, false),
            ],
            Some(32),
        );

        let outer_start = markers.marker_for_line(Some(31));
        let outer_join = markers.marker_for_line(Some(34));
        let nested_start = markers.marker_for_line(Some(36));
        let nested_single_line = markers.marker_for_line(Some(37));
        let nested_end = markers.marker_for_line(Some(40));
        let outer_end = markers.marker_for_line(Some(41));

        assert_eq!(outer_start.text(), "●");
        assert!(outer_start.is_current());
        assert_eq!(outer_join.text(), "┃");
        assert!(outer_join.is_current());
        assert_eq!(nested_start.text(), "○");
        assert!(!nested_start.is_current());
        assert_eq!(nested_single_line.text(), "●");
        assert!(!nested_single_line.is_current());
        assert_eq!(nested_end.text(), "○");
        assert!(!nested_end.is_current());
        assert_eq!(outer_end.text(), "●");
        assert!(outer_end.is_current());
    }

    #[test]
    fn current_resolved_outer_comment_leaves_unresolved_nested_boundaries_inactive() {
        let markers = CommentMarkerSet::new(
            &[
                comment(1, 10, 20, true),
                comment(2, 13, 18, false),
                comment(3, 15, 15, false),
            ],
            Some(11),
        );

        assert_marker(&markers, 10, "○", true);
        assert_marker(&markers, 12, "┃", true);
        assert_marker(&markers, 13, "●", false);
        assert_marker(&markers, 14, "┃", true);
        assert_marker(&markers, 15, "●", false);
        assert_marker(&markers, 18, "●", false);
        assert_marker(&markers, 19, "┃", true);
        assert_marker(&markers, 20, "○", true);
    }

    #[test]
    fn current_unresolved_middle_comment_preserves_outer_and_inner_boundaries() {
        let markers = CommentMarkerSet::new(
            &[
                comment(1, 10, 30, true),
                comment(2, 14, 24, false),
                comment(3, 18, 22, true),
                comment(4, 20, 20, true),
            ],
            Some(16),
        );

        assert_marker(&markers, 10, "○", false);
        assert_marker(&markers, 14, "●", true);
        assert_marker(&markers, 16, "┃", true);
        assert_marker(&markers, 18, "○", false);
        assert_marker(&markers, 20, "○", false);
        assert_marker(&markers, 22, "○", false);
        assert_marker(&markers, 24, "●", true);
        assert_marker(&markers, 30, "○", false);
    }

    #[test]
    fn current_comment_boundary_overrides_shared_nested_boundary() {
        let markers = CommentMarkerSet::new(
            &[
                comment(1, 10, 20, false),
                comment(2, 10, 15, true),
                comment(3, 15, 20, true),
            ],
            Some(11),
        );

        assert_marker(&markers, 10, "○", true);
        assert_marker(&markers, 12, "┃", true);
        assert_marker(&markers, 15, "○", true);
        assert_marker(&markers, 20, "○", false);
    }

    #[test]
    fn identical_ranges_use_latest_comment_as_current_tiebreaker() {
        let markers = CommentMarkerSet::new(
            &[comment(1, 10, 12, false), comment(2, 10, 12, true)],
            Some(11),
        );

        assert_marker(&markers, 10, "○", true);
        assert_marker(&markers, 11, "┃", true);
        assert_marker(&markers, 12, "○", true);
    }

    #[test]
    fn inactive_identical_ranges_use_latest_comment_tiebreaker() {
        let markers =
            CommentMarkerSet::new(&[comment(1, 10, 10, false), comment(2, 10, 10, true)], None);

        assert_marker(&markers, 10, "○", false);
    }
}
