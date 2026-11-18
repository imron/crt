//! Native GUI adapter.
//!
//! macOS hosts SwiftUI on the main thread. Other targets expose a stub
//! that errors if `crt gui` is invoked.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod stub;

#[cfg(target_os = "macos")]
use macos::run_gui;
#[cfg(not(target_os = "macos"))]
use stub::run_gui;

use anyhow::Result;

pub fn run() -> Result<()> {
    run_gui()
}
