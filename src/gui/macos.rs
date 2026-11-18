//! macOS GUI adapter.
//!
//! SwiftUI owns the process main thread. A multi-thread tokio runtime
//! keeps the client and any embedded server alive on worker threads.
//! Snapshot data crosses FFI as a JSON envelope (`{"data": ...}`).

use std::cell::RefCell;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::app;
use crate::git;
use crate::review_types;

unsafe extern "C" {
    fn crt_gui_run(data: *const u8, size: usize, icon: *const u8, icon_size: usize);
    fn crt_gui_apply_snapshot(data: *const u8, size: usize);
}

const APP_ICON_PNG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/crt-icon.png"));

// App is !Send (Rc document rows). Keep it on the GUI/main thread.
thread_local! {
    static SESSION: RefCell<Option<app::App>> = const { RefCell::new(None) };
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
    selected_path: Option<String>,
    diff: Option<GuiDiff>,
}

#[derive(Serialize)]
struct GuiFile {
    path: String,
    kind: &'static str,
    status: &'static str,
}

#[derive(Serialize)]
struct GuiDiff {
    path: String,
    is_binary: bool,
    lines: Vec<GuiDiffLine>,
}

#[derive(Serialize)]
struct GuiDiffLine {
    kind: &'static str,
    content: String,
    old_lineno: Option<u32>,
    new_lineno: Option<u32>,
}

#[derive(Serialize)]
struct SnapshotEnvelope<'a> {
    data: &'a GuiSnapshot,
}

#[derive(Deserialize)]
struct ActionEnvelope {
    data: GuiAction,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum GuiAction {
    SelectFile { path: String },
}

pub fn run_gui(
    base: Option<&str>,
    root: bool,
    reset: bool,
    standalone: bool,
    foreground: bool,
) -> Result<()> {
    if !foreground && !reset && detach_from_terminal()?.is_parent() {
        return Ok(());
    }

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
            let json = encode_snapshot(&snapshot)?;
            store_session(app);

            // NSApplication.run() blocks until the window closes. Worker
            // threads keep driving the embedded server for that whole time.
            // Swift copies `json` at the start of crt_gui_run.
            unsafe {
                crt_gui_run(
                    json.as_ptr(),
                    json.len(),
                    APP_ICON_PNG.as_ptr(),
                    APP_ICON_PNG.len(),
                );
            }

            if let Some(app) = take_session() {
                rt.block_on(app.shutdown());
            }
            Ok(())
        }
    }
}

enum ForkRole {
    Parent,
    Child,
}

impl ForkRole {
    fn is_parent(self) -> bool {
        matches!(self, Self::Parent)
    }
}

fn detach_from_terminal() -> Result<ForkRole> {
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(std::io::Error::last_os_error()).context("Failed to fork GUI process");
    }
    if pid > 0 {
        return Ok(ForkRole::Parent);
    }

    if unsafe { libc::setsid() } < 0 {
        return Err(std::io::Error::last_os_error())
            .context("Failed to create a new session for the GUI");
    }
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }

    let null = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDWR) };
    if null >= 0 {
        unsafe {
            libc::dup2(null, libc::STDIN_FILENO);
            libc::close(null);
        }
    }

    Ok(ForkRole::Child)
}

fn store_session(app: app::App) {
    SESSION.with(|session| {
        *session.borrow_mut() = Some(app);
    });
}

fn take_session() -> Option<app::App> {
    SESSION.with(|session| session.borrow_mut().take())
}

fn encode_snapshot(snapshot: &GuiSnapshot) -> Result<Vec<u8>> {
    serde_json::to_vec(&SnapshotEnvelope { data: snapshot })
        .context("Failed to serialize GUI snapshot")
}

fn snapshot_from_app(app: &app::App) -> GuiSnapshot {
    let context = &app.state.context;
    let repo_name = std::path::Path::new(&context.worktree)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| context.worktree.clone());
    let selected_path = app
        .state
        .selected_file_entry()
        .map(|entry| entry.change.path.clone());
    let diff = app.state.selected_file_entry().map(gui_diff);

    GuiSnapshot {
        version: format!("{} ({})", env!("CARGO_PKG_VERSION"), env!("CRT_BUILD_HASH")),
        repo_name,
        worktree: context.worktree.clone(),
        head_ref: context.head_ref.clone(),
        merge_base: git::short_hash(&context.merge_base),
        status: "Connected".to_string(),
        files: app.state.files.iter().map(gui_file).collect(),
        selected_path,
        diff,
    }
}

