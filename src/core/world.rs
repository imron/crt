//! Pane world models.
//!
//! Each interactive pane exposes a semantic world model that both rendering
//! and input mapping can target. UI adapters map native coordinates into these
//! worlds via `InteractionMap` and emit semantic events back to core.

use super::render::PaneId;

/// Semantic world model for a pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneWorldModel {
    FileList(FileListPaneWorld),
    Diff(DiffPaneWorld),
    Overlay(OverlayPaneWorld),
    Custom { pane: PaneId, description: String },
}

/// File-list pane semantics used for rendering and hit targeting.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileListPaneWorld {
    /// Mapping from display rows to file path ids.
    pub display_rows: Vec<Option<String>>,
    /// Current selected file path.
    pub selected_file: Option<String>,
}

/// Diff pane semantics used for text-precise navigation and selection.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiffPaneWorld {
    /// Anchors for each display line in the rendered diff.
    pub line_anchors: Vec<DiffLineAnchor>,
    /// Current cursor line in display-space.
    pub cursor_line: usize,
    /// Current cursor column in content-space.
    pub cursor_col: usize,
}

/// Semantic anchor for one rendered diff line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLineAnchor {
    pub display_row: usize,
    pub content_start_col: usize,
    pub content_end_col: usize,
    pub file_line: Option<usize>,
}

/// Overlay pane semantics (search, definition, help, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OverlayPaneWorld {
    /// Stable ids for selectable overlay items.
    pub selectable_item_ids: Vec<String>,
    /// Currently focused item id.
    pub selected_item_id: Option<String>,
}
