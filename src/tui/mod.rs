//! Terminal UI adapter and renderer.

pub(crate) mod input;
pub(crate) mod render;
mod runtime;
mod state;

pub(crate) use runtime::Tui;
pub(crate) use state::TuiState;
