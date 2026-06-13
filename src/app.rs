//! Application state, event loop, and input dispatch.
//!
//! The TUI operates as a client connected to the crt server (persistent or
//! embedded). The event loop is fully async, using crossterm's `EventStream`
//! to receive terminal events without blocking the tokio runtime.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseButton, MouseEvent,
    MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;

use crate::app_update;
use crate::client::Client;
use crate::config::StyleConfig;
use crate::core::command::Command;
use crate::core::diff;
use crate::core::interaction::CoreInteractionEngine;
use crate::core::prompt::PromptId;
use crate::core::review;
use crate::core::search as core_search;
use crate::core::{
    InputEvent, InputModifiers, MouseButton as CoreMouseButton, MouseEvent as CoreMouseEvent,
    MouseEventKind as CoreMouseEventKind, PaneId, PointerSemanticHit, TextAnchor,
};
use crate::model::{
    ConnectionContext, ContentMode, DefinitionLocation, FileEntry, PaneFocus, RenderVariant,
    ReviewStatus, SearchMatch,
};
use crate::tui::{input, render};

// ---------------------------------------------------------------------------
// Input mode
// ---------------------------------------------------------------------------

/// Current input mode for the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputMode {
    /// Normal mode — keys are dispatched as commands.
    Normal,
    /// Command mode — `:` prompt is active, collecting user input.
    Command,
    /// Diff search mode — `/` prompt is active, collecting search query.
    DiffSearch,
}

// ---------------------------------------------------------------------------
// Search results overlay
// ---------------------------------------------------------------------------

/// Search/definition results displayed in an overlay.
#[derive(Debug, Clone)]
pub struct SearchResults {
    /// The query that produced these results.
    pub query: String,
    /// Whether this was a `:grd` (diff-only) search.
    pub diff_only: bool,
    /// Matches from a codebase search.
    pub matches: Vec<SearchMatch>,
    /// Selected index in the results list.
    pub selected: usize,
    /// Scroll offset for the results list.
    pub scroll: usize,
}

/// Definition lookup results.
#[derive(Debug, Clone)]
pub struct DefinitionResults {
    /// The symbol that was looked up.
    pub symbol: String,
    /// Definition locations found.
    pub definitions: Vec<DefinitionLocation>,
    /// Selected index.
    pub selected: usize,
}

// ---------------------------------------------------------------------------
// Jump stack
// ---------------------------------------------------------------------------

/// A saved location for the jump stack (Ctrl-] / Ctrl-t).
#[derive(Debug, Clone)]
pub struct JumpLocation {
    /// Index of the file in the file list.
    pub file_index: usize,
    /// Scroll offset in the diff view.
    pub diff_scroll: usize,
    /// Line cursor position in the diff view.
    pub diff_line_cursor: usize,
    /// Content mode at the time of the jump.
    pub content_mode: ContentMode,
    /// Render variant at the time of the jump.
    pub render_variant: RenderVariant,
}

// ---------------------------------------------------------------------------
// Mouse selection
// ---------------------------------------------------------------------------

/// An in-progress or completed text selection via mouse drag.
#[derive(Debug, Clone)]
pub struct MouseSelection {
    /// Which pane the selection is confined to.
    pub pane: PaneFocus,
    /// Start position in pane semantic text coordinates.
    pub start: TextAnchor,
    /// Current end position in pane semantic text coordinates.
    pub end: TextAnchor,
    /// Set when selection was created by double-click word selection.
    /// The Up event should not re-extract text (it was already copied).
    pub word_selected: bool,
}

impl MouseSelection {
    /// Normalize so start is before end (handles upward/leftward drags).
    pub fn normalized(&self) -> (TextAnchor, TextAnchor) {
        if self.start.line < self.end.line
            || (self.start.line == self.end.line && self.start.column <= self.end.column)
        {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        }
    }
}

/// Last semantic content click, used for double-click detection.
#[derive(Debug, Clone)]
pub struct LastPointerClick {
    pub when: Instant,
    pub pane: PaneFocus,
    pub anchor: TextAnchor,
}

// ---------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------

