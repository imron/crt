//! Core render/status vocabulary shared by UI adapters.

/// Canonical pane id shared across UI adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaneId {
    FileList,
    Diff,
    Comments,
    Status,
    Overlay,
    Custom(u16),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusMessage {
    pub text: String,
}