fn gui_file(entry: &review_types::FileEntry) -> GuiFile {
    GuiFile {
        path: entry.change.path.clone(),
        kind: change_kind_name(entry.change.kind),
        status: review_status_name(&entry.status),
    }
}

fn gui_diff(entry: &review_types::FileEntry) -> GuiDiff {
    let mut lines = Vec::new();
    if !entry.diff.is_binary {
        for hunk in &entry.diff.hunks {
            lines.push(GuiDiffLine {
                kind: "hunk",
                content: trim_diff_text(&hunk.header),
                old_lineno: None,
                new_lineno: None,
            });
            for line in &hunk.lines {
                lines.push(GuiDiffLine {
                    kind: diff_line_kind(line.kind),
                    content: trim_diff_text(&line.content),
                    old_lineno: line.old_lineno,
                    new_lineno: line.new_lineno,
                });
            }
        }
    }
    GuiDiff {
        path: entry.change.path.clone(),
        is_binary: entry.diff.is_binary,
        lines,
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

fn trim_diff_text(text: &str) -> String {
    text.trim_end_matches(['\n', '\r']).to_string()
}

fn diff_line_kind(kind: review_types::LineKind) -> &'static str {
    match kind {
        review_types::LineKind::Context => "context",
        review_types::LineKind::Addition => "addition",
        review_types::LineKind::Deletion => "deletion",
    }
}

fn apply_action(app: &mut app::App, action: GuiAction) {
    match action {
        GuiAction::SelectFile { path } => {
            let Some(index) = app
                .state
                .files
                .iter()
                .position(|entry| entry.change.path == path)
            else {
                return;
            };
            let focus = app.state.focus_for_file(index);
            app.state.select_file(index, focus, true);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn crt_gui_submit_action(data: *const u8, size: usize) {
    if data.is_null() || size == 0 {
        return;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, size) }.to_vec();
    let action = match serde_json::from_slice::<ActionEnvelope>(&bytes) {
        Ok(envelope) => envelope.data,
        Err(_) => return,
    };

    let json = SESSION.with(|session| {
        let mut session = session.borrow_mut();
        let app = session.as_mut()?;
        apply_action(app, action);
        encode_snapshot(&snapshot_from_app(app)).ok()
    });
    let Some(json) = json else {
        return;
    };

    unsafe {
        crt_gui_apply_snapshot(json.as_ptr(), json.len());
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActionEnvelope, GuiAction, GuiDiff, GuiDiffLine, GuiFile, GuiSnapshot, SnapshotEnvelope,
    };

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
            selected_path: Some("src/gui.rs".to_string()),
            diff: Some(GuiDiff {
                path: "src/gui.rs".to_string(),
                is_binary: false,
                lines: vec![GuiDiffLine {
                    kind: "addition",
                    content: "fn main() {}".to_string(),
                    old_lineno: None,
                    new_lineno: Some(1),
                }],
            }),
        };
        let json = serde_json::to_string(&SnapshotEnvelope { data: &snapshot }).unwrap();
        assert!(json.contains("\"data\""));
        assert!(json.contains("\"repo_name\""));
        assert!(json.contains("\"selected_path\""));
        assert!(json.contains("\"old_lineno\""));
        assert!(!json.contains("repoName"));
        assert!(!json.contains("selectedPath"));
        assert!(!json.contains("oldLineno"));
    }

    #[test]
    fn select_file_action_decodes_from_envelope() {
        let json = r#"{"data":{"type":"select_file","path":"src/gui.rs"}}"#;
        let envelope: ActionEnvelope = serde_json::from_str(json).unwrap();
        match envelope.data {
            GuiAction::SelectFile { path } => assert_eq!(path, "src/gui.rs"),
        }
    }
}