/// Central application state. Every UI decision reads from here, and input
/// events mutate it (sometimes by sending requests to the server).
pub struct AppState {
    /// Visual styling configuration (loaded from config.toml).
    pub styles: StyleConfig,
    /// Current file list pane width (configurable, resizable by drag).
    pub file_list_width: u16,
    /// Path to the config file (for persisting layout changes).
    pub config_path: Option<std::path::PathBuf>,
    /// Resolved context from the server (repo root, worktree, refs).
    pub context: ConnectionContext,
    /// All files changed in base..HEAD, with their review status and diffs.
    pub files: Vec<FileEntry>,
    /// Index of the currently selected file in `files`.
    pub selected_file: usize,
    /// Which pane has keyboard focus.
    pub pane_focus: PaneFocus,
    /// What content the diff pane shows (diff vs. full file).
    pub content_mode: ContentMode,
    /// How the diff pane content is rendered.
    pub render_variant: RenderVariant,
    /// Vertical scroll offset in the diff view (in lines).
    pub diff_scroll: usize,
    /// Line cursor position in the diff view (0-indexed display row).
    /// Moves with j/k and stays visible within the viewport.
    pub diff_line_cursor: usize,
    /// Column cursor position within the content portion of the current diff
    /// line (0-indexed character offset after gutter and prefix). Resets to 0
    /// when the line cursor moves.
    pub diff_col_cursor: usize,
    /// Whether a reviewed file's diff has been expanded via Enter.
    /// Resets when selected_file changes.
    pub reviewed_diff_expanded: bool,
    /// HEAD version of the selected file (loaded from git on file change).
    pub head_content: Option<String>,
    /// Base version of the selected file (loaded from git on demand).
    pub base_content: Option<String>,
    /// Display row indices where each hunk starts (set during render,
    /// used by `[`/`]` to jump between hunks).
    pub hunk_start_rows: Vec<usize>,
    /// Display row indices where each hunk ends (exclusive, set during render).
    pub hunk_end_rows: Vec<usize>,
    /// Display row of the first actual change (+/-) in each hunk (set during render).
    /// Used by `]`/`[` to place the cursor on the context line just above the change.
    pub hunk_first_change_rows: Vec<usize>,
    /// Number of columns occupied by line-number gutters in the diff pane
    /// (set during render). Used to exclude gutters from mouse selection.
    pub diff_gutter_cols: usize,
    /// Total lines of content in the diff pane (set during render).
    pub diff_content_height: usize,
    /// Visible lines in the diff pane (set during render).
    pub diff_view_height: usize,
    /// Whether inline comments are visible in the diff pane.
    pub show_comments: bool,
    /// Whether the file list pane is visible.
    pub show_file_list: bool,
    /// Whether the diff pane is visible.
    pub show_diff_pane: bool,
    /// Active mouse text selection, if any.
    pub mouse_selection: Option<MouseSelection>,
    /// Mouse down anchor used to start a drag selection only after the pointer
    /// actually moves.
    pub mouse_down_anchor: Option<(PaneFocus, TextAnchor)>,
    /// Plain text of rendered diff lines (set during render, for clipboard).
    pub diff_rendered_text: Vec<String>,
    /// Plain text of rendered file list lines (set during render, for clipboard).
    pub file_list_rendered_text: Vec<String>,
    /// Mapping from display row to file index (set during render).
    /// Used for mouse click to select a file. Rows that are headers map to `None`.
    pub file_list_row_to_file: Vec<Option<usize>>,
    /// Vertical scroll offset in the file list (in display rows).
    pub file_list_scroll: usize,
    /// Screen area of the file list pane (set during render, for mouse hit testing).
    pub file_list_area: Rect,
    /// Screen area of the diff pane (set during render, for mouse hit testing).
    pub diff_area: Rect,
    /// Transient status bar message (e.g. "Press Ctrl-C again to quit").
    /// Cleared after a timeout or on next keypress.
    pub status_message: Option<(String, Instant)>,
    /// Last semantic content click, for double-click detection.
    pub last_click: Option<LastPointerClick>,
    /// Whether the help overlay is visible.
    pub show_help: bool,
    /// Current diff algorithm.
    pub diff_algorithm: crate::config::DiffAlgorithm,
    /// The default diff algorithm (from git config or fallback). Used to
    /// decide whether to show the algorithm in the title bar — it's only
    /// shown when the user has cycled away from the default.
    pub default_diff_algorithm: crate::config::DiffAlgorithm,
    /// Whether to ignore whitespace differences in the diff.
    pub ignore_whitespace: bool,
    /// Whether blame annotations are visible.
    pub show_blame: bool,
    /// Blame data for the HEAD version of the current file (line 1 = index 0).
    pub head_blame: Vec<crate::git::BlameLine>,
    /// Blame data for the base version of the current file.
    pub base_blame: Vec<crate::git::BlameLine>,
    /// Set by the key handler when `r` is pressed. The async event loop
    /// picks this up and calls the server.
    pub pending_review_toggle: bool,
    /// True while the user is dragging the file list / diff pane border.
    pub dragging_border: bool,
    /// Set to true to suspend the process (Ctrl-Z).
    pub should_suspend: bool,
    /// Set to true to exit the event loop.
    pub should_quit: bool,
    /// Set to true when the terminal regains focus — triggers a full
    /// file list reload on the next event loop iteration.
    pub pending_refresh: bool,
    /// Cached diff content — avoids rebuilding all `Line<'static>` on every
    /// frame when only the scroll offset changed.
    pub diff_cache: Option<crate::tui::render::diff_view::DiffCache>,
    /// Current input mode (Normal vs Command).
    pub input_mode: InputMode,
    /// Command-mode input buffer (the text after `:`).
    pub command_input: String,
    /// Cursor position within `command_input`.
    pub command_cursor: usize,
    /// Active search results overlay, if any.
    pub search_results: Option<SearchResults>,
    /// Active definition results overlay, if any.
    pub definition_results: Option<DefinitionResults>,
    /// Jump stack for Ctrl-] / Ctrl-t navigation.
    pub jump_stack: Vec<JumpLocation>,
    /// Pending command to execute asynchronously (set by key handler,
    /// processed by async event loop).
    pub pending_command: Option<Command>,
    /// Active diff search query (the confirmed search term).
    pub diff_search_query: Option<String>,
    /// In-progress diff search input (while typing in `/` prompt).
    pub diff_search_input: String,
    /// Cursor position within `diff_search_input`.
    pub diff_search_cursor: usize,
    /// Cached match positions: (display_row, byte_start, byte_end) relative
    /// to `diff_rendered_text`. Recomputed when query or content changes.
    pub diff_search_matches: Vec<(usize, usize, usize)>,
    /// Index of the currently focused match in `diff_search_matches`.
    pub diff_search_current: usize,
    /// When true, force diffs to use merge_base even for reviewed files.
    /// Toggled by the `m` keybinding.
    pub show_merge_base: bool,
    /// Core interaction entrypoint used by the TUI adapter for migrated input.
    pub core_interaction: CoreInteractionEngine,
    /// Active core prompt requested by the interaction engine, if any.
    pub active_core_prompt: Option<PromptId>,
}

impl AppState {
    fn new(
        styles: StyleConfig,
        file_list_width: u16,
        diff_algorithm: crate::config::DiffAlgorithm,
        config_path: Option<std::path::PathBuf>,
        context: ConnectionContext,
        mut files: Vec<FileEntry>,
    ) -> Self {
        review::sort_files(&mut files);

        Self {
            styles,
            file_list_width,
            config_path,
            context,
            files,
            // Index 0 is the first unreviewed file (due to sort order).
            selected_file: 0,
            pane_focus: PaneFocus::FileList,
            content_mode: ContentMode::Diff,
            render_variant: RenderVariant::Inline,
            diff_scroll: 0,
            diff_line_cursor: 0,
            diff_col_cursor: 0,
            reviewed_diff_expanded: false,
            head_content: None,
            base_content: None,
            hunk_start_rows: Vec::new(),
            hunk_end_rows: Vec::new(),
            hunk_first_change_rows: Vec::new(),
            diff_gutter_cols: 0,
            diff_content_height: 0,
            diff_view_height: 0,
            show_comments: false,
            show_file_list: true,
            show_diff_pane: true,
            mouse_selection: None,
            mouse_down_anchor: None,
            diff_rendered_text: Vec::new(),
            file_list_rendered_text: Vec::new(),
            file_list_row_to_file: Vec::new(),
            file_list_scroll: 0,
            file_list_area: Rect::default(),
            diff_area: Rect::default(),
            status_message: None,
            last_click: None,
            show_help: false,
            diff_algorithm,
            default_diff_algorithm: diff_algorithm,
            ignore_whitespace: false,
            show_blame: false,
            head_blame: Vec::new(),
            base_blame: Vec::new(),
            pending_review_toggle: false,
            dragging_border: false,
            should_suspend: false,
            should_quit: false,
            pending_refresh: false,
            diff_cache: None,
            input_mode: InputMode::Normal,
            command_input: String::new(),
            command_cursor: 0,
            search_results: None,
            definition_results: None,
            jump_stack: Vec::new(),
            pending_command: None,
            diff_search_query: None,
            diff_search_input: String::new(),
            diff_search_cursor: 0,
            diff_search_matches: Vec::new(),
            diff_search_current: 0,
            show_merge_base: false,
            core_interaction: CoreInteractionEngine::new(),
            active_core_prompt: None,
        }
    }

    /// The currently selected file, if any.
    pub fn selected_file_entry(&self) -> Option<&FileEntry> {
        self.files.get(self.selected_file)
    }

    /// Return the effective diff base for the currently selected file.
    ///
    /// If the file has been reviewed and `show_merge_base` is false, use the
    /// reviewed commit so the diff only shows changes since the review.
    /// Otherwise fall back to the merge base.
    pub fn effective_diff_base(&self) -> &str {
        review::effective_diff_base(
            self.selected_file_entry(),
            &self.context.merge_base,
            self.show_merge_base,
        )
    }

    /// Number of unreviewed files (Unreviewed + Changed status).
    /// Since files are sorted unreviewed-first, these are files[0..count].
    pub fn unreviewed_count(&self) -> usize {
        review::unreviewed_count(&self.files)
    }

    /// Maximum scroll offset: the last line sits at the top of the viewport.
    pub fn max_diff_scroll(&self) -> usize {
        self.diff_content_height.saturating_sub(1)
    }

    /// Load content for the currently selected file from the working tree.
    pub fn load_head_content(&mut self) {
        self.head_content = None;
        if let Some(entry) = self.files.get(self.selected_file) {
            let path = entry.change.path.clone();
            self.head_content = diff::workdir_file_content(&self.context.worktree, &path);
        }
    }

    /// Load base content for the currently selected file from git.
    pub fn load_base_content(&mut self) {
        self.base_content = None;
        if let Some(entry) = self.files.get(self.selected_file) {
            let path = entry.change.path.clone();
            self.base_content =
                diff::file_content(&self.context.worktree, &self.context.merge_base, &path);
        }
    }

