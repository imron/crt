use std::collections::BTreeMap;

use crate::app::model::CommentAttachment;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentMarkerSet {
    markers_by_line: BTreeMap<u32, char>,
    width: usize,
}

impl CommentMarkerSet {
    pub fn new(comments: &[CommentAttachment]) -> Self {
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

        Self {
            markers_by_line,
            width,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn marker_for_line(&self, line: Option<u32>) -> String {
        let Some(line) = line else {
            return " ".repeat(self.width);
        };
        let marker: String = self
            .markers_by_line
            .get(&line)
            .copied()
            .map(String::from)
            .unwrap_or_default();
        let pad = self.width.saturating_sub(marker.chars().count());
        let mut marker = marker;
        marker.push_str(&" ".repeat(pad));
        marker
    }
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
        let markers = CommentMarkerSet::new(&[comment(1, 3, 3, false), comment(2, 5, 5, true)]);

        assert_eq!(markers.width(), 1);
        assert_eq!(markers.marker_for_line(Some(3)), "●");
        assert_eq!(markers.marker_for_line(Some(5)), "○");
        assert_eq!(markers.marker_for_line(Some(4)), " ");
    }

    #[test]
    fn markers_connect_multiline_ranges() {
        let markers = CommentMarkerSet::new(&[comment(1, 3, 5, false)]);

        assert_eq!(markers.marker_for_line(Some(3)), "●");
        assert_eq!(markers.marker_for_line(Some(4)), "┃");
        assert_eq!(markers.marker_for_line(Some(5)), "●");
    }

    #[test]
    fn markers_merge_nested_comments_to_one_column() {
        let markers = CommentMarkerSet::new(&[comment(1, 3, 5, false), comment(2, 4, 4, true)]);

        assert_eq!(markers.width(), 1);
        assert_eq!(markers.marker_for_line(Some(3)), "●");
        assert_eq!(markers.marker_for_line(Some(4)), "○");
        assert_eq!(markers.marker_for_line(Some(5)), "●");
    }

    #[test]
    fn multiline_boundary_wins_over_nested_single_line_marker() {
        let markers = CommentMarkerSet::new(&[
            comment(1, 31, 41, false),
            comment(2, 36, 40, true),
            comment(3, 36, 36, false),
        ]);

        assert_eq!(markers.width(), 1);
        assert_eq!(markers.marker_for_line(Some(36)), "○");
        assert_eq!(markers.marker_for_line(Some(37)), "┃");
        assert_eq!(markers.marker_for_line(Some(40)), "○");
    }
}
