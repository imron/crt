//! Processed render contract.

/// Canonical pane id shared across UI adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaneId {
    FileList,
    Diff,
    Status,
    Overlay,
    Custom(u16),
}

/// Complete render snapshot emitted by core.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RenderModel {
    pub revision: u64,
    pub panes: Vec<PaneRenderModel>,
    pub status: Option<StatusMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneRenderModel {
    pub id: PaneId,
    pub title: Option<String>,
    pub lines: Vec<RenderLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderLine {
    pub spans: Vec<RenderSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderSpan {
    pub text: String,
    pub style: StyleToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StyleToken {
    Default,
    Selected,
    Focused,
    Added,
    Deleted,
    Context,
    Warning,
    Error,
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusMessage {
    pub text: String,
}

/// Interaction semantics for rendered output.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InteractionMap {
    pub panes: Vec<PaneInteractionMap>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneInteractionMap {
    pub pane_id: PaneId,
    pub regions: Vec<InteractionRegion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionRegion {
    pub id: String,
    pub bounds: RegionBounds,
    pub target: InteractionTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionBounds {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionTarget {
    FileRow {
        file_path: String,
    },
    DiffText {
        line: usize,
        column_start: usize,
        column_end: usize,
    },
    ScrollArea,
    ResizeBorder,
    OverlayItem {
        id: String,
    },
    Custom(String),
}

/// Render update stream: full snapshot for resync, delta for steady state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderUpdate {
    Snapshot {
        model: RenderModel,
        interaction: InteractionMap,
    },
    Delta {
        pane: Option<PaneRenderModel>,
        interaction: Option<PaneInteractionMap>,
        status: Option<StatusMessage>,
    },
}
