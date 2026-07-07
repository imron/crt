use crate::app::AppState;
use crate::core::text::char_to_byte_index;

pub fn active_diff_content_height(state: &AppState, view: &impl AppViewport) -> usize {
    state
        .active_document
        .as_ref()
        .map(|document| document.document().diff.len())
        .unwrap_or_else(|| view.diff_content_height())
}

pub trait AppViewport {
    fn hunk_start_rows(&self) -> &[usize];
    fn hunk_end_rows(&self) -> &[usize];
    fn hunk_first_change_rows(&self) -> &[usize];
    fn diff_gutter_cols(&self) -> usize;
    fn diff_content_start_col(&self) -> usize {
        if self.diff_gutter_cols() > 0 {
            self.diff_gutter_cols() + 3
        } else {
            0
        }
    }
    fn diff_content_height(&self) -> usize;
    fn diff_view_height(&self) -> usize;
    fn diff_rendered_text(&self) -> &[String];

    fn max_diff_scroll(&self) -> usize {
        self.diff_content_height().saturating_sub(1)
    }

    fn line_content_trimmed(&self, row: usize) -> &str {
        let line = match self.diff_rendered_text().get(row) {
            Some(line) => line.as_str(),
            None => return "",
        };
        let start = char_to_byte_index(line, self.diff_content_start_col()).unwrap_or(line.len());
        line[start..].trim_end()
    }

    fn current_line_text_len(&self, state: &AppState) -> usize {
        self.line_content_trimmed(state.diff_line_cursor)
            .chars()
            .count()
    }
}