    /// Which hunk (0-indexed) the cursor line is inside or nearest to.
    /// Returns the hunk containing the cursor, or the next hunk if the
    /// cursor is on context lines between hunks. Returns None only if
    /// the cursor is after all hunks.
    pub fn current_hunk_index(&self) -> Option<usize> {
        let cursor = self.diff_line_cursor;
        for (i, (&start, &end)) in self
            .hunk_start_rows
            .iter()
            .zip(&self.hunk_end_rows)
            .enumerate()
        {
            // Cursor is inside this hunk.
            if cursor >= start && cursor < end {
                return Some(i);
            }
            // Cursor is before this hunk (in context lines above it).
            if cursor < start {
                return Some(i);
            }
        }
        // Cursor is after the last hunk.
        if !self.hunk_start_rows.is_empty() {
            Some(self.hunk_start_rows.len() - 1)
        } else {
            None
        }
    }

    /// Clamp `diff_scroll` to the valid range.
    pub fn clamp_diff_scroll(&mut self) {
        self.diff_scroll = self.diff_scroll.min(self.max_diff_scroll());
    }

    /// Clamp `diff_line_cursor` to valid range and adjust scroll to keep
    /// the cursor visible in the viewport.
    pub fn clamp_cursor_and_scroll(&mut self) {
        let max = self.max_diff_scroll();
        self.diff_line_cursor = self.diff_line_cursor.min(max);
        // Scroll up if cursor is above the viewport.
        if self.diff_line_cursor < self.diff_scroll {
            self.diff_scroll = self.diff_line_cursor;
        }
        // Scroll down if cursor is below the viewport.
        if self.diff_view_height > 0
            && self.diff_line_cursor >= self.diff_scroll + self.diff_view_height
        {
            self.diff_scroll = self
                .diff_line_cursor
                .saturating_sub(self.diff_view_height - 1);
        }
        self.clamp_diff_scroll();
    }

    /// Determine which pane a terminal coordinate falls in.
    fn pane_at(&self, col: u16, row: u16) -> Option<PaneFocus> {
        if self.show_file_list && self.file_list_area.contains((col, row).into()) {
            Some(PaneFocus::FileList)
        } else if self.show_diff_pane && self.diff_area.contains((col, row).into()) {
            Some(PaneFocus::Diff)
        } else {
            None
        }
    }

    fn input_event_from_mouse(&self, mouse: &MouseEvent) -> Option<InputEvent> {
        let (kind, button) = match mouse.kind {
            MouseEventKind::Down(button) => {
                (CoreMouseEventKind::Down, Some(core_mouse_button(button)?))
            }
            MouseEventKind::Up(button) => {
                (CoreMouseEventKind::Up, Some(core_mouse_button(button)?))
            }
            MouseEventKind::Drag(button) => {
                (CoreMouseEventKind::Drag, Some(core_mouse_button(button)?))
            }
            MouseEventKind::Moved => (CoreMouseEventKind::Move, None),
            MouseEventKind::ScrollDown => (CoreMouseEventKind::ScrollDown, None),
            MouseEventKind::ScrollUp => (CoreMouseEventKind::ScrollUp, None),
            _ => return None,
        };

        let pane = self.pane_at(mouse.column, mouse.row);
        let local_pos = pane.map(|pane| {
            let area = self.area_for_pane(pane);
            (
                mouse.column.saturating_sub(area.x),
                mouse.row.saturating_sub(area.y),
            )
        });

        Some(InputEvent::Mouse(CoreMouseEvent {
            kind,
            button,
            local_pos,
            semantic_hit: pane.and_then(|pane| self.pointer_semantic_hit(pane, mouse)),
            modifiers: InputModifiers {
                ctrl: mouse.modifiers.contains(event::KeyModifiers::CONTROL),
                alt: mouse.modifiers.contains(event::KeyModifiers::ALT),
                shift: mouse.modifiers.contains(event::KeyModifiers::SHIFT),
            },
        }))
    }

    fn area_for_pane(&self, pane: PaneFocus) -> Rect {
        match pane {
            PaneFocus::FileList => self.file_list_area,
            PaneFocus::Diff => self.diff_area,
        }
    }

    fn pointer_semantic_hit(
        &self,
        pane: PaneFocus,
        mouse: &MouseEvent,
    ) -> Option<PointerSemanticHit> {
        let pane_id = match pane {
            PaneFocus::FileList => PaneId::FileList,
            PaneFocus::Diff => PaneId::Diff,
        };

        let text_anchor = self.pointer_text_anchor_for_pane(pane, mouse.column, mouse.row, false);

        Some(PointerSemanticHit {
            pane_id,
            region_id: self.pointer_region_id(pane, text_anchor),
            text_anchor,
        })
    }

    fn pointer_text_anchor_for_pane(
        &self,
        pane: PaneFocus,
        column: u16,
        row: u16,
        clamp: bool,
    ) -> Option<TextAnchor> {
        let area = self.area_for_pane(pane);
        let inner_top = area.y.saturating_add(1);
        let inner_left = area.x.saturating_add(1);
        let inner_right = area.right().saturating_sub(1);
        let inner_bottom = area.bottom().saturating_sub(1);

        if inner_top >= inner_bottom {
            return None;
        }

        let row = if clamp {
            row.clamp(inner_top, inner_bottom.saturating_sub(1))
        } else if row >= inner_top && row < inner_bottom {
            row
        } else {
            return None;
        };

        let column = if clamp && inner_left < inner_right {
            column.clamp(inner_left, inner_right.saturating_sub(1))
        } else {
            column
        };

        match pane {
            PaneFocus::FileList => Some(TextAnchor {
                line: self
                    .file_list_scroll
                    .saturating_add((row - inner_top) as usize),
                column: column.saturating_sub(inner_left) as usize,
            }),
            PaneFocus::Diff => Some(TextAnchor {
                line: self.diff_scroll.saturating_add((row - inner_top) as usize),
                column: (column as usize)
                    .saturating_sub(inner_left as usize + self.diff_content_start_col()),
            }),
        }
    }

    fn pointer_region_id(
        &self,
        pane: PaneFocus,
        text_anchor: Option<TextAnchor>,
    ) -> Option<String> {
        let anchor = text_anchor?;
        match pane {
            PaneFocus::FileList => match self.file_list_row_to_file.get(anchor.line) {
                Some(Some(file_idx)) => self
                    .files
                    .get(*file_idx)
                    .map(|entry| format!("file:{}", entry.change.path)),
                _ => Some(format!("file-list-row:{}", anchor.line)),
            },
            PaneFocus::Diff => Some(format!("diff-line:{}", anchor.line)),
        }
    }

    /// Load blame data for the currently selected file.
    pub fn load_blame(&mut self) {
        self.head_blame.clear();
        self.base_blame.clear();

        if let Some(entry) = self.files.get(self.selected_file) {
            let path = entry.change.path.clone();
            let (head, base) = diff::blame_pair(
                &self.context.worktree,
                &self.context.merge_base,
                &path,
                self.show_blame,
            );
            self.head_blame = head;
            self.base_blame = base;
        }
    }

