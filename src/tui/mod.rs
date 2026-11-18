//! Terminal UI adapter and renderer.

mod effects;
pub(crate) mod input;
pub(crate) mod render;
mod runtime;
mod state;

pub(crate) use effects::apply_core_effects;
pub(crate) use runtime::Tui;
pub(crate) use state::{InputMode, TuiState};
