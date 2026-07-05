pub use crate::app::model::CommentMarkerSet;
use crate::app::model::{CommentMarker as SemanticCommentMarker, CommentMarkerKind};
use crate::review_types::CommentAnchorSide;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentMarker {
    text: String,
    current: bool,
}

impl CommentMarker {
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

pub fn marker_column_width(_markers: &CommentMarkerSet) -> usize {
    // Always allocate the column so content doesn't shift when comments
    // are added or removed.
    1
}

pub fn marker_for_line(markers: &CommentMarkerSet, line: Option<u32>) -> CommentMarker {
    let width = marker_column_width(markers);
    let marker = markers.marker_for_line(line);
    comment_marker_from_semantic(width, marker)
}

pub fn marker_for_side_line(
    markers: &CommentMarkerSet,
    side: CommentAnchorSide,
    line: Option<u32>,
) -> CommentMarker {
    let width = marker_column_width(markers);
    let marker = markers.marker_for_side_line(side, line);
    comment_marker_from_semantic(width, marker)
}

pub fn marker_for_inline_row(markers: &CommentMarkerSet, row: usize) -> CommentMarker {
    let width = marker_column_width(markers);
    let marker = markers.marker_for_inline_row(row);
    comment_marker_from_semantic(width, marker)
}

fn comment_marker_from_semantic(width: usize, marker: SemanticCommentMarker) -> CommentMarker {
    let mut text = marker_text(marker);
    let pad = width.saturating_sub(text.chars().count());
    text.push_str(&" ".repeat(pad));
    CommentMarker {
        text,
        current: marker.is_current(),
    }
}

fn marker_text(marker: SemanticCommentMarker) -> String {
    match marker.kind() {
        None => String::new(),
        Some(CommentMarkerKind::Join) => "┃".to_string(),
        Some(CommentMarkerKind::SingleLine | CommentMarkerKind::Start | CommentMarkerKind::End) => {
            if marker.is_resolved() {
                "○".to_string()
            } else {
                "●".to_string()
            }
        }
    }
}
