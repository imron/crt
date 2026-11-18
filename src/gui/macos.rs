//! macOS GUI adapter.
//!
//! SwiftUI owns the process main thread. A multi-thread tokio runtime
//! keeps the client and any embedded server alive on worker threads.
//! Snapshot data crosses FFI as a JSON envelope (`{"data": ...}`).

use anyhow::{Context, Result};
use serde::Serialize;

use crate::app;
use crate::client;
use crate::git;

unsafe extern "C" {
    fn crt_gui_run(data: *const u8, size: usize);
}

#[derive(Serialize)]
struct GuiSnapshot {
    version: String,
    repo_name: String,
    worktree: String,
    head_ref: String,
    status: String,
}

#[derive(Serialize)]
struct SnapshotEnvelope<'a> {
    data: &'a GuiSnapshot,
}

struct GuiSession {
    client: client::Client,
    snapshot: GuiSnapshot,
}

pub fn run_gui() -> Result<()> {
    let rt = tokio::runtime::Runtime::new().context("Failed to create tokio runtime")?;
    let session = rt.block_on(connect_session())?;
    let json = serde_json::to_vec(&SnapshotEnvelope {
        data: &session.snapshot,
    })
    .context("Failed to serialize GUI snapshot")?;

    // NSApplication.run() blocks until the window closes. Worker threads
    // keep driving the embedded server for that whole time. Swift copies
    // `json` at the start of crt_gui_run.
    unsafe {
        crt_gui_run(json.as_ptr(), json.len());
    }

    rt.block_on(session.client.shutdown());
    Ok(())
}

async fn connect_session() -> Result<GuiSession> {
    let socket_path = app::default_socket_path()?;
    let cwd = std::env::current_dir().context("Failed to determine current directory")?;
    let repo = git::Repo::open(&cwd)?;
    let context = repo.context()?;
    let client = client::Client::connect_or_start(&socket_path, false).await?;

    let repo_name = context
        .worktree
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| context.worktree.display().to_string());

    Ok(GuiSession {
        client,
        snapshot: GuiSnapshot {
            version: format!("{} ({})", env!("CARGO_PKG_VERSION"), env!("CRT_BUILD_HASH")),
            repo_name,
            worktree: context.worktree.display().to_string(),
            head_ref: context.head.display_name(),
            status: "Connected".to_string(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::{GuiSnapshot, SnapshotEnvelope};

    #[test]
    fn snapshot_envelope_uses_snake_case_keys() {
        let snapshot = GuiSnapshot {
            version: "0.1.0 (deadbeef)".to_string(),
            repo_name: "crt".to_string(),
            worktree: "/tmp/crt".to_string(),
            head_ref: "wip/gui".to_string(),
            status: "Connected".to_string(),
        };
        let json = serde_json::to_string(&SnapshotEnvelope { data: &snapshot }).unwrap();
        assert!(json.contains("\"data\""));
        assert!(json.contains("\"repo_name\""));
        assert!(json.contains("\"head_ref\""));
        assert!(!json.contains("repoName"));
        assert!(!json.contains("headRef"));
    }
}
