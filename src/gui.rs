//! macOS GUI adapter entry.
//!
//! The SwiftUI window owns the process main thread. Later slices can start
//! the tokio runtime on a background thread before calling into Swift.

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
    use std::ffi::{CString, c_char};

    use anyhow::Context;

    unsafe extern "C" {
        fn crt_gui_run(version: *const c_char);
    }

    let version = format!("{} ({})", env!("CARGO_PKG_VERSION"), env!("CRT_BUILD_HASH"));
    let version = CString::new(version).context("GUI version string contained an interior NUL")?;

    // NSApplication.run() blocks until the window closes.
    unsafe {
        crt_gui_run(version.as_ptr());
    }
    Ok(())
}
