use std::collections::BTreeMap;

use crate::app::model::CommentAttachment;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentMarkerSet {
    markers_by_line: BTreeMap<u32, Vec<char>>,
    width: usize,
}

impl CommentMarkerSet {
    pub fn new(comments: &[CommentAttachment]) -> Self {
        let mut markers_by_line: BTreeMap<u32, Vec<char>> = BTreeMap::new();

        for comment in comments {
            let start = comment.line_start.max(1) as u32;
            let end = comment.line_end.max(comment.line_start).max(1) as u32;
            for line in start..=end {
                let marker = if start == end || line == start || line == end {
                    if comment.resolved { '○' } else { '●' }
                } else {
                    '┃'
                };
                markers_by_line.entry(line).or_default().push(marker);
            }
        }

        let width = markers_by_line.values().map(Vec::len).max().unwrap_or(0);

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
        let mut marker: String = self
            .markers_by_line
            .get(&line)
            .map(|markers| markers.iter().collect())
            .unwrap_or_default();
        let pad = self.width.saturating_sub(marker.chars().count());
        marker.push_str(&" ".repeat(pad));
        marker
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
    fn markers_stack_nested_comments() {
        let markers = CommentMarkerSet::new(&[comment(1, 3, 5, false), comment(2, 4, 4, true)]);

        assert_eq!(markers.width(), 2);
        assert_eq!(markers.marker_for_line(Some(3)), "● ");
        assert_eq!(markers.marker_for_line(Some(4)), "┃○");
        assert_eq!(markers.marker_for_line(Some(5)), "● ");
    }
}