    /// Refresh the diff for the currently selected file from the working tree,
    /// using the current diff algorithm and whitespace settings. Updates the
    /// cached diff in place without changing cursor position.
    fn refresh_current_file_diff(&mut self) {
        if let Some(entry) = self.files.get(self.selected_file) {
            let path = entry.change.path.clone();
            let diff_base = self.effective_diff_base().to_string();
            let merge_base = self.context.merge_base.clone();
            if let Some(diff) = diff::diff_with_fallback(
                &self.context.worktree,
                &diff_base,
                &merge_base,
                &path,
                self.diff_algorithm,
                self.ignore_whitespace,
            ) {
                if let Some(entry) = self.files.get_mut(self.selected_file) {
                    entry.diff = diff;
                }
            }
        }
        // Invalidate the diff cache so the view rebuilds.
        self.diff_cache = None;
    }

    /// Reload the diff for the currently selected file, respecting
    /// the `ignore_whitespace` flag.
    pub fn reload_current_diff(&mut self) {
        if let Some(entry) = self.files.get(self.selected_file) {
            let path = entry.change.path.clone();
            let diff_base = self.effective_diff_base().to_string();
            let merge_base = self.context.merge_base.clone();
            if let Some(diff) = diff::diff_with_fallback(
                &self.context.worktree,
                &diff_base,
                &merge_base,
                &path,
                self.diff_algorithm,
                self.ignore_whitespace,
            ) {
                if let Some(entry) = self.files.get_mut(self.selected_file) {
                    entry.diff = diff;
                }
            }
        }
        self.diff_scroll = self
            .selected_file_entry()
            .and_then(|e| e.diff.hunks.first())
            .map(|h| (h.new_start as usize).saturating_sub(1))
            .unwrap_or(0);
    }

    /// Number of content columns on the diff gutter + prefix.
    /// The prefix is " + ", " - ", or "   " (3 chars) appended after
    /// `diff_gutter_cols`.
    pub fn diff_content_start_col(&self) -> usize {
        if self.diff_gutter_cols > 0 {
            self.diff_gutter_cols + 3
        } else {
            0
        }
    }

    /// Get the content portion of the current cursor line (after gutter+prefix),
    /// or empty string if out of bounds. Includes trailing padding.
    pub fn current_line_content(&self) -> &str {
        let line = match self.diff_rendered_text.get(self.diff_line_cursor) {
            Some(l) => l.as_str(),
            None => return "",
        };
        let start = self.diff_content_start_col().min(line.len());
        &line[start..]
    }

    /// Get the content portion of a specific line (after gutter+prefix),
    /// with trailing whitespace stripped. Returns empty string if out of bounds.
    pub fn line_content_trimmed(&self, row: usize) -> &str {
        let line = match self.diff_rendered_text.get(row) {
            Some(l) => l.as_str(),
            None => return "",
        };
        let start = self.diff_content_start_col().min(line.len());
        line[start..].trim_end()
    }

    /// Length of the actual text content of the current line in characters
    /// (trailing padding stripped).
    pub fn current_line_text_len(&self) -> usize {
        self.line_content_trimmed(self.diff_line_cursor)
            .chars()
            .count()
    }

    /// Clamp the column cursor to the valid range for the current line's
    /// actual text (not padding).
    pub fn clamp_col_cursor(&mut self) {
        let max = self.current_line_text_len().saturating_sub(1);
        self.diff_col_cursor = self.diff_col_cursor.min(max);
    }

    /// Recompute diff search matches from the current query and rendered text.
    /// The query is treated as a regex (case-insensitive). Returns an error
    /// message if the regex is invalid.
    pub fn recompute_diff_search_matches(&mut self) -> Option<String> {
        self.diff_search_matches.clear();
        self.diff_search_current = 0;
        let query = match &self.diff_search_query {
            Some(q) if !q.is_empty() => q.clone(),
            _ => return None,
        };
        let re = match regex::RegexBuilder::new(&query)
            .case_insensitive(true)
            .build()
        {
            Ok(re) => re,
            Err(e) => {
                // Strip the verbose prefix regex puts on errors.
                let msg = e.to_string();
                let short = msg
                    .lines()
                    .next()
                    .unwrap_or(&msg)
                    .trim_start_matches("regex parse error:")
                    .trim();
                return Some(format!("Invalid regex: {short}"));
            }
        };
        for (row, line) in self.diff_rendered_text.iter().enumerate() {
            // Skip gutter columns so we only match content.
            let gutter = self.diff_gutter_cols;
            let search_start = gutter.min(line.len());
            let content = &line[search_start..];
            for m in re.find_iter(content) {
                // Skip zero-length matches to avoid infinite loops.
                if m.start() == m.end() {
                    continue;
                }
                let abs_start = search_start + m.start();
                let abs_end = search_start + m.end();
                self.diff_search_matches.push((row, abs_start, abs_end));
            }
        }
        None
    }

    /// Jump to the next diff search match at or after the cursor.
    pub fn diff_search_jump_to_current(&mut self) {
        if self.diff_search_matches.is_empty() {
            return;
        }
        // Find the first match at or after the current cursor line.
        let idx = self
            .diff_search_matches
            .iter()
            .position(|(row, _, _)| *row >= self.diff_line_cursor)
            .unwrap_or(0);
        self.diff_search_current = idx;
        let (row, _, _) = self.diff_search_matches[idx];
        self.diff_line_cursor = row;
        self.clamp_cursor_and_scroll();
    }

    /// Called after `selected_file` changes. Resets diff state and loads
    /// the appropriate file content from the working tree.
    pub fn on_file_changed(&mut self) {
        self.reviewed_diff_expanded = false;
        self.hunk_start_rows.clear();
        self.hunk_end_rows.clear();
        self.hunk_first_change_rows.clear();
        // Refresh the diff for this file from the working tree.
        self.refresh_current_file_diff();
        self.load_head_content();
        self.load_blame();

        // Reload base content if we're currently in base view.
        if self.content_mode == ContentMode::FullFile
            && self.render_variant == RenderVariant::BaseVersion
        {
            self.load_base_content();
        } else {
            self.base_content = None;
        }

        // Place cursor and scroll at the first hunk.
        let first_hunk_row = self
            .selected_file_entry()
            .and_then(|e| e.diff.hunks.first())
            .map(|h| (h.new_start as usize).saturating_sub(1))
            .unwrap_or(0);
        self.diff_line_cursor = first_hunk_row;
        self.diff_col_cursor = 0;
        self.diff_scroll = first_hunk_row;
    }
}

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

