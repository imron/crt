//! Core interaction contracts for thin clients.
//!
//! This module defines UI-neutral input, prompt, status, and service contracts.
//! UI adapters (TUI/GUI) translate native events into these types and render
//! from AppModel without embedding business logic.

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
pub mod text;

pub use input::{
    AppTarget, InputEvent, InputModifiers, Key, KeyEvent, KeyEventKind, MouseButton, MouseEvent,
    MouseEventKind, PointerSemanticHit, TextAnchor,
};
pub use interaction::{
    CommentEffect, CommentsPanelEffect, ConnectionState, CoreEffect, CoreEffects,
    CoreInteractionEngine, CurrentCommentContext, DefinitionResultsEffect, DiffCursorEffect,
    DiffSearchEffect, InteractionContext, PaneEffect, SearchResultsEffect, VisualSelectionEffect,
};
pub use prompt::{PromptId, PromptKind, PromptRequest};
pub use render::{PaneId, StatusMessage};
