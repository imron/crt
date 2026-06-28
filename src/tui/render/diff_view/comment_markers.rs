use std::collections::BTreeMap;

use crate::app::model::CommentAttachment;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentMarkerSet {
    markers_by_line: BTreeMap<u32, char>,
    current_markers_by_line: BTreeMap<u32, char>,
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

        let markers_by_line: BTreeMap<u32, char> = marker_candidates
            .into_iter()
            .map(|(line, candidate)| (line, candidate.glyph()))
            .collect();
        let width = if markers_by_line.is_empty() { 0 } else { 1 };
        let current_markers_by_line = current_comment(comments, current_line)
            .map(current_comment_markers)
            .unwrap_or_default();

        Self {
            markers_by_line,
            current_markers_by_line,
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
        let (marker, current) = self
            .current_markers_by_line
            .get(&line)
            .copied()
            .map(|marker| (marker, true))
            .or_else(|| {
                self.markers_by_line
                    .get(&line)
                    .copied()
                    .map(|marker| (marker, false))
            })
            .map(|(marker, current)| (String::from(marker), current))
            .unwrap_or_else(|| (String::new(), false));
        let pad = self.width.saturating_sub(marker.chars().count());
        let mut marker = marker;
        marker.push_str(&" ".repeat(pad));
        CommentMarker {
            text: marker,
            current,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CurrentComment {
    id: i64,
    start: u32,
    end: u32,
    resolved: bool,
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

fn current_comment_markers(comment: CurrentComment) -> BTreeMap<u32, char> {
    let mut markers = BTreeMap::new();
    for line in comment.start..=comment.end {
        let kind = if comment.start == comment.end {
            MarkerKind::SingleLine
        } else if line == comment.start || line == comment.end {
            MarkerKind::MultilineBoundary
        } else {
            MarkerKind::Continuation
        };
        let glyph = match kind {
            MarkerKind::Continuation => '┃',
            MarkerKind::MultilineBoundary | MarkerKind::SingleLine => {
                if comment.resolved {
                    '○'
                } else {
                    '●'
                }
            }
        };
        markers.insert(line, glyph);
    }
    markers
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
}
