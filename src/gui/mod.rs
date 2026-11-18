//! Native GUI adapter.
//!
//! macOS hosts SwiftUI on the main thread. Other targets expose a stub
//! that errors if `crt gui` is invoked.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod stub;

#[cfg(target_os = "macos")]
pub use macos::run_gui;
#[cfg(not(target_os = "macos"))]
pub use stub::run_gui;