/// The TUI application. Owns the terminal, client connection, and state.
pub struct App {
    pub state: AppState,
    client: Client,
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl App {
    /// Create a new application, loading initial data from the server.
    pub async fn new(client: Client, context: ConnectionContext) -> Result<Self> {
        let cfg = crate::config::load();

        // Load file list from server.
        let files = match client.list_changed_files().await {
            Ok(result) => result.files,
            Err(e) => {
                // If the server method is unavailable, start with empty list.
                eprintln!("Warning: could not load files: {e}");
                Vec::new()
            }
        };

        // Resolve diff algorithm: crt config → git config → patience.
        let diff_algorithm =
            diff::resolve_diff_algorithm(&context.worktree, cfg.layout.diff_algorithm);

        let config_path = crate::config::config_path();
        let mut state = AppState::new(
            cfg.style,
            cfg.layout.file_list_width,
            diff_algorithm,
            config_path,
            context,
            files,
        );
        // Refresh the first file's diff using the correct base (e.g.
        // reviewed_commit for previously-reviewed files).  The server always
        // computes diffs from merge_base, so we recompute locally here.
        state.on_file_changed();
        let terminal = setup_terminal().context("Failed to set up terminal")?;

        Ok(Self {
            state,
            client,
            terminal,
        })
    }

    /// Run the event loop. Returns when the user quits.
    pub async fn run(&mut self) -> Result<()> {
        // Install a panic hook that restores the terminal before printing
        // the panic message, so the user's shell isn't left broken.
        let original_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = restore_terminal_raw();
            original_hook(info);
        }));

        let result = self.event_loop().await;

        // Restore terminal regardless of how the loop exited.
        let _ = restore_terminal(&mut self.terminal);

        // Remove our panic hook.
        let _ = std::panic::take_hook();

        result
    }

    /// Async event loop: render → wait for event → dispatch → repeat.
    ///
    /// Terminal events are polled on a blocking thread and sent over an
    /// async channel, keeping the tokio runtime responsive for server
    /// calls and notifications.
    async fn event_loop(&mut self) -> Result<()> {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<Event>();

        // Spawn a blocking task that polls crossterm for events and
        // forwards them to the async channel.
        let poll_handle = tokio::task::spawn_blocking(move || {
            loop {
                // Poll with a short timeout so we can check if the channel
                // is closed (receiver dropped).
                if event::poll(Duration::from_millis(50)).unwrap_or(false) {
                    if let Ok(ev) = event::read() {
                        if event_tx.send(ev).is_err() {
                            break; // receiver dropped, exit
                        }
                    }
                }
                // Check if receiver is still alive.
                if event_tx.is_closed() {
                    break;
                }
            }
        });

        loop {
            // Render current state.
            let state = &mut self.state;
            self.terminal.draw(|frame| render::draw(frame, state))?;

            // Wait for the next terminal event, with a timeout so that
            // transient status messages get cleared by re-rendering.
            let ev = if self.state.status_message.is_some() {
                match tokio::time::timeout(Duration::from_secs(1), event_rx.recv()).await {
                    Ok(Some(ev)) => ev,
                    Ok(None) => break,  // channel closed
                    Err(_) => continue, // timeout — re-render to clear message
                }
            } else {
                match event_rx.recv().await {
                    Some(ev) => ev,
                    None => break,
                }
            };
            self.handle_event(ev);

            // Drain any additional queued events before re-rendering.
            while let Ok(ev) = event_rx.try_recv() {
                self.handle_event(ev);
            }

            // Process pending review toggle.
            if self.state.pending_review_toggle {
                self.state.pending_review_toggle = false;
                self.process_review_toggle().await;
            }

            // Process pending command (search, definition, etc.).
            if self.state.pending_command.is_some() {
                let cmd = self.state.pending_command.take().unwrap();
                self.process_pending_command(cmd).await;
            }

            // Check for server-pushed notifications (from other clients).
            self.process_notifications().await;

            // Refresh file list on focus gain.
            if self.state.pending_refresh {
                self.state.pending_refresh = false;
                self.reload_file_list().await;
            }

            if self.state.should_suspend {
                self.state.should_suspend = false;
                self.suspend()?;
            }

            if self.state.should_quit {
                break;
            }
        }

        // Clean up the polling task.
        drop(event_rx);
        let _ = poll_handle.await;

        Ok(())
    }

    /// Dispatch a single terminal event.
    fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                // Any keypress clears mouse selection.
                self.state.mouse_selection = None;
                self.state.mouse_down_anchor = None;
                input::handle_key_event(&mut self.state, key);
            }
            Event::Mouse(mouse) => {
                self.handle_mouse_event(mouse);
            }
            Event::Resize(_w, _h) => {
                // ratatui handles resize automatically on next draw.
            }
            Event::FocusGained => {
                // Terminal regained focus — schedule a full refresh.
                self.state.pending_refresh = true;
            }
            _ => {}
        }
    }

    /// Suspend the process: restore the terminal, send SIGTSTP, then
    /// re-setup the terminal when the user resumes with `fg`.
    fn suspend(&mut self) -> Result<()> {
        restore_terminal(&mut self.terminal)?;

        #[cfg(unix)]
        {
            // SAFETY: raise() is safe to call with a valid signal number.
            unsafe {
                libc::raise(libc::SIGTSTP);
            }
        }

        // When the user runs `fg`, execution resumes here.
        self.terminal = setup_terminal().context("Failed to re-setup terminal after resume")?;
        Ok(())
    }

    /// Check if a mouse column is on the border between file list and diff panes.
    fn is_on_pane_border(&self, col: u16, row: u16) -> bool {
        if !self.state.show_file_list || !self.state.show_diff_pane {
            return false;
        }
        let border_col = self.state.file_list_area.right().saturating_sub(1);
        col == border_col
            && row >= self.state.file_list_area.y
            && row < self.state.file_list_area.bottom()
    }

    /// Persist the current file list width to the config file.
    fn save_file_list_width(&self) {
        if let Some(path) = &self.state.config_path {
            let layout = crate::config::LayoutConfig {
                file_list_width: self.state.file_list_width,
                diff_algorithm: Some(self.state.diff_algorithm),
            };
            crate::config::save_layout(path, &layout);
        }
    }

    /// Handle mouse events: selection, scroll wheel, border drag.
    fn handle_mouse_event(&mut self, mouse: MouseEvent) {
        let input_event = self.state.input_event_from_mouse(&mouse);
        let semantic_content_hit = input_event.as_ref().and_then(mouse_content_hit);
        let pending_core_effects = input_event
            .map(|event| {
                self.state
                    .core_interaction
                    .handle_input(event, &Default::default())
            })
            .unwrap_or_default();

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Check if the user clicked on the pane border to start resizing.
                if self.is_on_pane_border(mouse.column, mouse.row) {
                    self.state.dragging_border = true;
                    return;
                }

                // Double-click detection: select the word under cursor.
                let is_double_click = semantic_content_hit.is_some_and(|(pane, anchor)| {
                    self.state.last_click.as_ref().is_some_and(|last| {
                        last.when.elapsed() < Duration::from_millis(400)
                            && last.pane == pane
                            && last.anchor == anchor
                    })
                });
                self.state.last_click =
                    semantic_content_hit.map(|(pane, anchor)| LastPointerClick {
                        when: Instant::now(),
                        pane,
                        anchor,
                    });

                if is_double_click {
                    // Clear any selection from the first click so the
                    // subsequent Up event doesn't overwrite the clipboard.
                    self.state.mouse_selection = None;
                    self.state.mouse_down_anchor = None;
                    if let Some((pane, anchor)) = semantic_content_hit {
                        if pane == PaneFocus::FileList {
                            self.copy_file_path_at(anchor.line);
                        } else {
                            self.select_word_at(pane, anchor);
                        }
                    }
                    self.state.last_click = None; // prevent triple-click
                    return; // skip drag selection setup
                }

                app_update::apply_core_effects(&mut self.state, pending_core_effects);

                if let Some((pane, anchor)) = semantic_content_hit {
                    // Record mouse-down anchor; drag starts selection.
                    self.state.mouse_selection = None;
                    self.state.mouse_down_anchor = Some((pane, anchor));
                }
            }
            _ => {
                if app_update::apply_core_effects(&mut self.state, pending_core_effects) {
                    return;
                }

                match mouse.kind {
                    MouseEventKind::Drag(MouseButton::Left) => {
                        if self.state.dragging_border {
                            // Resize the file list pane. Minimum 10, maximum
                            // terminal width minus 20.
                            let min_w = 10u16;
                            let max_w =
                                self.state.file_list_area.width + self.state.diff_area.width - 20;
                            let new_width = (mouse.column + 1).clamp(min_w, max_w);
                            self.state.file_list_width = new_width;
                            return;
                        }
                        let drag_content_hit = semantic_content_hit.or_else(|| {
                            let (pane, _) = self.state.mouse_down_anchor?;
                            self.state
                                .pointer_text_anchor_for_pane(pane, mouse.column, mouse.row, true)
                                .map(|anchor| (pane, anchor))
                        });

                        // Extend the selection in semantic coordinates, clamped to
                        // the originating pane when the pointer leaves its content.
                        if let Some((pane, anchor)) = drag_content_hit {
                            if let Some(sel) = &mut self.state.mouse_selection {
                                if sel.pane == pane {
                                    sel.end = anchor;
                                }
                            } else if let Some((start_pane, start)) = self.state.mouse_down_anchor {
                                if start_pane == pane {
                                    self.state.mouse_selection = Some(MouseSelection {
                                        pane,
                                        start,
                                        end: anchor,
                                        word_selected: false,
                                    });
                                }
                            }
                        }
                    }
                    MouseEventKind::Up(MouseButton::Left) => {
                        if self.state.dragging_border {
                            self.state.dragging_border = false;
                            self.save_file_list_width();
                            return;
                        }
                        self.state.mouse_down_anchor = None;
                        // Finish selection: extract text and copy to clipboard.
                        if let Some(sel) = self.state.mouse_selection.take() {
                            if !sel.word_selected {
                                // Only re-extract for drag selections — word selections
                                // were already copied by select_word_at().
                                let text = extract_selected_text(&self.state, &sel);
                                if !text.is_empty() {
                                    copy_to_clipboard(&text);
                                }
                            }
                            // Keep the selection visible until next keypress.
                            self.state.mouse_selection = Some(sel);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// Toggle the review status of the currently selected file.
    async fn process_review_toggle(&mut self) {
        let entry = match self.state.files.get(self.state.selected_file) {
            Some(e) => e,
            None => return,
        };

        let path = entry.change.path.clone();
        let is_reviewed = matches!(entry.status, ReviewStatus::Reviewed { .. });

        let result = if is_reviewed {
            self.client.unmark_reviewed(&path).await
        } else {
            self.client.mark_reviewed(&path).await
        };

        match result {
            Ok(action_result) => self.apply_review_result(&action_result),
            Err(e) => {
                let verb = if is_reviewed { "unmark" } else { "mark" };
                self.state.status_message =
                    Some((format!("Failed to {verb} reviewed: {e}"), Instant::now()));
            }
        }
    }

    /// Process a pending command set by the key handler.
    async fn process_pending_command(&mut self, cmd: Command) {
        match cmd {
            Command::SearchAll { pattern } => {
                self.state.status_message = Some(("Searching...".to_string(), Instant::now()));
                // Force a re-render so the user sees the searching message.
                let state = &mut self.state;
                let _ = self.terminal.draw(|frame| render::draw(frame, state));

                match self.client.search_codebase(&pattern, "all").await {
                    Ok(result) => match core_search::search_outcome(&pattern, false, result) {
                        core_search::SearchOutcome::NoMatches => {
                            self.state.status_message =
                                Some((format!("No matches for /{pattern}/"), Instant::now()));
                        }
                        core_search::SearchOutcome::ShowResults {
                            query,
                            diff_only,
                            matches,
                        } => {
                            self.state.search_results = Some(SearchResults {
                                query,
                                diff_only,
                                matches,
                                selected: 0,
                                scroll: 0,
                            });
                            self.state.status_message = None;
                        }
                    },
                    Err(e) => {
                        self.state.status_message =
                            Some((format!("Search error: {e}"), Instant::now()));
                    }
                }
            }
            Command::SearchDiff { pattern } => {
                self.state.status_message =
                    Some(("Searching diff files...".to_string(), Instant::now()));
                let state = &mut self.state;
                let _ = self.terminal.draw(|frame| render::draw(frame, state));

                match self.client.search_codebase(&pattern, "diff").await {
                    Ok(result) => match core_search::search_outcome(&pattern, true, result) {
                        core_search::SearchOutcome::NoMatches => {
                            self.state.status_message = Some((
                                format!("No matches for /{pattern}/ in diff"),
                                Instant::now(),
                            ));
                        }
                        core_search::SearchOutcome::ShowResults {
                            query,
                            diff_only,
                            matches,
                        } => {
                            self.state.search_results = Some(SearchResults {
                                query,
                                diff_only,
                                matches,
                                selected: 0,
                                scroll: 0,
                            });
                            self.state.status_message = None;
                        }
                    },
                    Err(e) => {
                        self.state.status_message =
                            Some((format!("Search error: {e}"), Instant::now()));
                    }
                }
            }
            Command::FindDefinition { symbol } => {
                self.state.status_message =
                    Some(("Finding definition...".to_string(), Instant::now()));
                let state = &mut self.state;
                let _ = self.terminal.draw(|frame| render::draw(frame, state));

                let context_file = self
                    .state
                    .selected_file_entry()
                    .map(|e| e.change.path.clone());
                match self
                    .client
                    .find_definition(&symbol, context_file.as_deref())
                    .await
                {
                    Ok(result) => {
                        match core_search::definition_outcome(&symbol, result, &self.state.files) {
                            core_search::DefinitionOutcome::NoDefinitions => {
                                self.state.status_message = Some((
                                    format!("No definitions found for '{symbol}'"),
                                    Instant::now(),
                                ));
                            }
                            core_search::DefinitionOutcome::Navigate(target) => {
                                self.state.status_message = None;
                                navigate_to_location_from_app(&mut self.state, target);
                            }
                            core_search::DefinitionOutcome::ShowResults {
                                symbol,
                                definitions,
                            } => {
                                self.state.definition_results = Some(DefinitionResults {
                                    symbol,
                                    definitions,
                                    selected: 0,
                                });
                                self.state.status_message = None;
                            }
                        }
                    }
                    Err(e) => {
                        self.state.status_message =
                            Some((format!("Definition error: {e}"), Instant::now()));
                    }
                }
            }
            Command::ViewFile { path, line_number } => {
                // view_file <path> <line>
                // For now, show a status message since read-only view
                // for non-diff files would require a separate content mode.
                self.state.status_message = Some((
                    format!("File not in diff: {path} {line_number}"),
                    Instant::now(),
                ));
            }
            Command::Quit
            | Command::SetBlame(_)
            | Command::SetComments(_)
            | Command::SetWhitespaceIgnored(_)
            | Command::Unknown { .. } => {
                self.state.status_message =
                    Some(("Unsupported pending command".to_string(), Instant::now()));
            }
        }
    }

    /// Check for server-pushed notifications and refresh state if needed.
    async fn process_notifications(&mut self) {
        let notifications = self.client.drain_notifications().await;
        if notifications.is_empty() {
            return;
        }

        // Any review-related notification triggers a full file list reload.
        let needs_reload = notifications.iter().any(|n| {
            matches!(
                n.kind,
                crate::protocol::NotificationKind::ReviewChanged { .. }
                    | crate::protocol::NotificationKind::ReviewsCleared
                    | crate::protocol::NotificationKind::ReviewsMigrated { .. }
            )
        });

        if needs_reload {
            self.reload_file_list().await;
        }
    }

    /// Reload the file list from the server, preserving selection and cursor.
    async fn reload_file_list(&mut self) {
        let selected_path = review::selected_path(&self.state.files, self.state.selected_file);

        // Save cursor/scroll position to restore after reload.
        let saved_cursor = self.state.diff_line_cursor;
        let saved_col = self.state.diff_col_cursor;
        let saved_scroll = self.state.diff_scroll;
        let saved_content_mode = self.state.content_mode;
        let saved_render_variant = self.state.render_variant;

        match self.client.list_changed_files().await {
            Ok(result) => {
                self.state.files = result.files;
                review::sort_files(&mut self.state.files);
                // Restore selection by path.
                let prev_selected = self.state.selected_file;
                self.state.selected_file =
                    review::restore_selection_by_path(&self.state.files, selected_path.as_deref());

                // Refresh diff and content for the selected file.
                self.state.refresh_current_file_diff();
                self.state.load_head_content();
                self.state.load_blame();

                // Restore cursor/scroll if we're still on the same file.
                if self.state.selected_file == prev_selected {
                    self.state.content_mode = saved_content_mode;
                    self.state.render_variant = saved_render_variant;
                    self.state.diff_line_cursor = saved_cursor;
                    self.state.diff_col_cursor = saved_col;
                    self.state.diff_scroll = saved_scroll;
                    self.state.clamp_cursor_and_scroll();
                } else {
                    self.state.on_file_changed();
                }
            }
            Err(e) => {
                self.state.status_message =
                    Some((format!("Failed to reload files: {e}"), Instant::now()));
            }
        }
    }

    /// Apply a review action result from the server: update the file's status,
    /// re-sort the file list, and auto-advance if needed.
    fn apply_review_result(&mut self, result: &crate::model::ReviewActionResult) {
        self.state.selected_file =
            review::apply_review_result(&mut self.state.files, self.state.selected_file, result);
        self.state.on_file_changed();
    }

    /// Select the word under the given semantic position and copy it.
    fn select_word_at(&mut self, pane: PaneFocus, anchor: TextAnchor) -> bool {
        let (text, base_col) = match pane {
            PaneFocus::FileList => (&self.state.file_list_rendered_text, 0),
            PaneFocus::Diff => (
                &self.state.diff_rendered_text,
                self.state.diff_content_start_col(),
            ),
        };
        let line = match text.get(anchor.line) {
            Some(line) => line,
            None => return false,
        };

        let chars: Vec<char> = line.chars().collect();
        let click_idx = base_col.saturating_add(anchor.column);
        let (start, end) = match word_bounds_at_index(&chars, click_idx) {
            Some(bounds) => bounds,
            None => return false,
        };

        let word: String = chars[start..=end].iter().collect();
        if word.is_empty() {
            return false;
        }

        copy_to_clipboard(&word);

        let start_anchor = TextAnchor {
            line: anchor.line,
            column: start.saturating_sub(base_col),
        };
        let end_anchor = TextAnchor {
            line: anchor.line,
            column: end.saturating_sub(base_col),
        };
        self.state.mouse_selection = Some(MouseSelection {
            pane,
            start: start_anchor,
            end: end_anchor,
            word_selected: true,
        });
        self.state.status_message = Some((format!("Copied identifier: {word}"), Instant::now()));
        true
    }

    /// Double-click in the file list: copy the full file path to the clipboard.
    fn copy_file_path_at(&mut self, row: usize) {
        if let Some(&Some(file_idx)) = self.state.file_list_row_to_file.get(row) {
            if let Some(entry) = self.state.files.get(file_idx) {
                let path = &entry.change.path;
                copy_to_clipboard(path);
                self.state.status_message = Some((format!("Copied: {path}"), Instant::now()));
            }
        }
    }
}

fn core_mouse_button(button: MouseButton) -> Option<CoreMouseButton> {
    match button {
        MouseButton::Left => Some(CoreMouseButton::Left),
        MouseButton::Right => Some(CoreMouseButton::Right),
        MouseButton::Middle => Some(CoreMouseButton::Middle),
    }
}

fn mouse_content_hit(event: &InputEvent) -> Option<(PaneFocus, TextAnchor)> {
    let InputEvent::Mouse(mouse) = event else {
        return None;
    };
    let hit = mouse.semantic_hit.as_ref()?;
    let pane = match hit.pane_id {
        PaneId::FileList => PaneFocus::FileList,
        PaneId::Diff => PaneFocus::Diff,
        _ => return None,
    };
    Some((pane, hit.text_anchor?))
}

fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn word_bounds_at_index(chars: &[char], click_idx: usize) -> Option<(usize, usize)> {
    if click_idx >= chars.len() || !is_identifier_char(chars[click_idx]) {
        return None;
    }

    // Expand to word boundaries.
    let mut start = click_idx;
    while start > 0 && is_identifier_char(chars[start - 1]) {
        start -= 1;
    }
    let mut end = click_idx;
    while end + 1 < chars.len() && is_identifier_char(chars[end + 1]) {
        end += 1;
    }
    Some((start, end))
}

// ---------------------------------------------------------------------------
// Navigation helpers (used by process_pending_command)
// ---------------------------------------------------------------------------

/// Navigate to a resolved location target from the App context without going
/// through the key handler.
fn navigate_to_location_from_app(state: &mut AppState, target: core_search::LocationTarget) {
    match target {
        core_search::LocationTarget::InDiff {
            file_index,
            line_number,
        } => {
            push_jump_stack_from_app(state);
            state.selected_file = file_index;
            state.on_file_changed();
            state.diff_line_cursor = (line_number as usize).saturating_sub(1);
            state.clamp_cursor_and_scroll();
        }
        core_search::LocationTarget::External {
            file_path,
            line_number,
        } => {
            push_jump_stack_from_app(state);
            state.status_message = Some((
                format!("Definition in file not in diff: {file_path}:{line_number}"),
                Instant::now(),
            ));
        }
    }
}

fn push_jump_stack_from_app(state: &mut AppState) {
    state.jump_stack.push(JumpLocation {
        file_index: state.selected_file,
        diff_scroll: state.diff_scroll,
        diff_line_cursor: state.diff_line_cursor,
        content_mode: state.content_mode,
        render_variant: state.render_variant,
    });
}

// ---------------------------------------------------------------------------
// Text extraction from selection
// ---------------------------------------------------------------------------

/// Extract the selected text from the rendered content stored in state.
fn extract_selected_text(state: &AppState, sel: &MouseSelection) -> String {
    let (text, base_col) = match sel.pane {
        PaneFocus::Diff => (&state.diff_rendered_text, state.diff_content_start_col()),
        PaneFocus::FileList => (&state.file_list_rendered_text, 0),
    };

    if text.is_empty() {
        return String::new();
    }

    let (start, end) = sel.normalized();

    let mut result = String::new();
    for line_idx in start.line..=end.line {
        if line_idx >= text.len() {
            break;
        }

        let line = &text[line_idx];
        let chars: Vec<char> = line.chars().collect();

        let col_start = if line_idx == start.line {
            base_col.saturating_add(start.column)
        } else {
            base_col
        };
        let col_end = if line_idx == end.line {
            base_col.saturating_add(end.column).saturating_add(1)
        } else {
            chars.len()
        };

        let col_start = col_start.min(chars.len());
        let col_end = col_end.min(chars.len());

        if col_start >= col_end {
            if line_idx < end.line {
                result.push('\n');
            }
            continue;
        }

        let extracted: String = chars[col_start..col_end].iter().collect();
        result.push_str(extracted.trim_end());
        if line_idx < end.line {
            result.push('\n');
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Clipboard (OSC 52)
// ---------------------------------------------------------------------------

/// Copy text to the system clipboard via the OSC 52 escape sequence.
/// This is supported by most modern terminal emulators (iTerm2, kitty,
/// alacritty, WezTerm, etc.).
fn copy_to_clipboard(text: &str) {
    let encoded = base64_encode(text.as_bytes());
    let osc = format!("\x1b]52;c;{encoded}\x07");
    let _ = io::stdout().write_all(osc.as_bytes());
    let _ = io::stdout().flush();
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((n >> 18) & 63) as usize] as char);
        result.push(CHARS[((n >> 12) & 63) as usize] as char);
        result.push(if chunk.len() > 1 {
            CHARS[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        result.push(if chunk.len() > 2 {
            CHARS[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    result
}

// ---------------------------------------------------------------------------
// Terminal setup / teardown
// ---------------------------------------------------------------------------

/// Enter raw mode, the alternate screen, and enable mouse capture.
fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    terminal::enable_raw_mode().context("Failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        crossterm::event::EnableFocusChange,
    )
    .context("Failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).context("Failed to create terminal")
}

/// Leave the alternate screen, disable raw mode, and release the mouse.
fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    terminal::disable_raw_mode().context("Failed to disable raw mode")?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        crossterm::event::DisableFocusChange,
        LeaveAlternateScreen
    )
    .context("Failed to leave alternate screen")?;
    terminal.show_cursor().context("Failed to show cursor")?;
    Ok(())
}

/// Emergency terminal restore without a Terminal handle (for panic hook).
fn restore_terminal_raw() {
    let _ = terminal::disable_raw_mode();
    let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChangeKind, DiffContent, FileChange, FileEntry, ReviewStatus};

    fn test_context() -> ConnectionContext {
        ConnectionContext {
            repo_root: "/repo".to_string(),
            worktree: "/repo".to_string(),
            base_ref: "main".to_string(),
            head_ref: "feature".to_string(),
            merge_base: "abc123".to_string(),
        }
    }

    fn test_file(path: &str) -> FileEntry {
        FileEntry {
            change: FileChange {
                path: path.to_string(),
                old_path: None,
                kind: ChangeKind::Modified,
            },
            status: ReviewStatus::Unreviewed,
            diff: DiffContent {
                hunks: Vec::new(),
                is_binary: false,
                diff_hash: format!("hash-{path}"),
            },
        }
    }

    fn test_state() -> AppState {
        AppState::new(
            StyleConfig::default(),
            30,
            crate::config::DiffAlgorithm::Myers,
            None,
            test_context(),
            vec![test_file("src/lib.rs")],
        )
    }

    #[test]
    fn test_base64_encode() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"Hello, world!"), "SGVsbG8sIHdvcmxkIQ==");
    }

    #[test]
    fn test_selection_normalized() {
        // Forward selection.
        let sel = MouseSelection {
            pane: PaneFocus::Diff,
            start: TextAnchor { line: 2, column: 5 },
            end: TextAnchor {
                line: 4,
                column: 10,
            },
            word_selected: false,
        };
        assert_eq!(
            sel.normalized(),
            (
                TextAnchor { line: 2, column: 5 },
                TextAnchor {
                    line: 4,
                    column: 10,
                }
            )
        );

        // Backward selection (dragged upward).
        let sel = MouseSelection {
            pane: PaneFocus::Diff,
            start: TextAnchor {
                line: 4,
                column: 10,
            },
            end: TextAnchor { line: 2, column: 5 },
            word_selected: false,
        };
        assert_eq!(
            sel.normalized(),
            (
                TextAnchor { line: 2, column: 5 },
                TextAnchor {
                    line: 4,
                    column: 10,
                }
            )
        );
    }

    #[test]
    fn extract_selected_text_uses_semantic_file_list_anchors() {
        let mut state = test_state();
        state.file_list_rendered_text = vec![
            "header".to_string(),
            "src/app.rs".to_string(),
            "src/core/input.rs".to_string(),
        ];

        let sel = MouseSelection {
            pane: PaneFocus::FileList,
            start: TextAnchor { line: 1, column: 4 },
            end: TextAnchor { line: 2, column: 7 },
            word_selected: false,
        };

        assert_eq!(extract_selected_text(&state, &sel), "app.rs\nsrc/core");
    }

    #[test]
    fn extract_selected_text_uses_diff_content_anchors() {
        let mut state = test_state();
        state.diff_gutter_cols = 4;
        state.diff_rendered_text = vec![
            "     + first line".to_string(),
            "       second line".to_string(),
        ];

        let sel = MouseSelection {
            pane: PaneFocus::Diff,
            start: TextAnchor { line: 0, column: 0 },
            end: TextAnchor { line: 1, column: 5 },
            word_selected: false,
        };

        assert_eq!(extract_selected_text(&state, &sel), "first line\nsecond");
    }

    #[test]
    fn mouse_input_event_maps_file_list_hit() {
        let mut state = test_state();
        state.file_list_area = Rect::new(0, 0, 30, 10);
        state.file_list_row_to_file = vec![Some(0)];

        let event = state
            .input_event_from_mouse(&MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 5,
                row: 1,
                modifiers: event::KeyModifiers::CONTROL,
            })
            .expect("expected mouse input");

        assert_eq!(
            event,
            InputEvent::Mouse(CoreMouseEvent {
                kind: CoreMouseEventKind::Down,
                button: Some(CoreMouseButton::Left),
                local_pos: Some((5, 1)),
                semantic_hit: Some(PointerSemanticHit {
                    pane_id: PaneId::FileList,
                    region_id: Some("file:src/lib.rs".to_string()),
                    text_anchor: Some(TextAnchor { line: 0, column: 4 }),
                }),
                modifiers: InputModifiers {
                    ctrl: true,
                    alt: false,
                    shift: false,
                },
            })
        );
    }

    #[test]
    fn mouse_input_event_maps_diff_hit_to_content_anchor() {
        let mut state = test_state();
        state.show_file_list = false;
        state.diff_area = Rect::new(0, 0, 80, 20);
        state.diff_scroll = 10;
        state.diff_gutter_cols = 4;

        let event = state
            .input_event_from_mouse(&MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 12,
                row: 3,
                modifiers: event::KeyModifiers::SHIFT,
            })
            .expect("expected mouse input");

        assert_eq!(
            event,
            InputEvent::Mouse(CoreMouseEvent {
                kind: CoreMouseEventKind::ScrollDown,
                button: None,
                local_pos: Some((12, 3)),
                semantic_hit: Some(PointerSemanticHit {
                    pane_id: PaneId::Diff,
                    region_id: Some("diff-line:12".to_string()),
                    text_anchor: Some(TextAnchor {
                        line: 12,
                        column: 4,
                    }),
                }),
                modifiers: InputModifiers {
                    ctrl: false,
                    alt: false,
                    shift: true,
                },
            })
        );
    }

    #[test]
    fn test_word_bounds_at_index_selects_full_identifier() {
        let text = "use merge_base even";
        let chars: Vec<char> = text.chars().collect();

        // Click on the first character 'm'.
        let (start, end) = word_bounds_at_index(&chars, 4).expect("expected identifier");
        let word: String = chars[start..=end].iter().collect();
        assert_eq!(word, "merge_base");

        // Click on the underscore still selects the whole identifier.
        let (start, end) = word_bounds_at_index(&chars, 9).expect("expected identifier");
        let word: String = chars[start..=end].iter().collect();
        assert_eq!(word, "merge_base");
    }

    #[test]
    fn test_word_bounds_at_index_returns_none_on_punctuation() {
        let text = "foo.bar";
        let chars: Vec<char> = text.chars().collect();

        // Click on '.'
        assert!(word_bounds_at_index(&chars, 3).is_none());
    }
}
