//! macOS GUI adapter.
//!
//! SwiftUI owns the process main thread. A multi-thread tokio runtime
//! keeps the client and any embedded server alive on worker threads.
//! Snapshot data crosses FFI as a JSON envelope (`{"data": ...}`).

use anyhow::{Context, Result};
use serde::Serialize;

use crate::app;
use crate::git;
use crate::review_types;

unsafe extern "C" {
    fn crt_gui_run(data: *const u8, size: usize);
}

#[derive(Serialize)]
struct GuiSnapshot {
    version: String,
    repo_name: String,
    worktree: String,
    head_ref: String,
    merge_base: String,
    status: String,
    files: Vec<GuiFile>,
}

#[derive(Serialize)]
struct GuiFile {
    path: String,
    kind: &'static str,
    status: &'static str,
}

#[derive(Serialize)]
struct SnapshotEnvelope<'a> {
    data: &'a GuiSnapshot,
}

pub fn run_gui(base: Option<&str>, root: bool, reset: bool, standalone: bool) -> Result<()> {
    let rt = tokio::runtime::Runtime::new().context("Failed to create tokio runtime")?;
    match rt.block_on(app::App::start_review(base, root, reset, standalone))? {
        app::ReviewStartup::Reset(summary) => {
            println!(
                "Reset review state for (merge_base: {}, head: {}): {} review(s) cleared.",
                summary.merge_base_short, summary.head_ref, summary.cleared
            );
            Ok(())
        }
        app::ReviewStartup::Review(app) => {
            let snapshot = snapshot_from_app(&app);
            let json = serde_json::to_vec(&SnapshotEnvelope { data: &snapshot })
                .context("Failed to serialize GUI snapshot")?;

            // NSApplication.run() blocks until the window closes. Worker
            // threads keep driving the embedded server for that whole time.
            // Swift copies `json` at the start of crt_gui_run.
            unsafe {
                crt_gui_run(json.as_ptr(), json.len());
            }

            rt.block_on(app.shutdown());
            Ok(())
        }
    }
}

fn snapshot_from_app(app: &app::App) -> GuiSnapshot {
    let context = &app.state.context;
    let repo_name = std::path::Path::new(&context.worktree)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| context.worktree.clone());

    GuiSnapshot {
        version: format!("{} ({})", env!("CARGO_PKG_VERSION"), env!("CRT_BUILD_HASH")),
        repo_name,
        worktree: context.worktree.clone(),
        head_ref: context.head_ref.clone(),
        merge_base: git::short_hash(&context.merge_base),
        status: "Connected".to_string(),
        files: app.state.files.iter().map(gui_file).collect(),
    }
}

fn gui_file(entry: &review_types::FileEntry) -> GuiFile {
    GuiFile {
        path: entry.change.path.clone(),
        kind: change_kind_name(entry.change.kind),
        status: review_status_name(&entry.status),
    }
}

fn change_kind_name(kind: review_types::ChangeKind) -> &'static str {
    match kind {
        review_types::ChangeKind::Added => "added",
        review_types::ChangeKind::Deleted => "deleted",
        review_types::ChangeKind::Modified => "modified",
        review_types::ChangeKind::Renamed => "renamed",
    }
}

fn review_status_name(status: &review_types::ReviewStatus) -> &'static str {
    match status {
        review_types::ReviewStatus::Unreviewed => "unreviewed",
        review_types::ReviewStatus::Reviewed { .. } => "reviewed",
        review_types::ReviewStatus::Changed { .. } => "changed",
    }
}

#[cfg(test)]
mod tests {
    use super::{GuiFile, GuiSnapshot, SnapshotEnvelope};

    #[test]
    fn snapshot_envelope_uses_snake_case_keys() {
        let snapshot = GuiSnapshot {
            version: "0.1.0 (deadbeef)".to_string(),
            repo_name: "crt".to_string(),
            worktree: "/tmp/crt".to_string(),
            head_ref: "wip/gui".to_string(),
            merge_base: "abc1234".to_string(),
            status: "Connected".to_string(),
            files: vec![GuiFile {
                path: "src/gui.rs".to_string(),
                kind: "modified",
                status: "unreviewed",
            }],
        };
        let json = serde_json::to_string(&SnapshotEnvelope { data: &snapshot }).unwrap();
        assert!(json.contains("\"data\""));
        assert!(json.contains("\"repo_name\""));
        assert!(json.contains("\"head_ref\""));
        assert!(json.contains("\"merge_base\""));
        assert!(json.contains("\"files\""));
        assert!(json.contains("src/gui.rs"));
        assert!(!json.contains("repoName"));
        assert!(!json.contains("headRef"));
        assert!(!json.contains("mergeBase"));
    }
}
