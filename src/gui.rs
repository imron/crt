//! macOS GUI adapter entry.
//!
//! SwiftUI owns the process main thread. A multi-thread tokio runtime
//! keeps the client and any embedded server alive on worker threads.

use anyhow::Result;

pub fn run() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        run_macos()
    }
    #[cfg(not(target_os = "macos"))]
    {
        anyhow::bail!("`crt gui` is only available on macOS")
    }
}

#[cfg(target_os = "macos")]
fn run_macos() -> Result<()> {
    use std::ffi::c_char;

    use anyhow::Context;

    unsafe extern "C" {
        fn crt_gui_run(
            version: *const c_char,
            repo_name: *const c_char,
            worktree: *const c_char,
            head_ref: *const c_char,
            status: *const c_char,
        );
    }

    let rt = tokio::runtime::Runtime::new().context("Failed to create tokio runtime")?;
    let snapshot = rt.block_on(connect_snapshot())?;

    let version = c_string(&snapshot.version)?;
    let repo_name = c_string(&snapshot.repo_name)?;
    let worktree = c_string(&snapshot.worktree)?;
    let head_ref = c_string(&snapshot.head_ref)?;
    let status = c_string(&snapshot.status)?;

    // NSApplication.run() blocks until the window closes. Worker threads
    // keep driving the embedded server for that whole time.
    unsafe {
        crt_gui_run(
            version.as_ptr(),
            repo_name.as_ptr(),
            worktree.as_ptr(),
            head_ref.as_ptr(),
            status.as_ptr(),
        );
    }

    rt.block_on(snapshot.client.shutdown());
    Ok(())
}

#[cfg(target_os = "macos")]
struct GuiSnapshot {
    client: crate::client::Client,
    version: String,
    repo_name: String,
    worktree: String,
    head_ref: String,
    status: String,
}

#[cfg(target_os = "macos")]
async fn connect_snapshot() -> Result<GuiSnapshot> {
    use anyhow::Context;

    use crate::app;
    use crate::client::Client;
    use crate::git::Repo;

    let socket_path = app::default_socket_path()?;
    let cwd = std::env::current_dir().context("Failed to determine current directory")?;
    let repo = Repo::open(&cwd)?;
    let context = repo.context()?;
    let client = Client::connect_or_start(&socket_path, false).await?;

    let repo_name = context
        .worktree
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| context.worktree.display().to_string());

    Ok(GuiSnapshot {
        client,
        version: format!("{} ({})", env!("CARGO_PKG_VERSION"), env!("CRT_BUILD_HASH")),
        repo_name,
        worktree: context.worktree.display().to_string(),
        head_ref: context.head.display_name(),
        status: "Connected".to_string(),
    })
}

#[cfg(target_os = "macos")]
fn c_string(value: &str) -> Result<std::ffi::CString> {
    use anyhow::Context;
    use std::ffi::CString;

    CString::new(value).with_context(|| format!("GUI string contained an interior NUL: {value}"))
}
