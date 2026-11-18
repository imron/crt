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

use crate::client::Client;
use crate::config::StyleConfig;
use crate::core::diff;
use crate::core::review;
use crate::keys;
use crate::model::{
    ConnectionContext, ContentMode, DefinitionLocation, FileEntry, PaneFocus, RenderVariant,
    ReviewStatus, SearchMatch,
};
use crate::ui;

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
    /// Pane area at time of selection start (for coordinate mapping).
    pub pane_area: Rect,
    /// Start position in terminal coordinates.
    pub start_col: u16,
    pub start_row: u16,
    /// Current end position in terminal coordinates.
    pub end_col: u16,
    pub end_row: u16,
    /// Set when selection was created by double-click word selection.
    /// The Up event should not re-extract text (it was already copied).
    pub word_selected: bool,
}

impl MouseSelection {
    /// Normalize so start is before end (handles upward/leftward drags).
    pub fn normalized(&self) -> (u16, u16, u16, u16) {
        if self.start_row < self.end_row
            || (self.start_row == self.end_row && self.start_col <= self.end_col)
        {
            (self.start_col, self.start_row, self.end_col, self.end_row)
        } else {
            (self.end_col, self.end_row, self.start_col, self.start_row)
        }
    }
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
    /// Mouse down anchor (pane, pane area, column, row) used to start a drag
    /// selection only after the pointer actually moves.
    pub mouse_down_anchor: Option<(PaneFocus, Rect, u16, u16)>,
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
    /// Last mouse click time and position, for double-click detection.
    pub last_click: Option<(Instant, u16, u16)>,
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
    pub diff_cache: Option<crate::ui::diff_view::DiffCache>,
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
    pub pending_command: Option<String>,
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
            self.terminal.draw(|frame| ui::draw(frame, state))?;

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
                self.process_pending_command(&cmd).await;
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
                keys::handle_key_event(&mut self.state, key);
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
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Check if the user clicked on the pane border to start resizing.
                if self.is_on_pane_border(mouse.column, mouse.row) {
                    self.state.dragging_border = true;
                    return;
                }

                let pane = self.state.pane_at(mouse.column, mouse.row);

                // Double-click detection: select the word under cursor.
                let is_double_click = self.state.last_click.is_some_and(|(t, c, r)| {
                    t.elapsed() < Duration::from_millis(400) && c == mouse.column && r == mouse.row
                });
                self.state.last_click = Some((Instant::now(), mouse.column, mouse.row));

                if is_double_click {
                    // Clear any selection from the first click so the
                    // subsequent Up event doesn't overwrite the clipboard.
                    self.state.mouse_selection = None;
                    self.state.mouse_down_anchor = None;
                    if let Some(pane) = pane {
                        if pane == PaneFocus::FileList {
                            self.copy_file_path_at(mouse.row);
                        } else {
                            self.select_word_at(pane, mouse.column, mouse.row);
                        }
                    }
                    self.state.last_click = None; // prevent triple-click
                    return; // skip drag selection setup
                }

