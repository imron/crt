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
use crate::keys;
use crate::model::{
    ConnectionContext, ContentMode, FileEntry, PaneFocus, RenderVariant, ReviewStatus,
};
use crate::ui;

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
    /// Whether a reviewed file's diff has been expanded via Enter.
    /// Resets when selected_file changes.
    pub reviewed_diff_expanded: bool,
    /// HEAD version of the selected file (loaded from git on file change).
    pub head_content: Option<String>,
    /// Base version of the selected file (loaded from git on demand).
    pub base_content: Option<String>,
    /// Display row indices where each hunk starts (set during render,
    /// used by Ctrl-i/Ctrl-o to jump between hunks).
    pub hunk_start_rows: Vec<usize>,
    /// Display row indices where each hunk ends (exclusive, set during render).
    pub hunk_end_rows: Vec<usize>,
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
    /// Whether Ctrl-W was pressed and we're waiting for the next key.
    pub pending_ctrl_w: bool,
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
    /// Cached diff content — avoids rebuilding all `Line<'static>` on every
    /// frame when only the scroll offset changed.
    pub diff_cache: Option<crate::ui::diff_view::DiffCache>,
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
        // Sort: unreviewed (including changed) first, then reviewed.
        // Alphabetical by path within each group.
        files.sort_by(|a, b| {
            let a_reviewed = matches!(a.status, ReviewStatus::Reviewed { .. });
            let b_reviewed = matches!(b.status, ReviewStatus::Reviewed { .. });
            a_reviewed
                .cmp(&b_reviewed)
                .then(a.change.path.cmp(&b.change.path))
        });

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
            reviewed_diff_expanded: false,
            head_content: None,
            base_content: None,
            hunk_start_rows: Vec::new(),
            hunk_end_rows: Vec::new(),
            diff_gutter_cols: 0,
            diff_content_height: 0,
            diff_view_height: 0,
            show_comments: false,
            show_file_list: true,
            show_diff_pane: true,
            mouse_selection: None,
            diff_rendered_text: Vec::new(),
            file_list_rendered_text: Vec::new(),
            file_list_row_to_file: Vec::new(),
            file_list_scroll: 0,
            file_list_area: Rect::default(),
            diff_area: Rect::default(),
            status_message: None,
            pending_ctrl_w: false,
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
            diff_cache: None,
        }
    }

    /// The currently selected file, if any.
    pub fn selected_file_entry(&self) -> Option<&FileEntry> {
        self.files.get(self.selected_file)
    }

    /// Number of unreviewed files (Unreviewed + Changed status).
    /// Since files are sorted unreviewed-first, these are files[0..count].
    pub fn unreviewed_count(&self) -> usize {
        self.files
            .iter()
            .filter(|f| !matches!(f.status, ReviewStatus::Reviewed { .. }))
            .count()
    }

    /// Maximum scroll offset: the last line sits at the top of the viewport.
    pub fn max_diff_scroll(&self) -> usize {
        self.diff_content_height.saturating_sub(1)
    }

    /// Load HEAD content for the currently selected file from git.
    pub fn load_head_content(&mut self) {
        self.head_content = None;
        if let Some(entry) = self.files.get(self.selected_file) {
            let path = entry.change.path.clone();
            if let Ok(repo) = crate::git::Repo::open(std::path::Path::new(&self.context.worktree))
            {
                self.head_content = repo.file_content("HEAD", &path).ok().flatten();
            }
        }
    }

    /// Load base content for the currently selected file from git.
    pub fn load_base_content(&mut self) {
        self.base_content = None;
        if let Some(entry) = self.files.get(self.selected_file) {
            let path = entry.change.path.clone();
            if let Ok(repo) = crate::git::Repo::open(std::path::Path::new(&self.context.worktree))
            {
                self.base_content = repo
                    .file_content(&self.context.merge_base, &path)
                    .ok()
                    .flatten();
            }
        }
    }

    /// Which hunk (0-indexed) the current scroll position is inside,
    /// or None if between hunks or before/after all hunks.
    pub fn current_hunk_index(&self) -> Option<usize> {
        for (i, (&start, &end)) in self
            .hunk_start_rows
            .iter()
            .zip(&self.hunk_end_rows)
            .enumerate()
        {
            if self.diff_scroll >= start && self.diff_scroll < end {
                return Some(i);
            }
        }
        None
    }

    /// Clamp `diff_scroll` to the valid range.
    pub fn clamp_diff_scroll(&mut self) {
        self.diff_scroll = self.diff_scroll.min(self.max_diff_scroll());
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

        if !self.show_blame {
            return;
        }

        if let Some(entry) = self.files.get(self.selected_file) {
            let path = entry.change.path.clone();
            if let Ok(repo) =
                crate::git::Repo::open(std::path::Path::new(&self.context.worktree))
            {
                if let Ok(blame) = repo.blame_file("HEAD", &path) {
                    self.head_blame = blame;
                }
                if let Ok(blame) = repo.blame_file(&self.context.merge_base, &path) {
                    self.base_blame = blame;
                }
            }
        }
    }

    /// Reload the diff for the currently selected file, respecting
    /// the `ignore_whitespace` flag.
    pub fn reload_current_diff(&mut self) {
        if let Some(entry) = self.files.get(self.selected_file) {
            let path = entry.change.path.clone();
            if let Ok(repo) =
                crate::git::Repo::open(std::path::Path::new(&self.context.worktree))
            {
                if let Ok(diff) = repo.diff_file_opts(
                    &self.context.merge_base,
                    "HEAD",
                    &path,
                    self.diff_algorithm,
                    self.ignore_whitespace,
                ) {
                    if let Some(entry) = self.files.get_mut(self.selected_file) {
                        entry.diff = diff;
                    }
                }
            }
        }
        self.diff_scroll = self
            .selected_file_entry()
            .and_then(|e| e.diff.hunks.first())
            .map(|h| (h.new_start as usize).saturating_sub(1))
            .unwrap_or(0);
    }

    /// Called after `selected_file` changes. Resets diff state and loads
    /// the appropriate file content from git.
    pub fn on_file_changed(&mut self) {
        self.reviewed_diff_expanded = false;
        self.hunk_start_rows.clear();
        self.hunk_end_rows.clear();
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

        // Scroll to the first hunk (approximate: new_start - 1 unchanged lines before it).
        self.diff_scroll = self
            .selected_file_entry()
            .and_then(|e| e.diff.hunks.first())
            .map(|h| (h.new_start as usize).saturating_sub(1))
            .unwrap_or(0);
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
        let diff_algorithm = cfg.layout.diff_algorithm.unwrap_or_else(|| {
            crate::git::Repo::open(std::path::Path::new(&context.worktree))
                .ok()
                .and_then(|repo| repo.diff_config().algorithm)
                .unwrap_or(crate::config::DiffAlgorithm::Patience)
        });

        let config_path = crate::config::config_path();
        let mut state = AppState::new(
            cfg.style,
            cfg.layout.file_list_width,
            diff_algorithm,
            config_path,
            context,
            files,
        );
        state.load_head_content();
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
                    Ok(None) => break,       // channel closed
                    Err(_) => continue,      // timeout — re-render to clear message
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

            // Check for server-pushed notifications (from other clients).
            self.process_notifications().await;

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
                keys::handle_key_event(&mut self.state, key);
            }
            Event::Mouse(mouse) => {
                self.handle_mouse_event(mouse);
            }
            Event::Resize(_w, _h) => {
                // ratatui handles resize automatically on next draw.
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

                // Double-click detection: select the word under cursor.
                let is_double_click = self
                    .state
                    .last_click
                    .is_some_and(|(t, c, r)| {
                        t.elapsed() < Duration::from_millis(400)
                            && c == mouse.column
                            && r == mouse.row
                    });
                self.state.last_click = Some((Instant::now(), mouse.column, mouse.row));

                if is_double_click {
                    if let Some(pane) = self.state.pane_at(mouse.column, mouse.row) {
                        self.select_word_at(pane, mouse.column, mouse.row);
                    }
                    self.state.last_click = None; // prevent triple-click
                    return; // skip drag selection setup
                }

                if let Some(pane) = self.state.pane_at(mouse.column, mouse.row) {
                    // Click in the file list selects a file.
                    if pane == PaneFocus::FileList {
                        self.handle_file_list_click(mouse.column, mouse.row);
                    }

                    // Start a new text selection in whichever pane was clicked.
                    let pane_area = match pane {
                        PaneFocus::FileList => self.state.file_list_area,
                        PaneFocus::Diff => self.state.diff_area,
                    };
                    self.state.mouse_selection = Some(MouseSelection {
                        pane,
                        pane_area,
                        start_col: mouse.column,
                        start_row: mouse.row,
                        end_col: mouse.column,
                        end_row: mouse.row,
                    });
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.state.dragging_border {
                    // Resize the file list pane. Minimum 10, maximum
                    // terminal width minus 20.
                    let min_w = 10u16;
                    let max_w = self.state.file_list_area.width
                        + self.state.diff_area.width
                        - 20;
                    let new_width = (mouse.column + 1).clamp(min_w, max_w);
                    self.state.file_list_width = new_width;
                    return;
                }
                // Extend the selection, clamped to the originating pane.
                if let Some(sel) = &mut self.state.mouse_selection {
                    let area = sel.pane_area;
                    sel.end_col = mouse.column.clamp(area.x + 1, area.right().saturating_sub(2));
                    sel.end_row = mouse.row.clamp(area.y + 1, area.bottom().saturating_sub(2));
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.state.dragging_border {
                    self.state.dragging_border = false;
                    self.save_file_list_width();
                    return;
                }
                // Finish selection: extract text and copy to clipboard.
                if let Some(sel) = self.state.mouse_selection.take() {
                    let text = extract_selected_text(&self.state, &sel);
                    if !text.is_empty() {
                        copy_to_clipboard(&text);
                    }
                    // Keep the selection visible until next keypress.
                    self.state.mouse_selection = Some(sel);
                }
            }
            MouseEventKind::ScrollDown => {
                if self.state.pane_at(mouse.column, mouse.row) == Some(PaneFocus::Diff) {
                    self.state.diff_scroll = self.state.diff_scroll.saturating_add(3);
                    self.state.clamp_diff_scroll();
                }
            }
            MouseEventKind::ScrollUp => {
                if self.state.pane_at(mouse.column, mouse.row) == Some(PaneFocus::Diff) {
                    self.state.diff_scroll = self.state.diff_scroll.saturating_sub(3);
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
                self.state.status_message = Some((
                    format!("Failed to {verb} reviewed: {e}"),
                    Instant::now(),
                ));
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
                crate::server::notify::NotificationKind::ReviewChanged { .. }
                    | crate::server::notify::NotificationKind::ReviewsCleared
            )
        });

        if needs_reload {
            self.reload_file_list().await;
        }
    }

    /// Reload the file list from the server, preserving selection by path.
    async fn reload_file_list(&mut self) {
        let selected_path = self
            .state
            .files
            .get(self.state.selected_file)
            .map(|f| f.change.path.clone());

        match self.client.list_changed_files().await {
            Ok(result) => {
                self.state.files = result.files;
                // Re-sort.
                self.state.files.sort_by(|a, b| {
                    let a_reviewed = matches!(a.status, ReviewStatus::Reviewed { .. });
                    let b_reviewed = matches!(b.status, ReviewStatus::Reviewed { .. });
                    a_reviewed
                        .cmp(&b_reviewed)
                        .then(a.change.path.cmp(&b.change.path))
                });
                // Restore selection by path.
                self.state.selected_file = selected_path
                    .and_then(|p| self.state.files.iter().position(|f| f.change.path == p))
                    .unwrap_or(0);
                self.state.on_file_changed();
            }
            Err(e) => {
                self.state.status_message = Some((
                    format!("Failed to reload files: {e}"),
                    Instant::now(),
                ));
            }
        }
    }

    /// Apply a review action result from the server: update the file's status,
    /// re-sort the file list, and auto-advance if needed.
    fn apply_review_result(&mut self, result: &crate::model::ReviewActionResult) {
        let was_marking = matches!(result.status, ReviewStatus::Reviewed { .. });

        // Before mutating, find the advance target: the next unreviewed
        // file after the current selection (by list position, not path).
        let advance_target = if was_marking {
            self.state
                .files
                .iter()
                .skip(self.state.selected_file + 1)
                .find(|f| !matches!(f.status, ReviewStatus::Reviewed { .. }))
                .map(|f| f.change.path.clone())
        } else {
            None
        };

        // Update the file entry's status.
        if let Some(entry) = self
            .state
            .files
            .iter_mut()
            .find(|f| f.change.path == result.file_path)
        {
            entry.status = result.status.clone();
        }

        // Re-sort: unreviewed/changed first, then reviewed.
        self.state.files.sort_by(|a, b| {
            let a_reviewed = matches!(a.status, ReviewStatus::Reviewed { .. });
            let b_reviewed = matches!(b.status, ReviewStatus::Reviewed { .. });
            a_reviewed
                .cmp(&b_reviewed)
                .then(a.change.path.cmp(&b.change.path))
        });

        if was_marking {
            let unreviewed_count = self.state.unreviewed_count();
            if unreviewed_count > 0 {
                // Try to select the file that was next in line.
                // Fall back to the first unreviewed file if that target
                // no longer exists or was already reviewed.
                self.state.selected_file = advance_target
                    .and_then(|path| self.state.files.iter().position(|f| f.change.path == path))
                    .unwrap_or(0);
            } else {
                // All reviewed — stay on the file we just reviewed.
                self.state.selected_file = self
                    .state
                    .files
                    .iter()
                    .position(|f| f.change.path == result.file_path)
                    .unwrap_or(0);
            }
        } else {
            // Un-marking: stay on the same file by path.
            self.state.selected_file = self
                .state
                .files
                .iter()
                .position(|f| f.change.path == result.file_path)
                .unwrap_or(0);
        }

        self.state.on_file_changed();
    }

    /// Select the word under the given terminal position and copy to clipboard.
    ///
    /// Reads directly from the terminal buffer so it works correctly
    /// regardless of blame columns, gutters, or other prefix spans.
    fn select_word_at(&mut self, pane: PaneFocus, col: u16, row: u16) {
        let pane_area = match pane {
            PaneFocus::FileList => self.state.file_list_area,
            PaneFocus::Diff => self.state.diff_area,
        };

        let inner_left = pane_area.x + 1;
        let inner_right = pane_area.right().saturating_sub(1);

        if col < inner_left || col >= inner_right {
            return;
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

        // Find the character index at the clicked column.
        let click_idx = row_chars.iter().position(|&(x, _)| x == col);
        let click_idx = match click_idx {
            Some(i) => i,
            None => return,
        };

        let is_word_char = |c: char| c.is_alphanumeric() || c == '_';

        if !is_word_char(row_chars[click_idx].1) {
            return;
        }

        // Expand to word boundaries.
        let mut start = click_idx;
        while start > 0 && is_word_char(row_chars[start - 1].1) {
            start -= 1;
        }
        let mut end = click_idx;
        while end + 1 < row_chars.len() && is_word_char(row_chars[end + 1].1) {
            end += 1;
        }

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
            });
        }
    }

    /// Map a mouse click in the file list pane to a file selection.
    fn handle_file_list_click(&mut self, _col: u16, row: u16) {
        let area = self.state.file_list_area;
        let inner_top = area.y + 1; // skip border

        // Convert screen row to content row (accounting for scroll).
        let content_row = (row as usize)
            .saturating_sub(inner_top as usize)
            + self.state.file_list_scroll;

        // Look up which file (if any) this row corresponds to.
        if let Some(&Some(file_idx)) = self.state.file_list_row_to_file.get(content_row) {
            if file_idx < self.state.files.len() && file_idx != self.state.selected_file {
                self.state.selected_file = file_idx;
                self.state.on_file_changed();
            }
        }
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
        let content_idx = (screen_row as usize)
            .saturating_sub(inner_top as usize)
            + scroll;
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
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
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
        LeaveAlternateScreen
    )
    .context("Failed to leave alternate screen")?;
    terminal.show_cursor().context("Failed to show cursor")?;
    Ok(())
}

/// Emergency terminal restore without a Terminal handle (for panic hook).
fn restore_terminal_raw() {
    let _ = terminal::disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        DisableMouseCapture,
        LeaveAlternateScreen
    );
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
        };
        assert_eq!(sel.normalized(), (5, 2, 10, 4));
    }
}
