//! UI-neutral input protocol.

use super::prompt::PromptId;
use super::render::PaneId;

/// A core-ingress input event emitted by UI adapters.
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize { width: u16, height: u16 },
    FocusGained,
    FocusLost,
    PromptSubmit { id: PromptId, value: String },
    PromptCancel { id: PromptId },
}

/// Keyboard event (UI-neutral).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub kind: KeyEventKind,
    pub key: Key,
    pub modifiers: InputModifiers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEventKind {
    Press,
    Repeat,
    Release,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Function(u8),
}

/// Simple modifier bitset for input events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InputModifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// Mouse event after UI adapter normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MouseEvent {
    pub kind: MouseEventKind,
    pub button: Option<MouseButton>,
    /// Optional pane-local coordinates (for diagnostics and fallback).
    pub local_pos: Option<(u16, u16)>,
    /// Semantic hit target resolved by the adapter via `InteractionMap`.
    pub semantic_hit: Option<PointerSemanticHit>,
    pub modifiers: InputModifiers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventKind {
    Down,
    Up,
    Drag,
    Move,
    ScrollUp,
    ScrollDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Semantic pointer hit emitted by UI adapters after coordinate mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointerSemanticHit {
    pub pane_id: PaneId,
    /// Region id from `InteractionRegion.id` when available.
    pub region_id: Option<String>,
    /// Optional text-precise anchor for cursor/selection semantics.
    pub text_anchor: Option<TextAnchor>,
}

/// Text-precise position in pane semantic space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextAnchor {
    pub line: usize,
    pub column: usize,
}