                if let Some(pane) = pane {
                    // Click in the file list selects a file.
                    if pane == PaneFocus::FileList {
                        self.handle_file_list_click(mouse.column, mouse.row);
                    } else {
                        self.set_diff_cursor_from_mouse(mouse.column, mouse.row);
                    }

                    // Record mouse-down anchor; drag starts selection.
                    let pane_area = match pane {
                        PaneFocus::FileList => self.state.file_list_area,
                        PaneFocus::Diff => self.state.diff_area,
                    };
                    self.state.mouse_selection = None;
                    self.state.mouse_down_anchor = Some((pane, pane_area, mouse.column, mouse.row));
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.state.dragging_border {
                    // Resize the file list pane. Minimum 10, maximum
                    // terminal width minus 20.
                    let min_w = 10u16;
                    let max_w = self.state.file_list_area.width + self.state.diff_area.width - 20;
                    let new_width = (mouse.column + 1).clamp(min_w, max_w);
                    self.state.file_list_width = new_width;
                    return;
                }
                // Extend the selection, clamped to the originating pane.
                if let Some(sel) = &mut self.state.mouse_selection {
                    let area = sel.pane_area;
                    sel.end_col = mouse
                        .column
                        .clamp(area.x + 1, area.right().saturating_sub(2));
                    sel.end_row = mouse.row.clamp(area.y + 1, area.bottom().saturating_sub(2));
                } else if let Some((pane, pane_area, start_col, start_row)) =
                    self.state.mouse_down_anchor
                {
                    let end_col = mouse
                        .column
                        .clamp(pane_area.x + 1, pane_area.right().saturating_sub(2));
                    let end_row = mouse
                        .row
                        .clamp(pane_area.y + 1, pane_area.bottom().saturating_sub(2));
                    self.state.mouse_selection = Some(MouseSelection {
                        pane,
                        pane_area,
                        start_col,
                        start_row,
                        end_col,
                        end_row,
                        word_selected: false,
                    });
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
            MouseEventKind::ScrollDown => {
                if self.state.pane_at(mouse.column, mouse.row) == Some(PaneFocus::Diff) {
                    self.state.diff_scroll = self.state.diff_scroll.saturating_add(3);
                    self.state.clamp_diff_scroll();
                    // Keep cursor visible in viewport.
                    if self.state.diff_line_cursor < self.state.diff_scroll {
                        self.state.diff_line_cursor = self.state.diff_scroll;
                    }
                }
            }
            MouseEventKind::ScrollUp => {
                if self.state.pane_at(mouse.column, mouse.row) == Some(PaneFocus::Diff) {
                    self.state.diff_scroll = self.state.diff_scroll.saturating_sub(3);
                    // Keep cursor visible in viewport.
                    let bottom =
                        self.state.diff_scroll + self.state.diff_view_height.saturating_sub(1);
                    if self.state.diff_line_cursor > bottom {
                        self.state.diff_line_cursor = bottom;
                    }
                }
            }
            _ => {}
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
    async fn process_pending_command(&mut self, cmd: &str) {
        let (name, args) = match cmd.split_once(char::is_whitespace) {
            Some((n, a)) => (n, a.trim()),
            None => (cmd, ""),
        };

        match name {
            "gr" => {
                self.state.status_message = Some(("Searching...".to_string(), Instant::now()));
                // Force a re-render so the user sees the searching message.
                let state = &mut self.state;
                let _ = self.terminal.draw(|frame| ui::draw(frame, state));

                match self.client.search_codebase(args, "all").await {
                    Ok(result) => {
                        if result.matches.is_empty() {
                            self.state.status_message =
                                Some((format!("No matches for /{args}/"), Instant::now()));
                        } else {
                            self.state.search_results = Some(SearchResults {
                                query: args.to_string(),
                                diff_only: false,
                                matches: result.matches,
                                selected: 0,
                                scroll: 0,
                            });
                            self.state.status_message = None;
                        }
                    }
                    Err(e) => {
                        self.state.status_message =
                            Some((format!("Search error: {e}"), Instant::now()));
                    }
                }
            }
            "grd" => {
                self.state.status_message =
                    Some(("Searching diff files...".to_string(), Instant::now()));
                let state = &mut self.state;
                let _ = self.terminal.draw(|frame| ui::draw(frame, state));

                match self.client.search_codebase(args, "diff").await {
                    Ok(result) => {
                        if result.matches.is_empty() {
                            self.state.status_message =
                                Some((format!("No matches for /{args}/ in diff"), Instant::now()));
                        } else {
                            self.state.search_results = Some(SearchResults {
                                query: args.to_string(),
                                diff_only: true,
                                matches: result.matches,
                                selected: 0,
                                scroll: 0,
                            });
                            self.state.status_message = None;
                        }
                    }
                    Err(e) => {
                        self.state.status_message =
                            Some((format!("Search error: {e}"), Instant::now()));
                    }
                }
            }
            "find_definition" => {
                self.state.status_message =
                    Some(("Finding definition...".to_string(), Instant::now()));
                let state = &mut self.state;
                let _ = self.terminal.draw(|frame| ui::draw(frame, state));

                let context_file = self
                    .state
                    .selected_file_entry()
                    .map(|e| e.change.path.clone());
                match self
                    .client
                    .find_definition(args, context_file.as_deref())
                    .await
                {
                    Ok(result) => {
                        if result.definitions.is_empty() {
                            self.state.status_message = Some((
                                format!("No definitions found for '{args}'"),
                                Instant::now(),
                            ));
                        } else if result.definitions.len() == 1 {
                            // Single result: navigate directly.
                            let def = result.definitions[0].clone();
                            self.state.status_message = None;
                            navigate_to_definition_from_app(&mut self.state, &def);
                        } else {
                            // Multiple results: show picker.
                            self.state.definition_results = Some(DefinitionResults {
                                symbol: args.to_string(),
                                definitions: result.definitions,
                                selected: 0,
                            });
                            self.state.status_message = None;
                        }
                    }
                    Err(e) => {
                        self.state.status_message =
                            Some((format!("Definition error: {e}"), Instant::now()));
                    }
                }
            }
            "view_file" => {
                // view_file <path> <line>
                // For now, show a status message since read-only view
                // for non-diff files would require a separate content mode.
                self.state.status_message =
                    Some((format!("File not in diff: {args}"), Instant::now()));
            }
            _ => {
                self.state.status_message =
                    Some((format!("Unknown pending command: {name}"), Instant::now()));
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

    /// Move the diff cursor to the mouse position.
    fn set_diff_cursor_from_mouse(&mut self, col: u16, row: u16) {
        let area = self.state.diff_area;
        let inner_top = area.y + 1;
        let inner_left = area.x + 1;
        let inner_bottom = area.bottom().saturating_sub(1);
        if row < inner_top || row >= inner_bottom {
            return;
        }

        let view_row = (row - inner_top) as usize;
        let display_row = self.state.diff_scroll.saturating_add(view_row);
        self.state.diff_line_cursor = display_row;

        let content_start = inner_left as usize + self.state.diff_content_start_col();
        self.state.diff_col_cursor = (col as usize).saturating_sub(content_start);

        self.state.clamp_cursor_and_scroll();
        self.state.clamp_col_cursor();
    }

    /// Select the word under the given terminal position and copy to clipboard.
    ///
    /// In the diff pane, this maps through rendered diff text for stable
    /// coordinate behavior. Other panes read directly from the terminal buffer.
    fn select_word_at(&mut self, pane: PaneFocus, col: u16, row: u16) -> bool {
        let pane_area = match pane {
            PaneFocus::FileList => self.state.file_list_area,
            PaneFocus::Diff => self.state.diff_area,
        };

        let inner_left = pane_area.x + 1;
        let inner_right = pane_area.right().saturating_sub(1);

        if col < inner_left || col >= inner_right {
            return false;
        }

        // Prefer rendered-text mapping for the diff pane. This avoids terminal
        // buffer cell quirks and maps directly to what we render.
        if pane == PaneFocus::Diff {
            let inner_top = pane_area.y + 1;
            let content_row =
                (row as usize).saturating_sub(inner_top as usize) + self.state.diff_scroll;
            let line = match self.state.diff_rendered_text.get(content_row) {
                Some(l) => l,
                None => return false,
            };

            let chars: Vec<char> = line.chars().collect();
            let click_idx = (col as usize).saturating_sub(inner_left as usize);
            if click_idx >= chars.len() || !is_identifier_char(chars[click_idx]) {
                return false;
            }

            let mut start = click_idx;
            while start > 0 && is_identifier_char(chars[start - 1]) {
                start -= 1;
            }
            let mut end = click_idx;
            while end + 1 < chars.len() && is_identifier_char(chars[end + 1]) {
                end += 1;
            }

            let word: String = chars[start..=end].iter().collect();
            if word.is_empty() {
                return false;
            }

            copy_to_clipboard(&word);
            self.state.mouse_selection = Some(MouseSelection {
                pane,
                pane_area,
                start_col: inner_left.saturating_add(start as u16),
                start_row: row,
                end_col: inner_left.saturating_add(end as u16),
                end_row: row,
                word_selected: true,
            });
            self.state.status_message =
                Some((format!("Copied identifier: {word}"), Instant::now()));
            return true;
        }

        // Read the row from the terminal buffer.
        let buf = self.terminal.current_buffer_mut();
        let mut row_chars: Vec<(u16, char)> = Vec::new();
        for x in inner_left..inner_right {
            if let Some(cell) = buf.cell(ratatui::layout::Position { x, y: row }) {
                let sym = cell.symbol();
                // Multi-width chars: only take the first cell.
                if !sym.is_empty() {
                    row_chars.push((x, sym.chars().next().unwrap_or(' ')));
                }
            }
        }

        let (start, end) = match word_bounds_at_column(&row_chars, col) {
            Some(bounds) => bounds,
            None => return false,
        };

        let word: String = row_chars[start..=end].iter().map(|&(_, c)| c).collect();
        if !word.is_empty() {
            copy_to_clipboard(&word);

            // Set selection highlight on the word.
            self.state.mouse_selection = Some(MouseSelection {
                pane,
                pane_area,
                start_col: row_chars[start].0,
                start_row: row,
                end_col: row_chars[end].0,
                end_row: row,
                word_selected: true,
            });

            self.state.status_message =
                Some((format!("Copied identifier: {word}"), Instant::now()));
            return true;
        }

        false
    }

    /// Double-click in the file list: copy the full file path to the clipboard.
    fn copy_file_path_at(&mut self, row: u16) {
        let area = self.state.file_list_area;
        let inner_top = area.y + 1;
        let content_row =
            (row as usize).saturating_sub(inner_top as usize) + self.state.file_list_scroll;

        if let Some(&Some(file_idx)) = self.state.file_list_row_to_file.get(content_row) {
            if let Some(entry) = self.state.files.get(file_idx) {
                let path = &entry.change.path;
                copy_to_clipboard(path);
                self.state.status_message = Some((format!("Copied: {path}"), Instant::now()));
            }
        }
    }

    /// Map a mouse click in the file list pane to a file selection.
    fn handle_file_list_click(&mut self, _col: u16, row: u16) {
        let area = self.state.file_list_area;
        let inner_top = area.y + 1; // skip border

        // Convert screen row to content row (accounting for scroll).
        let content_row =
            (row as usize).saturating_sub(inner_top as usize) + self.state.file_list_scroll;

        // Look up which file (if any) this row corresponds to.
        if let Some(&Some(file_idx)) = self.state.file_list_row_to_file.get(content_row) {
            if file_idx < self.state.files.len() && file_idx != self.state.selected_file {
                self.state.selected_file = file_idx;
                self.state.on_file_changed();
            }
        }
    }
}

fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn word_bounds_at_column(row_chars: &[(u16, char)], col: u16) -> Option<(usize, usize)> {
    // Find the character index at the clicked column.
    let click_idx = row_chars.iter().position(|&(x, _)| x == col)?;

    if !is_identifier_char(row_chars[click_idx].1) {
        return None;
    }

    // Expand to word boundaries.
    let mut start = click_idx;
    while start > 0 && is_identifier_char(row_chars[start - 1].1) {
        start -= 1;
    }
    let mut end = click_idx;
    while end + 1 < row_chars.len() && is_identifier_char(row_chars[end + 1].1) {
        end += 1;
    }
    Some((start, end))
}

// ---------------------------------------------------------------------------
// Navigation helpers (used by process_pending_command)
// ---------------------------------------------------------------------------

/// Navigate to a definition location (same logic as in keys.rs but accessible
/// from the App context without going through the key handler).
fn navigate_to_definition_from_app(state: &mut AppState, def: &DefinitionLocation) {
    if let Some(idx) = state
        .files
        .iter()
        .position(|f| f.change.path == def.file_path)
    {
        state.jump_stack.push(JumpLocation {
            file_index: state.selected_file,
            diff_scroll: state.diff_scroll,
            diff_line_cursor: state.diff_line_cursor,
            content_mode: state.content_mode,
            render_variant: state.render_variant,
        });
        state.selected_file = idx;
        state.on_file_changed();
        state.diff_line_cursor = (def.line_number as usize).saturating_sub(1);
        state.clamp_cursor_and_scroll();
    } else {
        // File not in diff — show status message for now.
        state.status_message = Some((
            format!(
                "Definition in file not in diff: {}:{}",
                def.file_path, def.line_number
            ),
            Instant::now(),
        ));
    }
}

// ---------------------------------------------------------------------------
// Text extraction from selection
// ---------------------------------------------------------------------------

/// Extract the selected text from the rendered content stored in state.
fn extract_selected_text(state: &AppState, sel: &MouseSelection) -> String {
    let (text, scroll) = match sel.pane {
        PaneFocus::Diff => (&state.diff_rendered_text, state.diff_scroll),
        PaneFocus::FileList => (&state.file_list_rendered_text, 0),
    };

    if text.is_empty() {
        return String::new();
    }

    let area = sel.pane_area;
    // Inner area excludes borders.
    let inner_top = area.y + 1;
    let inner_left = area.x + 1;

    // In the diff pane, skip the line-number gutter columns so only the
    // code content (prefix + text) is extracted.
    let content_left = if sel.pane == PaneFocus::Diff && state.diff_gutter_cols > 0 {
        inner_left + state.diff_gutter_cols as u16
    } else {
        inner_left
    };

    let (start_col, start_row, end_col, end_row) = sel.normalized();

    let mut result = String::new();
    for screen_row in start_row..=end_row {
        let content_idx = (screen_row as usize).saturating_sub(inner_top as usize) + scroll;
        if content_idx >= text.len() {
            break;
        }

        let line = &text[content_idx];
        let chars: Vec<char> = line.chars().collect();

        // Map screen columns to character indices, skipping the gutter.
        let col_start = if screen_row == start_row {
            (start_col.max(content_left) as usize).saturating_sub(inner_left as usize)
        } else {
            (content_left as usize).saturating_sub(inner_left as usize)
        };
        let col_end = if screen_row == end_row {
            (end_col as usize)
                .saturating_sub(inner_left as usize)
                .saturating_add(1)
        } else {
            chars.len()
        };

        let col_start = col_start.min(chars.len());
        let col_end = col_end.min(chars.len());

        if col_start >= col_end {
            if screen_row < end_row {
                result.push('\n');
            }
            continue;
        }

        let extracted: String = chars[col_start..col_end].iter().collect();
        result.push_str(extracted.trim_end());
        if screen_row < end_row {
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
            pane_area: Rect::default(),
            start_col: 5,
            start_row: 2,
            end_col: 10,
            end_row: 4,
            word_selected: false,
        };
        assert_eq!(sel.normalized(), (5, 2, 10, 4));

        // Backward selection (dragged upward).
        let sel = MouseSelection {
            pane: PaneFocus::Diff,
            pane_area: Rect::default(),
            start_col: 10,
            start_row: 4,
            end_col: 5,
            end_row: 2,
            word_selected: false,
        };
        assert_eq!(sel.normalized(), (5, 2, 10, 4));
    }

    #[test]
    fn test_word_bounds_at_column_selects_full_identifier() {
        let text = "use merge_base even";
        let row_chars: Vec<(u16, char)> = text
            .chars()
            .enumerate()
            .map(|(i, c)| ((i + 1) as u16, c))
            .collect();

        // Click on the first character 'm'.
        let (start, end) = word_bounds_at_column(&row_chars, 5).expect("expected identifier");
        let word: String = row_chars[start..=end].iter().map(|(_, c)| *c).collect();
        assert_eq!(word, "merge_base");

        // Click on the underscore still selects the whole identifier.
        let (start, end) = word_bounds_at_column(&row_chars, 10).expect("expected identifier");
        let word: String = row_chars[start..=end].iter().map(|(_, c)| *c).collect();
        assert_eq!(word, "merge_base");
    }

    #[test]
    fn test_word_bounds_at_column_returns_none_on_punctuation() {
        let text = "foo.bar";
        let row_chars: Vec<(u16, char)> = text
            .chars()
            .enumerate()
            .map(|(i, c)| ((i + 1) as u16, c))
            .collect();

        // Click on '.'
        assert!(word_bounds_at_column(&row_chars, 4).is_none());
    }
}
