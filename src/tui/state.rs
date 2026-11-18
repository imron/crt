//! TUI-owned presentation state.

use crate::tui::render::diff_view::DiffCache;

#[derive(Default)]
pub(crate) struct TuiState {
    pub diff_cache: Option<DiffCache>,
}
