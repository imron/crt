use crate::app::AppState;

pub fn active_diff_content_height(state: &AppState) -> usize {
    state
        .active_document
        .as_ref()
        .map(|document| document.document().diff.len())
        .unwrap_or(0)
}

pub trait ViewportMetrics {
    fn diff_view_height(&self) -> usize;
}
