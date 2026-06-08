//! Core interaction contracts for thin clients.
//!
//! This module defines UI-neutral input, prompt, render, and service contracts.
//! UI adapters (TUI/GUI) translate native events into these types and render
//! from these models without embedding business logic.

pub mod command;
pub mod diff;
pub mod input;
pub mod interaction;
pub mod navigation;
pub mod prompt;
pub mod render;
pub mod review;
pub mod search;
pub mod services;
pub mod world;

pub use input::{
    InputEvent, InputModifiers, Key, KeyEvent, KeyEventKind, MouseButton, MouseEvent,
    PointerSemanticHit, TextAnchor,
};
pub use interaction::{
    ConnectionState, CoreEffect, CoreEffects, CoreInteractionEngine, InteractionContext,
};
pub use prompt::{PromptId, PromptKind, PromptRequest};
pub use render::{
    InteractionMap, InteractionRegion, InteractionTarget, PaneId, PaneInteractionMap,
    PaneRenderModel, RegionBounds, RenderLine, RenderModel, RenderSpan, RenderUpdate,
    StatusMessage, StyleToken,
};
pub use world::{
    DiffLineAnchor, DiffPaneWorld, FileListPaneWorld, OverlayPaneWorld, PaneWorldModel,
};
