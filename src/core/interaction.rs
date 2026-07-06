//! Core interaction entrypoint scaffold.

use super::command::{self, CommandParse};
use super::input::{AppTarget, InputEvent, Key, KeyEventKind, MouseButton, MouseEventKind};
use super::navigation::Direction;
use super::prompt::{PromptId, PromptKind, PromptRequest};
use super::render::{PaneId, StatusMessage};

/// Core output effect stream.
pub type CoreEffects = Vec<CoreEffect>;

/// Core-to-UI effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreEffect {
    RequestPrompt(PromptRequest),
    ClearPrompt { id: PromptId },
    Command(CommandParse),
    DiffSearch(DiffSearchEffect),
    Comment(CommentEffect),
    CommentsPanel(CommentsPanelEffect),
    DiffCursor(DiffCursorEffect),
    VisualSelection(VisualSelectionEffect),
    SearchResults(SearchResultsEffect),
    DefinitionResults(DefinitionResultsEffect),
    Pane(PaneEffect),
    Quit,
    Suspend,
    ShowHelp,
    DismissHelp,
    Undo,
    ReviewToggle,
    NavigateFile(Direction),
    NavigateFileSection(Direction),
    NavigateUnresolvedComment(Direction),
    JumpHunk(Direction),
    GoToDefinition,
    PopJumpStack,
    TogglePaneFocus,
    TogglePaneVisibility(PaneId),
    ToggleInlineDiff,
    ToggleBlame,
    ToggleWhitespaceIgnored,
    CycleViewMode,
    CycleDiffAlgorithm,
    ToggleDiffBase,
    Status(StatusMessage),
    ConnectionState(ConnectionState),
    TransientError(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffSearchEffect {
    Submit { query: String },
    NextMatch,
    PreviousMatch,
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentEffect {
    SubmitBody { body: String },
    SubmitEditBody { id: i64, body: String },
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentsPanelEffect {
    Toggle,
    SelectNext,
    SelectPrevious,
    ToggleExpanded,
    NavigateToSelected,
    NavigateNextComment,
    NavigatePreviousComment,
    EditCurrent,
    ToggleResolvedCurrent,
    RequestDeleteCurrent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffCursorEffect {
    MoveTo { line: usize, column: usize },
    LineDown,
    LineUp,
    PageDown,
    PageUp,
    HalfPageDown,
    HalfPageUp,
    ScrollDown,
    ScrollUp,
    WheelDown,
    WheelUp,
    Top,
    Bottom,
    ViewTop,
    ViewMiddle,
    ViewBottom,
    CharLeft,
    CharRight,
    LineStart,
    LineEnd,
    WordForward,
    WordBackward,
    BigWordForward,
    BigWordBackward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualSelectionEffect {
    StartLine,
    StartText { anchor: super::input::TextAnchor },
    ExtendTo { anchor: super::input::TextAnchor },
    Move(DiffCursorEffect),
    Cancel,
    Commit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchResultsEffect {
    Close,
    SelectNext,
    SelectPrevious,
    SelectFirst,
    SelectLast,
    AcceptSelected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefinitionResultsEffect {
    Close,
    SelectNext,
    SelectPrevious,
    AcceptSelected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneEffect {
    ActivateFileListSelection,
    ActivateDiffSelection,
    SelectFile { file_index: usize },
    SelectComment { comment_id: i64 },
}

/// Connection lifecycle states for UI adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Connected,
    Reconnecting,
    Reconnected,
    Disconnected,
}

/// Context passed from adapter/runtime into the interaction engine.
#[derive(Debug, Clone, Default)]
pub struct InteractionContext {
    /// Optional hint about current connection state.
    pub connection_state: Option<ConnectionState>,
    /// Whether the help overlay is currently visible in the adapter.
    pub help_visible: bool,
    /// Search results overlay visibility, supplied by the adapter.
    pub search_results_visible: bool,
    /// Definition results overlay visibility, supplied by the adapter.
    pub definition_results_visible: bool,
    /// Currently focused pane, supplied by the adapter.
    pub focused_pane: Option<PaneId>,
    /// Whether a diff-search query is active and can be cleared.
    pub diff_search_active: bool,
    /// Whether active diff-search navigation has matches.
    pub diff_search_has_matches: bool,
    /// Whether the diff pane is visible and can accept diff interactions.
    pub diff_pane_visible: bool,
    /// Whether a visual selection is active in the diff pane.
    pub visual_selection_active: bool,
    /// Whether a comment anchor is waiting for body text.
    pub pending_comment_anchor_active: bool,
    /// Whether the comments panel is visible.
    pub comments_panel_visible: bool,
    /// Whether the cursor or comments panel selection has a current comment.
    pub current_comment_active: bool,
    /// Current comment id/body for edit prompt setup.
    pub current_comment: Option<CurrentCommentContext>,
    /// Whether the adapter still has an active quit confirmation prompt.
    pub quit_confirmation_active: bool,
    /// Word under the cursor, supplied by the adapter for commands like `gd`.
    pub fallback_word: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentCommentContext {
    pub id: i64,
    pub body: String,
}

/// Core interaction engine.
///
/// This owns prompt lifecycle and maps core input contracts to app effects.
#[derive(Debug, Default)]
pub struct CoreInteractionEngine {
    next_prompt_id: u64,
    active_prompt: Option<ActivePrompt>,
}

#[derive(Debug, Clone)]
struct ActivePrompt {
    id: PromptId,
    kind: PromptKind,
    comment_edit_id: Option<i64>,
}

impl CoreInteractionEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset_prompt_state(&mut self) {
        self.active_prompt = None;
    }

    /// Single core input entrypoint for UI adapters.
    pub fn handle_input(&mut self, event: InputEvent, context: &InteractionContext) -> CoreEffects {
        if let InputEvent::Mouse(mouse) = &event {
            if let Some(effect) = mouse_click_effect(mouse) {
                return vec![effect];
            }
            if let Some(effect) = mouse_diff_cursor_effect(mouse) {
                return vec![CoreEffect::DiffCursor(effect)];
            }
        }

        if let InputEvent::Key(key) = &event {
            if key.kind == KeyEventKind::Press {
                if context.visual_selection_active
                    && context.diff_pane_visible
                    && matches!(key.key, Key::Enter | Key::Char(' ') | Key::Char('c'))
                    && key_has_no_modifier(key.modifiers)
                {
                    let id = self.next_prompt(PromptKind::Comment);
                    return vec![
                        CoreEffect::VisualSelection(VisualSelectionEffect::Commit),
                        CoreEffect::RequestPrompt(PromptRequest {
                            id,
                            kind: PromptKind::Comment,
                            title: "Comment".to_string(),
                            placeholder: Some("Write a comment".to_string()),
                            initial_value: String::new(),
                        }),
                    ];
                }
                if key.key == Key::Char('C') && key_has_no_command_modifier(key.modifiers) {
                    return vec![CoreEffect::CommentsPanel(CommentsPanelEffect::Toggle)];
                }
                if context.help_visible {
                    if help_dismiss_key(key) {
                        return vec![CoreEffect::DismissHelp];
                    }
                    return Vec::new();
                }
                if context.search_results_visible {
                    if let Some(effect) = search_results_key_effect(key) {
                        return vec![CoreEffect::SearchResults(effect)];
                    }
                    return Vec::new();
                }
                if context.definition_results_visible {
                    if let Some(effect) = definition_results_key_effect(key) {
                        return vec![CoreEffect::DefinitionResults(effect)];
                    }
                    return Vec::new();
                }
                if let Some(effect) = visual_selection_key_effect(key, context) {
                    return vec![CoreEffect::VisualSelection(effect)];
                }
                if let Some(effect) = comments_panel_key_effect(key, context) {
                    return vec![CoreEffect::CommentsPanel(effect)];
                }
                if context.current_comment_active
                    && matches!(key.key, Key::Char('c') | Key::Char('e'))
                    && key_has_no_modifier(key.modifiers)
                {
                    if let Some(comment) = &context.current_comment {
                        let id = self.next_comment_edit_prompt(comment.id);
                        return vec![CoreEffect::RequestPrompt(PromptRequest {
                            id,
                            kind: PromptKind::Comment,
                            title: "Edit Comment".to_string(),
                            placeholder: Some("Write a comment".to_string()),
                            initial_value: comment.body.clone(),
                        })];
                    }
                    return vec![CoreEffect::CommentsPanel(CommentsPanelEffect::EditCurrent)];
                }
                if context.diff_pane_visible
                    && matches!(
                        context.focused_pane,
                        Some(PaneId::Diff) | Some(PaneId::FileList)
                    )
                    && !context.visual_selection_active
                    && key.key == Key::Char('c')
                    && key_has_no_modifier(key.modifiers)
                {
                    let id = self.next_prompt(PromptKind::Comment);
                    return vec![
                        CoreEffect::VisualSelection(VisualSelectionEffect::StartLine),
                        CoreEffect::VisualSelection(VisualSelectionEffect::Commit),
                        CoreEffect::RequestPrompt(PromptRequest {
                            id,
                            kind: PromptKind::Comment,
                            title: "Comment".to_string(),
                            placeholder: Some("Write a comment".to_string()),
                            initial_value: String::new(),
                        }),
                    ];
                }
                if context.pending_comment_anchor_active
                    && matches!(key.key, Key::Enter | Key::Char(' '))
                    && key_has_no_modifier(key.modifiers)
                {
                    let id = self.next_prompt(PromptKind::Comment);
                    return vec![CoreEffect::RequestPrompt(PromptRequest {
                        id,
                        kind: PromptKind::Comment,
                        title: "Comment".to_string(),
                        placeholder: Some("Write a comment".to_string()),
                        initial_value: String::new(),
                    })];
                }
                if let Some(effect) = pane_key_effect(key, context.focused_pane) {
                    return vec![CoreEffect::Pane(effect)];
                }
                if let Some(effect) = diff_cursor_key_effect(key) {
                    return vec![CoreEffect::DiffCursor(effect)];
                }
            }
        }

        match event {
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_control_modifier_only(key.modifiers)
                    && key.key == Key::Char('c') =>
            {
                if context.quit_confirmation_active {
                    vec![CoreEffect::Quit]
                } else {
                    vec![CoreEffect::Status(StatusMessage {
                        text: "Press Ctrl-C again to quit".to_string(),
                    })]
                }
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_command_modifier(key.modifiers)
                    && key.key == Key::Char(':') =>
            {
                let id = self.next_prompt(PromptKind::CommandLine);
                vec![CoreEffect::RequestPrompt(PromptRequest {
                    id,
                    kind: PromptKind::CommandLine,
                    title: "Command".to_string(),
                    placeholder: Some("Type a command".to_string()),
                    initial_value: String::new(),
                })]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_command_modifier(key.modifiers)
                    && key.key == Key::Char('/') =>
            {
                let id = self.next_prompt(PromptKind::Search);
                vec![CoreEffect::RequestPrompt(PromptRequest {
                    id,
                    kind: PromptKind::Search,
                    title: "Search".to_string(),
                    placeholder: Some("Type a regex".to_string()),
                    initial_value: String::new(),
                })]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Char('q') =>
            {
                vec![CoreEffect::Quit]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_control_modifier_only(key.modifiers)
                    && key.key == Key::Char('z') =>
            {
                vec![CoreEffect::Suspend]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_command_modifier(key.modifiers)
                    && key.key == Key::Char('?') =>
            {
                vec![CoreEffect::ShowHelp]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press && approve_review_toggle_key(&key) =>
            {
                vec![CoreEffect::ReviewToggle]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Char('u') =>
            {
                vec![CoreEffect::Undo]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_control_modifier_only(key.modifiers)
                    && key.key == Key::Char('b') =>
            {
                vec![CoreEffect::ToggleBlame]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_control_modifier_only(key.modifiers)
                    && key.key == Key::Char('w') =>
            {
                vec![CoreEffect::ToggleWhitespaceIgnored]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_control_shift_modifier_only(key.modifiers)
                    && matches!(key.key, Key::Char('n') | Key::Char('N')) =>
            {
                vec![CoreEffect::NavigateFileSection(Direction::Next)]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_control_shift_modifier_only(key.modifiers)
                    && matches!(key.key, Key::Char('p') | Key::Char('P')) =>
            {
                vec![CoreEffect::NavigateFileSection(Direction::Prev)]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Char('}') =>
            {
                vec![CoreEffect::NavigateUnresolvedComment(Direction::Next)]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Char('{') =>
            {
                vec![CoreEffect::NavigateUnresolvedComment(Direction::Prev)]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_control_modifier_only(key.modifiers)
                    && key.key == Key::Char('n') =>
            {
                vec![CoreEffect::NavigateFile(Direction::Next)]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_control_modifier_only(key.modifiers)
                    && key.key == Key::Char('p') =>
            {
                vec![CoreEffect::NavigateFile(Direction::Prev)]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Char(']') =>
            {
                vec![CoreEffect::JumpHunk(Direction::Next)]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Char('[') =>
            {
                vec![CoreEffect::JumpHunk(Direction::Prev)]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_control_modifier_only(key.modifiers)
                    && key.key == Key::Char(']') =>
            {
                vec![CoreEffect::GoToDefinition]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_control_modifier_only(key.modifiers)
                    && key.key == Key::Char('t') =>
            {
                vec![CoreEffect::PopJumpStack]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_command_modifier(key.modifiers)
                    && key.key == Key::Tab =>
            {
                vec![CoreEffect::TogglePaneFocus]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Char('1') =>
            {
                vec![CoreEffect::TogglePaneVisibility(PaneId::FileList)]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Char('2') =>
            {
                vec![CoreEffect::TogglePaneVisibility(PaneId::Diff)]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Char('i') =>
            {
                vec![CoreEffect::ToggleInlineDiff]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Char('s') =>
            {
                vec![CoreEffect::CycleViewMode]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Char('d') =>
            {
                vec![CoreEffect::CycleDiffAlgorithm]
            }
            InputEvent::Key(key)
                if key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Char('m') =>
            {
                vec![CoreEffect::ToggleDiffBase]
            }
            InputEvent::Key(key)
                if context.diff_search_has_matches
                    && key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Char('n') =>
            {
                vec![CoreEffect::DiffSearch(DiffSearchEffect::NextMatch)]
            }
            InputEvent::Key(key)
                if context.diff_search_has_matches
                    && key.kind == KeyEventKind::Press
                    && key_has_no_command_modifier(key.modifiers)
                    && key.key == Key::Char('N') =>
            {
                vec![CoreEffect::DiffSearch(DiffSearchEffect::PreviousMatch)]
            }
            InputEvent::Key(key)
                if context.diff_search_active
                    && key.kind == KeyEventKind::Press
                    && key_has_no_modifier(key.modifiers)
                    && key.key == Key::Escape =>
            {
                vec![CoreEffect::DiffSearch(DiffSearchEffect::Clear)]
            }
            InputEvent::PromptSubmit { id, value } => {
                let Some(active) = self.take_active_prompt(id) else {
                    return Vec::new();
                };

                match active.kind {
                    PromptKind::CommandLine => vec![
                        CoreEffect::ClearPrompt { id },
                        CoreEffect::Command(command::parse_command(
                            &value,
                            context.fallback_word.as_deref(),
                        )),
                    ],
                    PromptKind::Search => vec![
                        CoreEffect::ClearPrompt { id },
                        CoreEffect::DiffSearch(DiffSearchEffect::Submit { query: value }),
                    ],
                    PromptKind::Comment => vec![
                        CoreEffect::ClearPrompt { id },
                        match active.comment_edit_id {
                            Some(comment_id) => {
                                CoreEffect::Comment(CommentEffect::SubmitEditBody {
                                    id: comment_id,
                                    body: value,
                                })
                            }
                            None => CoreEffect::Comment(CommentEffect::SubmitBody { body: value }),
                        },
                    ],
                    PromptKind::Custom(_) => vec![CoreEffect::ClearPrompt { id }],
                }
            }
            InputEvent::PromptCancel { id } => {
                let Some(active) = self.take_active_prompt(id) else {
                    return Vec::new();
                };

                let mut effects = vec![CoreEffect::ClearPrompt { id }];
                if active.kind == PromptKind::Comment && active.comment_edit_id.is_none() {
                    effects.push(CoreEffect::Comment(CommentEffect::Cancel));
                }
                effects
            }
            _ => Vec::new(),
        }
    }

    fn next_prompt(&mut self, kind: PromptKind) -> PromptId {
        self.next_prompt_id = self.next_prompt_id.saturating_add(1);
        let id = PromptId(self.next_prompt_id);
        self.active_prompt = Some(ActivePrompt {
            id,
            kind,
            comment_edit_id: None,
        });
        id
    }

    fn next_comment_edit_prompt(&mut self, comment_id: i64) -> PromptId {
        self.next_prompt_id = self.next_prompt_id.saturating_add(1);
        let id = PromptId(self.next_prompt_id);
        self.active_prompt = Some(ActivePrompt {
            id,
            kind: PromptKind::Comment,
            comment_edit_id: Some(comment_id),
        });
        id
    }

    fn take_active_prompt(&mut self, id: PromptId) -> Option<ActivePrompt> {
        let active = self.active_prompt.clone()?;
        if active.id != id {
            return None;
        }
        self.active_prompt = None;
        Some(active)
    }
}

fn key_has_no_command_modifier(modifiers: super::input::InputModifiers) -> bool {
    !modifiers.ctrl && !modifiers.alt
}

fn key_has_no_modifier(modifiers: super::input::InputModifiers) -> bool {
    !modifiers.ctrl && !modifiers.alt && !modifiers.shift
}

fn key_has_control_modifier_only(modifiers: super::input::InputModifiers) -> bool {
    modifiers.ctrl && !modifiers.alt && !modifiers.shift
}

fn key_has_control_shift_modifier_only(modifiers: super::input::InputModifiers) -> bool {
    modifiers.ctrl && !modifiers.alt && modifiers.shift
}

fn approve_review_toggle_key(key: &super::input::KeyEvent) -> bool {
    match key.key {
        Key::Char('a') => key_has_no_modifier(key.modifiers),
        Key::Char('A') => key_has_no_command_modifier(key.modifiers),
        _ => false,
    }
}

fn help_dismiss_key(key: &super::input::KeyEvent) -> bool {
    match key.key {
        Key::Escape => key_has_no_modifier(key.modifiers),
        Key::Char('q') => key_has_no_modifier(key.modifiers),
        Key::Char('?') => key_has_no_command_modifier(key.modifiers),
        _ => false,
    }
}

fn search_results_key_effect(key: &super::input::KeyEvent) -> Option<SearchResultsEffect> {
    match key.key {
        Key::Escape if key_has_no_modifier(key.modifiers) => Some(SearchResultsEffect::Close),
        Key::Char('q') if key_has_no_modifier(key.modifiers) => Some(SearchResultsEffect::Close),
        Key::Char('j') if key_has_no_modifier(key.modifiers) => {
            Some(SearchResultsEffect::SelectNext)
        }
        Key::Down if key_has_no_modifier(key.modifiers) => Some(SearchResultsEffect::SelectNext),
        Key::Char('k') if key_has_no_modifier(key.modifiers) => {
            Some(SearchResultsEffect::SelectPrevious)
        }
        Key::Up if key_has_no_modifier(key.modifiers) => Some(SearchResultsEffect::SelectPrevious),
        Key::Char('g') if key_has_no_modifier(key.modifiers) => {
            Some(SearchResultsEffect::SelectFirst)
        }
        Key::Char('G') if key_has_no_command_modifier(key.modifiers) => {
            Some(SearchResultsEffect::SelectLast)
        }
        Key::Enter if key_has_no_modifier(key.modifiers) => {
            Some(SearchResultsEffect::AcceptSelected)
        }
        _ => None,
    }
}

fn definition_results_key_effect(key: &super::input::KeyEvent) -> Option<DefinitionResultsEffect> {
    match key.key {
        Key::Escape if key_has_no_modifier(key.modifiers) => Some(DefinitionResultsEffect::Close),
        Key::Char('q') if key_has_no_modifier(key.modifiers) => {
            Some(DefinitionResultsEffect::Close)
        }
        Key::Char('j') if key_has_no_modifier(key.modifiers) => {
            Some(DefinitionResultsEffect::SelectNext)
        }
        Key::Down if key_has_no_modifier(key.modifiers) => {
            Some(DefinitionResultsEffect::SelectNext)
        }
        Key::Char('k') if key_has_no_modifier(key.modifiers) => {
            Some(DefinitionResultsEffect::SelectPrevious)
        }
        Key::Up if key_has_no_modifier(key.modifiers) => {
            Some(DefinitionResultsEffect::SelectPrevious)
        }
        Key::Enter if key_has_no_modifier(key.modifiers) => {
            Some(DefinitionResultsEffect::AcceptSelected)
        }
        _ => None,
    }
}

fn pane_key_effect(
    key: &super::input::KeyEvent,
    focused_pane: Option<PaneId>,
) -> Option<PaneEffect> {
    if key.key != Key::Enter || !key_has_no_modifier(key.modifiers) {
        return None;
    }

    match focused_pane {
        Some(PaneId::FileList) => Some(PaneEffect::ActivateFileListSelection),
        Some(PaneId::Diff) => Some(PaneEffect::ActivateDiffSelection),
        Some(PaneId::Comments) => None,
        _ => None,
    }
}

fn comments_panel_key_effect(
    key: &super::input::KeyEvent,
    context: &InteractionContext,
) -> Option<CommentsPanelEffect> {
    match key.key {
        Key::Char('r') if key_has_no_modifier(key.modifiers) && context.current_comment_active => {
            Some(CommentsPanelEffect::ToggleResolvedCurrent)
        }
        Key::Char('d') if key_has_no_modifier(key.modifiers) && context.current_comment_active => {
            Some(CommentsPanelEffect::RequestDeleteCurrent)
        }
        Key::Char('j') | Key::Down
            if key_has_no_modifier(key.modifiers)
                && context.focused_pane == Some(PaneId::Comments) =>
        {
            Some(CommentsPanelEffect::SelectNext)
        }
        Key::Char('k') | Key::Up
            if key_has_no_modifier(key.modifiers)
                && context.focused_pane == Some(PaneId::Comments) =>
        {
            Some(CommentsPanelEffect::SelectPrevious)
        }
        Key::Enter
            if key_has_no_modifier(key.modifiers)
                && context.focused_pane == Some(PaneId::Comments) =>
        {
            Some(CommentsPanelEffect::ToggleExpanded)
        }
        _ => None,
    }
}

fn diff_cursor_key_effect(key: &super::input::KeyEvent) -> Option<DiffCursorEffect> {
    match key.key {
        Key::Char('j') | Key::Down if key_has_no_modifier(key.modifiers) => {
            Some(DiffCursorEffect::LineDown)
        }
        Key::Char('k') | Key::Up if key_has_no_modifier(key.modifiers) => {
            Some(DiffCursorEffect::LineUp)
        }
        Key::Char(' ') => Some(DiffCursorEffect::PageDown),
        Key::Char('d') if key_has_control_modifier_only(key.modifiers) => {
            Some(DiffCursorEffect::HalfPageDown)
        }
        Key::Char('u') if key_has_control_modifier_only(key.modifiers) => {
            Some(DiffCursorEffect::HalfPageUp)
        }
        Key::Char('e') if key_has_control_modifier_only(key.modifiers) => {
            Some(DiffCursorEffect::ScrollDown)
        }
        Key::Char('y') if key_has_control_modifier_only(key.modifiers) => {
            Some(DiffCursorEffect::ScrollUp)
        }
        Key::Char('g') if key_has_no_modifier(key.modifiers) => Some(DiffCursorEffect::Top),
        Key::Char('G') if key_has_no_command_modifier(key.modifiers) => {
            Some(DiffCursorEffect::Bottom)
        }
        Key::Char('H') if key_has_no_command_modifier(key.modifiers) => {
            Some(DiffCursorEffect::ViewTop)
        }
        Key::Char('M') if key_has_no_command_modifier(key.modifiers) => {
            Some(DiffCursorEffect::ViewMiddle)
        }
        Key::Char('L') if key_has_no_command_modifier(key.modifiers) => {
            Some(DiffCursorEffect::ViewBottom)
        }
        Key::Char('h') | Key::Left if key_has_no_modifier(key.modifiers) => {
            Some(DiffCursorEffect::CharLeft)
        }
        Key::Char('l') | Key::Right if key_has_no_modifier(key.modifiers) => {
            Some(DiffCursorEffect::CharRight)
        }
        Key::Char('0') if key_has_no_modifier(key.modifiers) => Some(DiffCursorEffect::LineStart),
        Key::Char('$') if key_has_no_command_modifier(key.modifiers) => {
            Some(DiffCursorEffect::LineEnd)
        }
        Key::Char('w') if key_has_no_modifier(key.modifiers) => Some(DiffCursorEffect::WordForward),
        Key::Char('b') if key_has_no_modifier(key.modifiers) => {
            Some(DiffCursorEffect::WordBackward)
        }
        Key::Char('W') if key_has_no_command_modifier(key.modifiers) => {
            Some(DiffCursorEffect::BigWordForward)
        }
        Key::Char('B') if key_has_no_command_modifier(key.modifiers) => {
            Some(DiffCursorEffect::BigWordBackward)
        }
        _ => None,
    }
}

fn visual_selection_key_effect(
    key: &super::input::KeyEvent,
    context: &InteractionContext,
) -> Option<VisualSelectionEffect> {
    if !context.diff_pane_visible {
        return None;
    }

    if context.visual_selection_active {
        match key.key {
            Key::Escape if key_has_no_modifier(key.modifiers) => {
                Some(VisualSelectionEffect::Cancel)
            }
            Key::Enter if key_has_no_modifier(key.modifiers) => Some(VisualSelectionEffect::Commit),
            _ => diff_cursor_key_effect(key).map(VisualSelectionEffect::Move),
        }
    } else {
        match key.key {
            Key::Char('V') if key_has_no_command_modifier(key.modifiers) => {
                Some(VisualSelectionEffect::StartLine)
            }
            Key::Char('v') if key.modifiers.shift && key_has_no_command_modifier(key.modifiers) => {
                Some(VisualSelectionEffect::StartLine)
            }
            Key::Char('v') if key_has_no_modifier(key.modifiers) => {
                Some(VisualSelectionEffect::StartLine)
            }
            _ => None,
        }
    }
}

fn mouse_diff_cursor_effect(mouse: &super::input::MouseEvent) -> Option<DiffCursorEffect> {
    if mouse.semantic_hit.as_ref()?.pane_id != PaneId::Diff {
        return None;
    }

    match mouse.kind {
        MouseEventKind::ScrollDown => Some(DiffCursorEffect::WheelDown),
        MouseEventKind::ScrollUp => Some(DiffCursorEffect::WheelUp),
        _ => None,
    }
}

fn mouse_click_effect(mouse: &super::input::MouseEvent) -> Option<CoreEffect> {
    if mouse.kind != MouseEventKind::Down || mouse.button != Some(MouseButton::Left) {
        return None;
    }

    let hit = mouse.semantic_hit.as_ref()?;
    match hit.target {
        AppTarget::File { index } => Some(CoreEffect::Pane(PaneEffect::SelectFile {
            file_index: index,
        })),
        AppTarget::Comment { id } => Some(CoreEffect::Pane(PaneEffect::SelectComment {
            comment_id: id,
        })),
        AppTarget::DiffText { anchor } => Some(CoreEffect::DiffCursor(DiffCursorEffect::MoveTo {
            line: anchor.line,
            column: anchor.column,
        })),
        AppTarget::Pane { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::input::{AppTarget, InputModifiers, KeyEvent};

    fn key_event(key: Key, modifiers: InputModifiers) -> InputEvent {
        InputEvent::Key(KeyEvent {
            kind: KeyEventKind::Press,
            key,
            modifiers,
        })
    }

    fn requested_prompt_id(effects: &[CoreEffect]) -> PromptId {
        match effects {
            [CoreEffect::RequestPrompt(PromptRequest { id, .. })] => *id,
            _ => panic!("expected prompt request"),
        }
    }

    #[test]
    fn colon_requests_command_prompt() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(Key::Char(':'), InputModifiers::default()),
            &InteractionContext::default(),
        );

        assert!(matches!(
            effects.as_slice(),
            [CoreEffect::RequestPrompt(PromptRequest {
                kind: PromptKind::CommandLine,
                ..
            })]
        ));
    }

    #[test]
    fn slash_requests_search_prompt() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(Key::Char('/'), InputModifiers::default()),
            &InteractionContext::default(),
        );

        assert!(matches!(
            effects.as_slice(),
            [CoreEffect::RequestPrompt(PromptRequest {
                kind: PromptKind::Search,
                ..
            })]
        ));
    }

    #[test]
    fn shifted_colon_still_requests_command_prompt() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char(':'),
                InputModifiers {
                    shift: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert!(matches!(
            effects.as_slice(),
            [CoreEffect::RequestPrompt(PromptRequest {
                kind: PromptKind::CommandLine,
                ..
            })]
        ));
    }

    #[test]
    fn control_colon_does_not_request_prompt() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char(':'),
                InputModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert!(effects.is_empty());
    }

    #[test]
    fn a_requests_review_toggle() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(Key::Char('a'), InputModifiers::default()),
            &InteractionContext::default(),
        );

        assert_eq!(effects, vec![CoreEffect::ReviewToggle]);
    }

    #[test]
    fn shift_a_requests_review_toggle() {
        let mut engine = CoreInteractionEngine::new();

        let shifted = InputModifiers {
            shift: true,
            ..Default::default()
        };
        let effects = engine.handle_input(key_event(Key::Char('A'), shifted), &Default::default());

        assert_eq!(effects, vec![CoreEffect::ReviewToggle]);
    }

    #[test]
    fn r_does_not_request_review_toggle() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(Key::Char('r'), InputModifiers::default()),
            &InteractionContext::default(),
        );

        assert!(effects.is_empty());
    }

    #[test]
    fn q_requests_quit_when_help_is_hidden() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(Key::Char('q'), InputModifiers::default()),
            &InteractionContext::default(),
        );

        assert_eq!(effects, vec![CoreEffect::Quit]);
    }

    #[test]
    fn control_c_requests_quit_confirmation_or_quit() {
        let mut engine = CoreInteractionEngine::new();
        let control_c = key_event(
            Key::Char('c'),
            InputModifiers {
                ctrl: true,
                ..Default::default()
            },
        );

        let first = engine.handle_input(control_c.clone(), &InteractionContext::default());
        let second = engine.handle_input(
            control_c,
            &InteractionContext {
                quit_confirmation_active: true,
                ..InteractionContext::default()
            },
        );

        assert_eq!(
            first,
            vec![CoreEffect::Status(StatusMessage {
                text: "Press Ctrl-C again to quit".to_string()
            })]
        );
        assert_eq!(second, vec![CoreEffect::Quit]);
    }

    #[test]
    fn control_z_requests_suspend() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char('z'),
                InputModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert_eq!(effects, vec![CoreEffect::Suspend]);
    }

    #[test]
    fn question_mark_requests_help() {
        let mut engine = CoreInteractionEngine::new();

        let normal = engine.handle_input(
            key_event(Key::Char('?'), InputModifiers::default()),
            &InteractionContext::default(),
        );
        let shifted = engine.handle_input(
            key_event(
                Key::Char('?'),
                InputModifiers {
                    shift: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert_eq!(normal, vec![CoreEffect::ShowHelp]);
        assert_eq!(shifted, vec![CoreEffect::ShowHelp]);
    }

    #[test]
    fn help_visible_dismiss_keys_dismiss_help_before_global_actions() {
        let mut engine = CoreInteractionEngine::new();
        let context = InteractionContext {
            help_visible: true,
            ..InteractionContext::default()
        };

        let q = engine.handle_input(
            key_event(Key::Char('q'), InputModifiers::default()),
            &context,
        );
        let escape =
            engine.handle_input(key_event(Key::Escape, InputModifiers::default()), &context);
        let question = engine.handle_input(
            key_event(
                Key::Char('?'),
                InputModifiers {
                    shift: true,
                    ..Default::default()
                },
            ),
            &context,
        );

        assert_eq!(q, vec![CoreEffect::DismissHelp]);
        assert_eq!(escape, vec![CoreEffect::DismissHelp]);
        assert_eq!(question, vec![CoreEffect::DismissHelp]);
    }

    #[test]
    fn help_visible_swallows_non_dismiss_keys() {
        let mut engine = CoreInteractionEngine::new();
        let context = InteractionContext {
            help_visible: true,
            ..InteractionContext::default()
        };

        let effects = engine.handle_input(
            key_event(Key::Char('j'), InputModifiers::default()),
            &context,
        );

        assert!(effects.is_empty());
    }

    #[test]
    fn search_results_visible_routes_overlay_keys_before_global_actions() {
        let mut engine = CoreInteractionEngine::new();
        let context = InteractionContext {
            search_results_visible: true,
            ..InteractionContext::default()
        };

        let close = engine.handle_input(
            key_event(Key::Char('q'), InputModifiers::default()),
            &context,
        );
        let next = engine.handle_input(key_event(Key::Down, InputModifiers::default()), &context);
        let previous = engine.handle_input(
            key_event(Key::Char('k'), InputModifiers::default()),
            &context,
        );
        let first = engine.handle_input(
            key_event(Key::Char('g'), InputModifiers::default()),
            &context,
        );
        let last = engine.handle_input(
            key_event(
                Key::Char('G'),
                InputModifiers {
                    shift: true,
                    ..Default::default()
                },
            ),
            &context,
        );
        let accept =
            engine.handle_input(key_event(Key::Enter, InputModifiers::default()), &context);

        assert_eq!(
            close,
            vec![CoreEffect::SearchResults(SearchResultsEffect::Close)]
        );
        assert_eq!(
            next,
            vec![CoreEffect::SearchResults(SearchResultsEffect::SelectNext)]
        );
        assert_eq!(
            previous,
            vec![CoreEffect::SearchResults(
                SearchResultsEffect::SelectPrevious
            )]
        );
        assert_eq!(
            first,
            vec![CoreEffect::SearchResults(SearchResultsEffect::SelectFirst)]
        );
        assert_eq!(
            last,
            vec![CoreEffect::SearchResults(SearchResultsEffect::SelectLast)]
        );
        assert_eq!(
            accept,
            vec![CoreEffect::SearchResults(
                SearchResultsEffect::AcceptSelected
            )]
        );
    }

    #[test]
    fn search_results_visible_swallows_non_overlay_keys() {
        let mut engine = CoreInteractionEngine::new();
        let context = InteractionContext {
            search_results_visible: true,
            ..InteractionContext::default()
        };

        let effects = engine.handle_input(
            key_event(Key::Char('h'), InputModifiers::default()),
            &context,
        );

        assert!(effects.is_empty());
    }

    #[test]
    fn definition_results_visible_routes_overlay_keys_before_global_actions() {
        let mut engine = CoreInteractionEngine::new();
        let context = InteractionContext {
            definition_results_visible: true,
            ..InteractionContext::default()
        };

        let close = engine.handle_input(
            key_event(Key::Char('q'), InputModifiers::default()),
            &context,
        );
        let next = engine.handle_input(
            key_event(Key::Char('j'), InputModifiers::default()),
            &context,
        );
        let previous = engine.handle_input(key_event(Key::Up, InputModifiers::default()), &context);
        let accept =
            engine.handle_input(key_event(Key::Enter, InputModifiers::default()), &context);

        assert_eq!(
            close,
            vec![CoreEffect::DefinitionResults(
                DefinitionResultsEffect::Close
            )]
        );
        assert_eq!(
            next,
            vec![CoreEffect::DefinitionResults(
                DefinitionResultsEffect::SelectNext
            )]
        );
        assert_eq!(
            previous,
            vec![CoreEffect::DefinitionResults(
                DefinitionResultsEffect::SelectPrevious
            )]
        );
        assert_eq!(
            accept,
            vec![CoreEffect::DefinitionResults(
                DefinitionResultsEffect::AcceptSelected
            )]
        );
    }

    #[test]
    fn definition_results_visible_swallows_non_overlay_keys() {
        let mut engine = CoreInteractionEngine::new();
        let context = InteractionContext {
            definition_results_visible: true,
            ..InteractionContext::default()
        };

        let effects = engine.handle_input(
            key_event(Key::Char('l'), InputModifiers::default()),
            &context,
        );

        assert!(effects.is_empty());
    }

    #[test]
    fn diff_cursor_line_and_page_keys_request_cursor_effects() {
        let mut engine = CoreInteractionEngine::new();

        let line_down = engine.handle_input(
            key_event(Key::Down, InputModifiers::default()),
            &Default::default(),
        );
        let line_up = engine.handle_input(
            key_event(Key::Char('k'), InputModifiers::default()),
            &Default::default(),
        );
        let page_down = engine.handle_input(
            key_event(
                Key::Char(' '),
                InputModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &Default::default(),
        );
        let half_down = engine.handle_input(
            key_event(
                Key::Char('d'),
                InputModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &Default::default(),
        );
        let half_up = engine.handle_input(
            key_event(
                Key::Char('u'),
                InputModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &Default::default(),
        );

        assert_eq!(
            line_down,
            vec![CoreEffect::DiffCursor(DiffCursorEffect::LineDown)]
        );
        assert_eq!(
            line_up,
            vec![CoreEffect::DiffCursor(DiffCursorEffect::LineUp)]
        );
        assert_eq!(
            page_down,
            vec![CoreEffect::DiffCursor(DiffCursorEffect::PageDown)]
        );
        assert_eq!(
            half_down,
            vec![CoreEffect::DiffCursor(DiffCursorEffect::HalfPageDown)]
        );
        assert_eq!(
            half_up,
            vec![CoreEffect::DiffCursor(DiffCursorEffect::HalfPageUp)]
        );
    }

    #[test]
    fn diff_cursor_view_and_horizontal_keys_request_cursor_effects() {
        let mut engine = CoreInteractionEngine::new();
        let shifted = InputModifiers {
            shift: true,
            ..Default::default()
        };

        let scroll_down = engine.handle_input(
            key_event(
                Key::Char('e'),
                InputModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &Default::default(),
        );
        let scroll_up = engine.handle_input(
            key_event(
                Key::Char('y'),
                InputModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &Default::default(),
        );
        let top = engine.handle_input(
            key_event(Key::Char('g'), InputModifiers::default()),
            &Default::default(),
        );
        let bottom = engine.handle_input(key_event(Key::Char('G'), shifted), &Default::default());
        let view_top = engine.handle_input(key_event(Key::Char('H'), shifted), &Default::default());
        let view_middle =
            engine.handle_input(key_event(Key::Char('M'), shifted), &Default::default());
        let view_bottom =
            engine.handle_input(key_event(Key::Char('L'), shifted), &Default::default());
        let char_left = engine.handle_input(
            key_event(Key::Left, InputModifiers::default()),
            &Default::default(),
        );
        let char_right = engine.handle_input(
            key_event(Key::Char('l'), InputModifiers::default()),
            &Default::default(),
        );
        let line_start = engine.handle_input(
            key_event(Key::Char('0'), InputModifiers::default()),
            &Default::default(),
        );
        let line_end = engine.handle_input(key_event(Key::Char('$'), shifted), &Default::default());

        assert_eq!(
            scroll_down,
            vec![CoreEffect::DiffCursor(DiffCursorEffect::ScrollDown)]
        );
        assert_eq!(
            scroll_up,
            vec![CoreEffect::DiffCursor(DiffCursorEffect::ScrollUp)]
        );
        assert_eq!(top, vec![CoreEffect::DiffCursor(DiffCursorEffect::Top)]);
        assert_eq!(
            bottom,
            vec![CoreEffect::DiffCursor(DiffCursorEffect::Bottom)]
        );
        assert_eq!(
            view_top,
            vec![CoreEffect::DiffCursor(DiffCursorEffect::ViewTop)]
        );
        assert_eq!(
            view_middle,
            vec![CoreEffect::DiffCursor(DiffCursorEffect::ViewMiddle)]
        );
        assert_eq!(
            view_bottom,
            vec![CoreEffect::DiffCursor(DiffCursorEffect::ViewBottom)]
        );
        assert_eq!(
            char_left,
            vec![CoreEffect::DiffCursor(DiffCursorEffect::CharLeft)]
        );
        assert_eq!(
            char_right,
            vec![CoreEffect::DiffCursor(DiffCursorEffect::CharRight)]
        );
        assert_eq!(
            line_start,
            vec![CoreEffect::DiffCursor(DiffCursorEffect::LineStart)]
        );
        assert_eq!(
            line_end,
            vec![CoreEffect::DiffCursor(DiffCursorEffect::LineEnd)]
        );
    }

    #[test]
    fn diff_cursor_word_keys_request_cursor_effects() {
        let mut engine = CoreInteractionEngine::new();
        let shifted = InputModifiers {
            shift: true,
            ..Default::default()
        };

        let word_forward = engine.handle_input(
            key_event(Key::Char('w'), InputModifiers::default()),
            &Default::default(),
        );
        let word_backward = engine.handle_input(
            key_event(Key::Char('b'), InputModifiers::default()),
            &Default::default(),
        );
        let bigword_forward =
            engine.handle_input(key_event(Key::Char('W'), shifted), &Default::default());
        let bigword_backward =
            engine.handle_input(key_event(Key::Char('B'), shifted), &Default::default());

        assert_eq!(
            word_forward,
            vec![CoreEffect::DiffCursor(DiffCursorEffect::WordForward)]
        );
        assert_eq!(
            word_backward,
            vec![CoreEffect::DiffCursor(DiffCursorEffect::WordBackward)]
        );
        assert_eq!(
            bigword_forward,
            vec![CoreEffect::DiffCursor(DiffCursorEffect::BigWordForward)]
        );
        assert_eq!(
            bigword_backward,
            vec![CoreEffect::DiffCursor(DiffCursorEffect::BigWordBackward)]
        );
    }

    #[test]
    fn visual_selection_start_keys_require_visible_diff_pane() {
        let mut engine = CoreInteractionEngine::new();
        let visible_context = InteractionContext {
            diff_pane_visible: true,
            ..InteractionContext::default()
        };
        let hidden_context = InteractionContext {
            diff_pane_visible: false,
            ..InteractionContext::default()
        };
        let shifted = InputModifiers {
            shift: true,
            ..Default::default()
        };

        let line = engine.handle_input(key_event(Key::Char('V'), shifted), &visible_context);
        let line_lowercase_shift =
            engine.handle_input(key_event(Key::Char('v'), shifted), &visible_context);
        let lowercase_line = engine.handle_input(
            key_event(Key::Char('v'), InputModifiers::default()),
            &visible_context,
        );
        let ignored = engine.handle_input(
            key_event(Key::Char('v'), InputModifiers::default()),
            &hidden_context,
        );

        assert_eq!(
            line,
            vec![CoreEffect::VisualSelection(
                VisualSelectionEffect::StartLine
            )]
        );
        assert_eq!(
            line_lowercase_shift,
            vec![CoreEffect::VisualSelection(
                VisualSelectionEffect::StartLine
            )]
        );
        assert_eq!(
            lowercase_line,
            vec![CoreEffect::VisualSelection(
                VisualSelectionEffect::StartLine
            )]
        );
        assert!(ignored.is_empty());
    }

    #[test]
    fn diff_comment_key_captures_current_line_and_requests_prompt() {
        let mut engine = CoreInteractionEngine::new();
        let context = InteractionContext {
            focused_pane: Some(PaneId::Diff),
            diff_pane_visible: true,
            ..InteractionContext::default()
        };

        let effects = engine.handle_input(
            key_event(Key::Char('c'), InputModifiers::default()),
            &context,
        );

        assert_eq!(
            effects,
            vec![
                CoreEffect::VisualSelection(VisualSelectionEffect::StartLine),
                CoreEffect::VisualSelection(VisualSelectionEffect::Commit),
                CoreEffect::RequestPrompt(PromptRequest {
                    id: PromptId(1),
                    kind: PromptKind::Comment,
                    title: "Comment".to_string(),
                    placeholder: Some("Write a comment".to_string()),
                    initial_value: String::new(),
                })
            ]
        );
    }

    #[test]
    fn diff_comment_key_accepts_file_list_focus() {
        let mut engine = CoreInteractionEngine::new();
        let context = InteractionContext {
            focused_pane: Some(PaneId::FileList),
            diff_pane_visible: true,
            ..InteractionContext::default()
        };

        let effects = engine.handle_input(
            key_event(Key::Char('c'), InputModifiers::default()),
            &context,
        );

        assert_eq!(
            effects,
            vec![
                CoreEffect::VisualSelection(VisualSelectionEffect::StartLine),
                CoreEffect::VisualSelection(VisualSelectionEffect::Commit),
                CoreEffect::RequestPrompt(PromptRequest {
                    id: PromptId(1),
                    kind: PromptKind::Comment,
                    title: "Comment".to_string(),
                    placeholder: Some("Write a comment".to_string()),
                    initial_value: String::new(),
                })
            ]
        );
    }

    #[test]
    fn diff_comment_key_requires_visible_diff_pane() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(Key::Char('c'), InputModifiers::default()),
            &InteractionContext {
                focused_pane: Some(PaneId::Diff),
                diff_pane_visible: false,
                ..InteractionContext::default()
            },
        );

        assert!(effects.is_empty());
    }

    #[test]
    fn active_visual_selection_routes_movement_escape_and_enter() {
        let mut engine = CoreInteractionEngine::new();
        let context = InteractionContext {
            diff_pane_visible: true,
            visual_selection_active: true,
            ..InteractionContext::default()
        };

        let down = engine.handle_input(
            key_event(Key::Char('j'), InputModifiers::default()),
            &context,
        );
        let escape =
            engine.handle_input(key_event(Key::Escape, InputModifiers::default()), &context);
        let enter = engine.handle_input(key_event(Key::Enter, InputModifiers::default()), &context);

        assert_eq!(
            down,
            vec![CoreEffect::VisualSelection(VisualSelectionEffect::Move(
                DiffCursorEffect::LineDown
            ))]
        );
        assert_eq!(
            escape,
            vec![CoreEffect::VisualSelection(VisualSelectionEffect::Cancel)]
        );
        assert_eq!(
            enter,
            vec![
                CoreEffect::VisualSelection(VisualSelectionEffect::Commit),
                CoreEffect::RequestPrompt(PromptRequest {
                    id: PromptId(1),
                    kind: PromptKind::Comment,
                    title: "Comment".to_string(),
                    placeholder: Some("Write a comment".to_string()),
                    initial_value: String::new(),
                })
            ]
        );
    }

    #[test]
    fn active_visual_selection_comment_key_commits_and_requests_prompt() {
        let mut engine = CoreInteractionEngine::new();
        let context = InteractionContext {
            diff_pane_visible: true,
            visual_selection_active: true,
            ..InteractionContext::default()
        };

        let effects = engine.handle_input(
            key_event(Key::Char('c'), InputModifiers::default()),
            &context,
        );

        assert_eq!(
            effects,
            vec![
                CoreEffect::VisualSelection(VisualSelectionEffect::Commit),
                CoreEffect::RequestPrompt(PromptRequest {
                    id: PromptId(1),
                    kind: PromptKind::Comment,
                    title: "Comment".to_string(),
                    placeholder: Some("Write a comment".to_string()),
                    initial_value: String::new(),
                })
            ]
        );
    }

    #[test]
    fn semantic_diff_mouse_scroll_requests_cursor_effects() {
        let mut engine = CoreInteractionEngine::new();

        let down = engine.handle_input(
            InputEvent::Mouse(super::super::input::MouseEvent {
                kind: MouseEventKind::ScrollDown,
                button: None,
                local_pos: Some((4, 2)),
                semantic_hit: Some(super::super::input::PointerSemanticHit {
                    pane_id: PaneId::Diff,
                    target: AppTarget::DiffText {
                        anchor: super::super::input::TextAnchor { line: 8, column: 0 },
                    },
                    region_id: Some("diff-line:8".to_string()),
                    text_anchor: Some(super::super::input::TextAnchor { line: 8, column: 0 }),
                }),
                modifiers: InputModifiers::default(),
            }),
            &InteractionContext::default(),
        );
        let up = engine.handle_input(
            InputEvent::Mouse(super::super::input::MouseEvent {
                kind: MouseEventKind::ScrollUp,
                button: None,
                local_pos: Some((4, 2)),
                semantic_hit: Some(super::super::input::PointerSemanticHit {
                    pane_id: PaneId::Diff,
                    target: AppTarget::DiffText {
                        anchor: super::super::input::TextAnchor { line: 8, column: 0 },
                    },
                    region_id: Some("diff-line:8".to_string()),
                    text_anchor: Some(super::super::input::TextAnchor { line: 8, column: 0 }),
                }),
                modifiers: InputModifiers::default(),
            }),
            &InteractionContext::default(),
        );

        assert_eq!(
            down,
            vec![CoreEffect::DiffCursor(DiffCursorEffect::WheelDown)]
        );
        assert_eq!(up, vec![CoreEffect::DiffCursor(DiffCursorEffect::WheelUp)]);
    }

    #[test]
    fn non_diff_mouse_scroll_has_no_cursor_effect() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            InputEvent::Mouse(super::super::input::MouseEvent {
                kind: MouseEventKind::ScrollDown,
                button: None,
                local_pos: Some((4, 2)),
                semantic_hit: Some(super::super::input::PointerSemanticHit {
                    pane_id: PaneId::FileList,
                    target: AppTarget::Pane {
                        pane_id: PaneId::FileList,
                    },
                    region_id: Some("file-list-row:0".to_string()),
                    text_anchor: Some(super::super::input::TextAnchor { line: 0, column: 0 }),
                }),
                modifiers: InputModifiers::default(),
            }),
            &InteractionContext::default(),
        );

        assert!(effects.is_empty());
    }

    #[test]
    fn semantic_file_list_mouse_down_requests_file_selection() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            InputEvent::Mouse(super::super::input::MouseEvent {
                kind: MouseEventKind::Down,
                button: Some(MouseButton::Left),
                local_pos: Some((4, 3)),
                semantic_hit: Some(super::super::input::PointerSemanticHit {
                    pane_id: PaneId::FileList,
                    target: AppTarget::File { index: 5 },
                    region_id: Some("file:src/lib.rs".to_string()),
                    text_anchor: Some(super::super::input::TextAnchor {
                        line: 12,
                        column: 3,
                    }),
                }),
                modifiers: InputModifiers::default(),
            }),
            &InteractionContext::default(),
        );

        assert_eq!(
            effects,
            vec![CoreEffect::Pane(PaneEffect::SelectFile { file_index: 5 })]
        );
    }

    #[test]
    fn semantic_file_list_mouse_down_requests_comment_selection() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            InputEvent::Mouse(super::super::input::MouseEvent {
                kind: MouseEventKind::Down,
                button: Some(MouseButton::Left),
                local_pos: Some((4, 3)),
                semantic_hit: Some(super::super::input::PointerSemanticHit {
                    pane_id: PaneId::FileList,
                    target: AppTarget::Comment { id: 9 },
                    region_id: Some("comment:9".to_string()),
                    text_anchor: Some(super::super::input::TextAnchor {
                        line: 12,
                        column: 3,
                    }),
                }),
                modifiers: InputModifiers::default(),
            }),
            &InteractionContext::default(),
        );

        assert_eq!(
            effects,
            vec![CoreEffect::Pane(PaneEffect::SelectComment {
                comment_id: 9
            })]
        );
    }

    #[test]
    fn semantic_diff_mouse_down_requests_cursor_move() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            InputEvent::Mouse(super::super::input::MouseEvent {
                kind: MouseEventKind::Down,
                button: Some(MouseButton::Left),
                local_pos: Some((9, 5)),
                semantic_hit: Some(super::super::input::PointerSemanticHit {
                    pane_id: PaneId::Diff,
                    target: AppTarget::DiffText {
                        anchor: super::super::input::TextAnchor {
                            line: 18,
                            column: 5,
                        },
                    },
                    region_id: Some("diff-line:18".to_string()),
                    text_anchor: Some(super::super::input::TextAnchor {
                        line: 18,
                        column: 5,
                    }),
                }),
                modifiers: InputModifiers::default(),
            }),
            &InteractionContext::default(),
        );

        assert_eq!(
            effects,
            vec![CoreEffect::DiffCursor(DiffCursorEffect::MoveTo {
                line: 18,
                column: 5,
            })]
        );
    }

    #[test]
    fn pane_enter_requests_focused_pane_activation() {
        let mut engine = CoreInteractionEngine::new();

        let file_list = engine.handle_input(
            key_event(Key::Enter, InputModifiers::default()),
            &InteractionContext {
                focused_pane: Some(PaneId::FileList),
                ..InteractionContext::default()
            },
        );
        let diff = engine.handle_input(
            key_event(Key::Enter, InputModifiers::default()),
            &InteractionContext {
                focused_pane: Some(PaneId::Diff),
                ..InteractionContext::default()
            },
        );

        assert_eq!(
            file_list,
            vec![CoreEffect::Pane(PaneEffect::ActivateFileListSelection)]
        );
        assert_eq!(
            diff,
            vec![CoreEffect::Pane(PaneEffect::ActivateDiffSelection)]
        );
    }

    #[test]
    fn enter_without_focused_pane_has_no_pane_action() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(Key::Enter, InputModifiers::default()),
            &InteractionContext::default(),
        );

        assert!(effects.is_empty());
    }

    #[test]
    fn control_r_does_not_request_review_toggle() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char('r'),
                InputModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert!(effects.is_empty());
    }

    #[test]
    fn control_n_requests_next_file_navigation() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char('n'),
                InputModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert_eq!(effects, vec![CoreEffect::NavigateFile(Direction::Next)]);
    }

    #[test]
    fn control_p_requests_previous_file_navigation() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char('p'),
                InputModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert_eq!(effects, vec![CoreEffect::NavigateFile(Direction::Prev)]);
    }

    #[test]
    fn shifted_control_n_requests_next_file_section_navigation() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char('n'),
                InputModifiers {
                    ctrl: true,
                    shift: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert_eq!(
            effects,
            vec![CoreEffect::NavigateFileSection(Direction::Next)]
        );
    }

    #[test]
    fn shifted_control_p_requests_previous_file_section_navigation() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char('p'),
                InputModifiers {
                    ctrl: true,
                    shift: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert_eq!(
            effects,
            vec![CoreEffect::NavigateFileSection(Direction::Prev)]
        );
    }

    #[test]
    fn bracket_keys_request_hunk_navigation() {
        let mut engine = CoreInteractionEngine::new();

        let next = engine.handle_input(
            key_event(Key::Char(']'), InputModifiers::default()),
            &InteractionContext::default(),
        );
        let prev = engine.handle_input(
            key_event(Key::Char('['), InputModifiers::default()),
            &InteractionContext::default(),
        );

        assert_eq!(next, vec![CoreEffect::JumpHunk(Direction::Next)]);
        assert_eq!(prev, vec![CoreEffect::JumpHunk(Direction::Prev)]);
    }

    #[test]
    fn control_bracket_requests_definition_navigation() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char(']'),
                InputModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert_eq!(effects, vec![CoreEffect::GoToDefinition]);
    }

    #[test]
    fn control_t_requests_jump_stack_pop() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char('t'),
                InputModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert_eq!(effects, vec![CoreEffect::PopJumpStack]);
    }

    #[test]
    fn shifted_bracket_does_not_request_hunk_navigation() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char(']'),
                InputModifiers {
                    shift: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert!(effects.is_empty());
    }

    #[test]
    fn tab_requests_pane_focus_toggle() {
        let mut engine = CoreInteractionEngine::new();

        let normal = engine.handle_input(
            key_event(Key::Tab, InputModifiers::default()),
            &InteractionContext::default(),
        );
        let shifted = engine.handle_input(
            key_event(
                Key::Tab,
                InputModifiers {
                    shift: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert_eq!(normal, vec![CoreEffect::TogglePaneFocus]);
        assert_eq!(shifted, vec![CoreEffect::TogglePaneFocus]);
    }

    #[test]
    fn number_keys_request_pane_visibility_toggle() {
        let mut engine = CoreInteractionEngine::new();

        let file_list = engine.handle_input(
            key_event(Key::Char('1'), InputModifiers::default()),
            &InteractionContext::default(),
        );
        let diff = engine.handle_input(
            key_event(Key::Char('2'), InputModifiers::default()),
            &InteractionContext::default(),
        );

        assert_eq!(
            file_list,
            vec![CoreEffect::TogglePaneVisibility(PaneId::FileList)]
        );
        assert_eq!(diff, vec![CoreEffect::TogglePaneVisibility(PaneId::Diff)]);
    }

    #[test]
    fn view_keys_request_view_effects() {
        let mut engine = CoreInteractionEngine::new();

        let inline = engine.handle_input(
            key_event(Key::Char('i'), InputModifiers::default()),
            &InteractionContext::default(),
        );
        let view = engine.handle_input(
            key_event(Key::Char('s'), InputModifiers::default()),
            &InteractionContext::default(),
        );
        let algorithm = engine.handle_input(
            key_event(Key::Char('d'), InputModifiers::default()),
            &InteractionContext::default(),
        );
        let base = engine.handle_input(
            key_event(Key::Char('m'), InputModifiers::default()),
            &InteractionContext::default(),
        );
        let blame = engine.handle_input(
            key_event(
                Key::Char('b'),
                InputModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );
        let whitespace = engine.handle_input(
            key_event(
                Key::Char('w'),
                InputModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert_eq!(inline, vec![CoreEffect::ToggleInlineDiff]);
        assert_eq!(view, vec![CoreEffect::CycleViewMode]);
        assert_eq!(algorithm, vec![CoreEffect::CycleDiffAlgorithm]);
        assert_eq!(base, vec![CoreEffect::ToggleDiffBase]);
        assert_eq!(blame, vec![CoreEffect::ToggleBlame]);
        assert_eq!(whitespace, vec![CoreEffect::ToggleWhitespaceIgnored]);
    }

    #[test]
    fn shifted_view_key_does_not_request_view_effect() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char('i'),
                InputModifiers {
                    shift: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert!(effects.is_empty());
    }

    #[test]
    fn diff_search_navigation_keys_request_search_effects() {
        let mut engine = CoreInteractionEngine::new();
        let context = InteractionContext {
            diff_search_active: true,
            diff_search_has_matches: true,
            ..InteractionContext::default()
        };

        let next = engine.handle_input(
            key_event(Key::Char('n'), InputModifiers::default()),
            &context,
        );
        let previous = engine.handle_input(
            key_event(
                Key::Char('N'),
                InputModifiers {
                    shift: true,
                    ..Default::default()
                },
            ),
            &context,
        );
        let clear =
            engine.handle_input(key_event(Key::Escape, InputModifiers::default()), &context);

        assert_eq!(
            next,
            vec![CoreEffect::DiffSearch(DiffSearchEffect::NextMatch)]
        );
        assert_eq!(
            previous,
            vec![CoreEffect::DiffSearch(DiffSearchEffect::PreviousMatch)]
        );
        assert_eq!(clear, vec![CoreEffect::DiffSearch(DiffSearchEffect::Clear)]);
    }

    #[test]
    fn inactive_diff_search_keys_do_not_request_search_effects() {
        let mut engine = CoreInteractionEngine::new();

        let next = engine.handle_input(
            key_event(Key::Char('n'), InputModifiers::default()),
            &InteractionContext::default(),
        );
        let clear = engine.handle_input(
            key_event(Key::Escape, InputModifiers::default()),
            &InteractionContext::default(),
        );

        assert!(next.is_empty());
        assert!(clear.is_empty());
    }

    #[test]
    fn control_n_keeps_file_navigation_precedence() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char('n'),
                InputModifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert_eq!(effects, vec![CoreEffect::NavigateFile(Direction::Next)]);
    }

    #[test]
    fn command_prompt_submit_emits_parsed_command() {
        let mut engine = CoreInteractionEngine::new();
        let effects = engine.handle_input(
            key_event(Key::Char(':'), InputModifiers::default()),
            &InteractionContext::default(),
        );
        let id = requested_prompt_id(&effects);

        let effects = engine.handle_input(
            InputEvent::PromptSubmit {
                id,
                value: "gr needle".to_string(),
            },
            &InteractionContext::default(),
        );

        assert_eq!(
            effects,
            vec![
                CoreEffect::ClearPrompt { id },
                CoreEffect::Command(CommandParse::Parsed(command::Command::SearchAll {
                    pattern: "needle".to_string()
                }))
            ]
        );
    }

    #[test]
    fn command_prompt_submit_uses_context_fallback_word() {
        let mut engine = CoreInteractionEngine::new();
        let effects = engine.handle_input(
            key_event(Key::Char(':'), InputModifiers::default()),
            &InteractionContext::default(),
        );
        let id = requested_prompt_id(&effects);

        let effects = engine.handle_input(
            InputEvent::PromptSubmit {
                id,
                value: "gd".to_string(),
            },
            &InteractionContext {
                fallback_word: Some("cursor_symbol".to_string()),
                ..InteractionContext::default()
            },
        );

        assert_eq!(
            effects,
            vec![
                CoreEffect::ClearPrompt { id },
                CoreEffect::Command(CommandParse::Parsed(command::Command::FindDefinition {
                    symbol: "cursor_symbol".to_string()
                }))
            ]
        );
    }

    #[test]
    fn search_prompt_submit_emits_diff_search_effect() {
        let mut engine = CoreInteractionEngine::new();
        let effects = engine.handle_input(
            key_event(Key::Char('/'), InputModifiers::default()),
            &InteractionContext::default(),
        );
        let id = requested_prompt_id(&effects);

        let effects = engine.handle_input(
            InputEvent::PromptSubmit {
                id,
                value: "needle".to_string(),
            },
            &InteractionContext::default(),
        );

        assert_eq!(
            effects,
            vec![
                CoreEffect::ClearPrompt { id },
                CoreEffect::DiffSearch(DiffSearchEffect::Submit {
                    query: "needle".to_string()
                })
            ]
        );
    }

    #[test]
    fn pending_comment_anchor_enter_requests_comment_prompt() {
        let mut engine = CoreInteractionEngine::new();
        let effects = engine.handle_input(
            key_event(Key::Enter, InputModifiers::default()),
            &InteractionContext {
                pending_comment_anchor_active: true,
                ..InteractionContext::default()
            },
        );

        assert_eq!(
            effects,
            vec![CoreEffect::RequestPrompt(PromptRequest {
                id: PromptId(1),
                kind: PromptKind::Comment,
                title: "Comment".to_string(),
                placeholder: Some("Write a comment".to_string()),
                initial_value: String::new(),
            })]
        );
    }

    #[test]
    fn comment_prompt_submit_emits_comment_body_effect() {
        let mut engine = CoreInteractionEngine::new();
        let effects = engine.handle_input(
            key_event(Key::Enter, InputModifiers::default()),
            &InteractionContext {
                pending_comment_anchor_active: true,
                ..InteractionContext::default()
            },
        );
        let id = requested_prompt_id(&effects);

        let effects = engine.handle_input(
            InputEvent::PromptSubmit {
                id,
                value: "needs work".to_string(),
            },
            &InteractionContext::default(),
        );

        assert_eq!(
            effects,
            vec![
                CoreEffect::ClearPrompt { id },
                CoreEffect::Comment(CommentEffect::SubmitBody {
                    body: "needs work".to_string(),
                })
            ]
        );
    }

    #[test]
    fn shift_c_toggles_comments_panel() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(
                Key::Char('C'),
                InputModifiers {
                    shift: true,
                    ..Default::default()
                },
            ),
            &InteractionContext::default(),
        );

        assert_eq!(
            effects,
            vec![CoreEffect::CommentsPanel(CommentsPanelEffect::Toggle)]
        );
    }

    #[test]
    fn brace_keys_navigate_unresolved_comments() {
        let mut engine = CoreInteractionEngine::new();

        let next = engine.handle_input(
            key_event(Key::Char('}'), InputModifiers::default()),
            &InteractionContext::default(),
        );
        let previous = engine.handle_input(
            key_event(Key::Char('{'), InputModifiers::default()),
            &InteractionContext::default(),
        );

        assert_eq!(
            next,
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Next)]
        );
        assert_eq!(
            previous,
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Prev)]
        );
    }

    #[test]
    fn u_requests_undo() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(Key::Char('u'), InputModifiers::default()),
            &InteractionContext::default(),
        );

        assert_eq!(effects, vec![CoreEffect::Undo]);
    }

    #[test]
    fn u_requests_undo_with_current_comment_context() {
        let mut engine = CoreInteractionEngine::new();

        let effects = engine.handle_input(
            key_event(Key::Char('u'), InputModifiers::default()),
            &InteractionContext {
                comments_panel_visible: true,
                focused_pane: Some(PaneId::Comments),
                current_comment_active: true,
                current_comment: Some(CurrentCommentContext {
                    id: 12,
                    body: "resolved feedback".to_string(),
                }),
                ..InteractionContext::default()
            },
        );

        assert_eq!(effects, vec![CoreEffect::Undo]);
    }

    #[test]
    fn current_comment_edit_prompt_submits_update_effect() {
        let mut engine = CoreInteractionEngine::new();
        let effects = engine.handle_input(
            key_event(Key::Char('e'), InputModifiers::default()),
            &InteractionContext {
                current_comment_active: true,
                current_comment: Some(CurrentCommentContext {
                    id: 9,
                    body: "existing".to_string(),
                }),
                ..InteractionContext::default()
            },
        );

        assert_eq!(
            effects,
            vec![CoreEffect::RequestPrompt(PromptRequest {
                id: PromptId(1),
                kind: PromptKind::Comment,
                title: "Edit Comment".to_string(),
                placeholder: Some("Write a comment".to_string()),
                initial_value: "existing".to_string(),
            })]
        );
        let id = requested_prompt_id(&effects);

        let effects = engine.handle_input(
            InputEvent::PromptSubmit {
                id,
                value: "changed".to_string(),
            },
            &InteractionContext::default(),
        );

        assert_eq!(
            effects,
            vec![
                CoreEffect::ClearPrompt { id },
                CoreEffect::Comment(CommentEffect::SubmitEditBody {
                    id: 9,
                    body: "changed".to_string(),
                })
            ]
        );
    }

    #[test]
    fn comment_prompt_cancel_emits_cancel_effect() {
        let mut engine = CoreInteractionEngine::new();
        let effects = engine.handle_input(
            key_event(Key::Enter, InputModifiers::default()),
            &InteractionContext {
                pending_comment_anchor_active: true,
                ..InteractionContext::default()
            },
        );
        let id = requested_prompt_id(&effects);

        let effects = engine.handle_input(
            InputEvent::PromptCancel { id },
            &InteractionContext::default(),
        );

        assert_eq!(
            effects,
            vec![
                CoreEffect::ClearPrompt { id },
                CoreEffect::Comment(CommentEffect::Cancel)
            ]
        );
    }

    #[test]
    fn prompt_cancel_clears_active_prompt() {
        let mut engine = CoreInteractionEngine::new();
        let effects = engine.handle_input(
            key_event(Key::Char(':'), InputModifiers::default()),
            &InteractionContext::default(),
        );
        let id = requested_prompt_id(&effects);

        let effects = engine.handle_input(
            InputEvent::PromptCancel { id },
            &InteractionContext::default(),
        );

        assert_eq!(effects, vec![CoreEffect::ClearPrompt { id }]);
    }
}
