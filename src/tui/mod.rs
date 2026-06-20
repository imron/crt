//! Terminal UI adapter and renderer.

use anyhow::{Context, Result};

use crate::client::Client;
use crate::review_types::ConnectionContext;

mod effects;
mod input;
mod render;
mod runtime;
mod state;

pub async fn run(client: Client, context: ConnectionContext) -> Result<()> {
    let mut tui = runtime::Tui::new(client, context)
        .await
        .context("Failed to initialize TUI")?;
    tui.run().await
}
