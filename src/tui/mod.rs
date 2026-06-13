//! Terminal UI adapter and renderer.

pub(crate) mod input;
pub(crate) mod render;
mod runtime;

pub(crate) use runtime::Tui;
