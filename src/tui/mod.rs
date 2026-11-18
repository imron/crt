//! Terminal UI adapter and renderer.

use anyhow::{Context, Result};

use crate::app::AppSession;

mod effects;
mod input;
mod render;
mod runtime;
mod state;

pub async fn run(session: AppSession) -> Result<()> {
    let mut tui = runtime::Tui::new(session).context("Failed to initialize TUI")?;
    tui.run().await
}
