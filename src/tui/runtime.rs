//! Terminal UI runtime.
//!
//! Owns terminal setup, event polling, rendering, mouse handling, and the
//! server client used by the interactive TUI.

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

use crate::app::{
    AppState, DefinitionResults, JumpLocation, LastPointerClick, MouseSelection, SearchResults,
};
use crate::client::Client;
use crate::core::command::Command;
use crate::core::diff;
use crate::core::review;
use crate::core::search as core_search;
use crate::core::{InputEvent, PaneId, TextAnchor};
use crate::model::{ConnectionContext, PaneFocus, ReviewStatus};
use crate::tui::{TuiState, apply_core_effects, input, render};

/// The terminal UI runtime. Owns the terminal, client connection, and state.
pub struct Tui {
    pub state: AppState,
    tui_state: TuiState,
    client: Client,
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl Tui {
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
            tui_state: TuiState::default(),
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
            let tui_state = &mut self.tui_state;
            self.terminal
                .draw(|frame| render::draw(frame, state, tui_state))?;

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
            if let Some(cmd) = self.state.pending_command.take() {
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
                input::handle_key_event(&mut self.state, &mut self.tui_state, key);
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

                apply_core_effects(&mut self.state, &mut self.tui_state, pending_core_effects);

                if let Some((pane, anchor)) = semantic_content_hit {
                    // Record mouse-down anchor; drag starts selection.
                    self.state.mouse_selection = None;
                    self.state.mouse_down_anchor = Some((pane, anchor));
                }
            }
            _ => {
                if apply_core_effects(&mut self.state, &mut self.tui_state, pending_core_effects) {
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
                let tui_state = &mut self.tui_state;
                let _ = self
                    .terminal
                    .draw(|frame| render::draw(frame, state, tui_state));

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
                let tui_state = &mut self.tui_state;
                let _ = self
                    .terminal
                    .draw(|frame| render::draw(frame, state, tui_state));

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
                let tui_state = &mut self.tui_state;
                let _ = self
                    .terminal
                    .draw(|frame| render::draw(frame, state, tui_state));

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
                                navigate_to_location_from_tui(&mut self.state, target);
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

/// Navigate to a resolved location target from the TUI runtime context without
/// going through the key handler.
fn navigate_to_location_from_tui(state: &mut AppState, target: core_search::LocationTarget) {
    match target {
        core_search::LocationTarget::InDiff {
            file_index,
            line_number,
        } => {
            push_jump_stack_from_tui(state);
            state.selected_file = file_index;
            state.on_file_changed();
            state.diff_line_cursor = (line_number as usize).saturating_sub(1);
            state.clamp_cursor_and_scroll();
        }
        core_search::LocationTarget::External {
            file_path,
            line_number,
        } => {
            push_jump_stack_from_tui(state);
            state.status_message = Some((
                format!("Definition in file not in diff: {file_path}:{line_number}"),
                Instant::now(),
            ));
        }
    }
}

fn push_jump_stack_from_tui(state: &mut AppState) {
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
    use crate::config::StyleConfig;
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
