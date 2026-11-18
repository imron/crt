use crate::app::AppState;

pub fn active_diff_content_height(state: &AppState, view: &impl AppViewport) -> usize {
    state
        .active_document
        .as_ref()
        .map(|document| document.document().diff.len())
        .unwrap_or_else(|| view.diff_content_height())
}

pub trait AppViewport {
    fn diff_content_height(&self) -> usize;
    fn diff_view_height(&self) -> usize;

    fn max_diff_scroll(&self) -> usize {
        self.diff_content_height().saturating_sub(1)
    }
}
