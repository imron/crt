//! Terminal UI adapter and renderer.

use anyhow::{Context, Result};

use crate::app::App;

mod effects;
mod input;
mod render;
mod runtime;
mod state;

pub async fn run(app: App) -> Result<()> {
    let mut tui = runtime::Tui::new(app).context("Failed to initialize TUI")?;
    let result = tui.run().await;
    let app = tui.into_app();
    app.shutdown().await;
    result
}
