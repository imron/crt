//! Terminal UI runtime.
//!
//! Owns terminal setup, event polling, rendering, mouse handling, and
//! terminal-specific presentation state.

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

use super::effects::apply_core_effects;
use super::input::{CoreInputDispatch, KeyInputResult};
use super::state::{DefinitionResults, LastPointerClick, MouseSelection, SearchResults, TuiState};
use super::{input, render};
use crate::app::{App, AppState, JumpLocation};
use crate::core::command::Command;
use crate::core::search as core_search;
use crate::core::{CoreEffect, InputEvent, InteractionContext, PaneId, TextAnchor};
use crate::review_types::PaneFocus;

/// How long the "Press Ctrl-C again" prompt stays active.
const CTRL_C_TIMEOUT: Duration = Duration::from_secs(3);

/// The terminal UI runtime. Owns terminal interaction and presentation state.
pub struct Tui {
    app: App,
    tui_state: TuiState,
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl Tui {
    /// Create a terminal runtime for an already-loaded app.
    pub fn new(app: App) -> Result<Self> {
        let terminal = setup_terminal().context("Failed to set up terminal")?;
        let file_list_width = app.config.layout.file_list_width;

        Ok(Self {
            app,
            tui_state: TuiState::new(file_list_width),
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
            self.render_current_frame()?;

            // Wait for the next terminal event, with a timeout so that
            // transient status messages get cleared by re-rendering.
            let ev = if self.tui_state.has_status_message() {
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

            self.process_app_background_work().await;

            // Process pending command (search, definition, etc.).
            if let Some(cmd) = self.tui_state.pending_command.take() {
                self.process_pending_command(cmd).await;
            }

            self.process_app_background_work().await;

            if self.tui_state.should_suspend {
                self.tui_state.should_suspend = false;
                self.suspend()?;
            }

            if self.tui_state.should_quit {
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
                self.tui_state.mouse_selection = None;
                self.tui_state.mouse_down_anchor = None;
                match input::handle_key_event(&mut self.tui_state, key) {
                    KeyInputResult::Core(dispatch) => {
                        let handled = self.dispatch_core_input(dispatch);
                        if !handled {
                            self.tui_state.clear_status_message();
                        }
                    }
                    KeyInputResult::Local => {}
                    KeyInputResult::Unhandled => {
                        self.tui_state.clear_status_message();
                    }
                }
            }
            Event::Mouse(mouse) => {
                self.handle_mouse_event(mouse);
            }
            Event::Resize(_w, _h) => {
                // ratatui handles resize automatically on next draw.
            }
            Event::FocusGained => {
                let dispatch = CoreInputDispatch::Interaction(InputEvent::FocusGained);
                self.dispatch_core_input(dispatch);
            }
            _ => {}
        }
    }

    fn dispatch_core_input(&mut self, dispatch: CoreInputDispatch) -> bool {
        let effects = self.core_effects_for_input(dispatch);
        apply_core_effects(&mut self.app, &mut self.tui_state, effects)
    }

    fn render_current_frame(&mut self) -> Result<()> {
        let model = self.app.model();
        let styles = &self.app.config.style;
        let tui_state = &mut self.tui_state;
        self.terminal
            .draw(|frame| render::draw(frame, &model, tui_state, styles))?;
        self.tui_state.clamp_cursor_and_scroll(&mut self.app.state);
        self.tui_state.clamp_file_list_scroll(&mut self.app.state);
        Ok(())
    }

    fn core_effects_for_input(&mut self, dispatch: CoreInputDispatch) -> Vec<CoreEffect> {
        let (event, context) = match dispatch {
            CoreInputDispatch::Interaction(event) => (event, self.interaction_context()),
            CoreInputDispatch::PromptSubmit(event) => {
                (event, self.app.prompt_submit_context(&self.tui_state))
            }
            CoreInputDispatch::PromptCancel(event) => (event, InteractionContext::default()),
        };
        self.app.handle_input(event, &context)
    }

    fn interaction_context(&self) -> InteractionContext {
        InteractionContext {
            help_visible: self.tui_state.show_help,
            search_results_visible: self.tui_state.search_results.is_some(),
            definition_results_visible: self.tui_state.definition_results.is_some(),
            quit_confirmation_active: self.tui_state.quit_confirmation_active(CTRL_C_TIMEOUT),
            ..self.app.interaction_context()
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
        if !self.tui_state.show_file_list || !self.tui_state.show_diff_pane {
            return false;
        }
        let border_col = self.tui_state.file_list_area.right().saturating_sub(1);
        col == border_col
            && row >= self.tui_state.file_list_area.y
            && row < self.tui_state.file_list_area.bottom()
    }

    /// Persist the current file list width to the config file.
    fn save_file_list_width(&self) {
        if let Some(path) = self.app.config_path() {
            let layout = crate::config::LayoutConfig {
                file_list_width: self.tui_state.file_list_width,
                diff_algorithm: Some(self.app.state.diff_algorithm),
            };
            crate::config::save_layout(path, &layout);
        }
    }

    /// Handle mouse events: selection, scroll wheel, border drag.
    fn handle_mouse_event(&mut self, mouse: MouseEvent) {
        let model = self.app.model();
        let input_event = input::input_event_from_mouse(&model, &self.tui_state, &mouse);
        let semantic_content_hit = input_event.as_ref().and_then(mouse_content_hit);
        let pending_core_effects = input_event
            .map(|event| self.core_effects_for_input(CoreInputDispatch::Interaction(event)))
            .unwrap_or_default();

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Check if the user clicked on the pane border to start resizing.
                if self.is_on_pane_border(mouse.column, mouse.row) {
                    self.tui_state.dragging_border = true;
                    return;
                }

                // Double-click detection: select the word under cursor.
                let is_double_click = semantic_content_hit.is_some_and(|(pane, anchor)| {
                    self.tui_state.last_click.as_ref().is_some_and(|last| {
                        last.when.elapsed() < Duration::from_millis(400)
                            && last.pane == pane
                            && last.anchor == anchor
                    })
                });
                self.tui_state.last_click =
                    semantic_content_hit.map(|(pane, anchor)| LastPointerClick {
                        when: Instant::now(),
                        pane,
                        anchor,
                    });

                if is_double_click {
                    // Clear any selection from the first click so the
                    // subsequent Up event doesn't overwrite the clipboard.
                    self.tui_state.mouse_selection = None;
                    self.tui_state.mouse_down_anchor = None;
                    if let Some((pane, anchor)) = semantic_content_hit {
                        if pane == PaneFocus::FileList {
                            self.copy_file_path_at(anchor.line);
                        } else {
                            self.select_word_at(pane, anchor);
                        }
                    }
                    self.tui_state.last_click = None; // prevent triple-click
                    return; // skip drag selection setup
                }

                apply_core_effects(&mut self.app, &mut self.tui_state, pending_core_effects);

                if let Some((pane, anchor)) = semantic_content_hit {
                    // Record mouse-down anchor; drag starts selection.
                    self.tui_state.mouse_selection = None;
                    self.tui_state.mouse_down_anchor = Some((pane, anchor));
                }
            }
            _ => {
                if apply_core_effects(&mut self.app, &mut self.tui_state, pending_core_effects) {
                    return;
                }

                match mouse.kind {
                    MouseEventKind::Drag(MouseButton::Left) => {
                        if self.tui_state.dragging_border {
                            // Resize the file list pane. Minimum 10, maximum
                            // terminal width minus 20.
                            let min_w = 10u16;
                            let max_w = self.tui_state.file_list_area.width
                                + self.tui_state.diff_area.width
                                - 20;
                            let new_width = (mouse.column + 1).clamp(min_w, max_w);
                            self.tui_state.file_list_width = new_width;
                            return;
                        }
                        let drag_content_hit = semantic_content_hit.or_else(|| {
                            let (pane, _) = self.tui_state.mouse_down_anchor?;
                            self.tui_state
                                .pointer_text_anchor_for_pane(
                                    &model,
                                    pane,
                                    mouse.column,
                                    mouse.row,
                                    true,
                                )
                                .map(|anchor| (pane, anchor))
                        });

                        // Extend the selection in semantic coordinates, clamped to
                        // the originating pane when the pointer leaves its content.
                        if let Some((pane, anchor)) = drag_content_hit {
                            if let Some(sel) = &mut self.tui_state.mouse_selection {
                                if sel.pane == pane {
                                    sel.end = anchor;
                                }
                            } else if let Some((start_pane, start)) =
                                self.tui_state.mouse_down_anchor
                            {
                                if start_pane == pane {
                                    self.tui_state.mouse_selection = Some(MouseSelection {
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
                        if self.tui_state.dragging_border {
                            self.tui_state.dragging_border = false;
                            self.save_file_list_width();
                            return;
                        }
                        self.tui_state.mouse_down_anchor = None;
                        // Finish selection: extract text and copy to clipboard.
                        if let Some(sel) = self.tui_state.mouse_selection.take() {
                            if !sel.word_selected {
                                // Only re-extract for drag selections — word selections
                                // were already copied by select_word_at().
                                let text = extract_selected_text(&self.tui_state, &sel);
                                if !text.is_empty() {
                                    copy_to_clipboard(&text);
                                }
                            }
                            // Keep the selection visible until next keypress.
                            self.tui_state.mouse_selection = Some(sel);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    async fn process_app_background_work(&mut self) {
        let status = self.app.process_background_work().await;
        self.tui_state.apply_status_update(status);
    }

    /// Process a pending command set by the key handler.
    async fn process_pending_command(&mut self, cmd: Command) {
        match cmd {
            Command::SearchAll { pattern } => {
                self.tui_state.set_status_message("Searching...");
                // Force a re-render so the user sees the searching message.
                let _ = self.render_current_frame();

                match self.app.search_codebase(&pattern, "all").await {
                    Ok(result) => match core_search::search_outcome(&pattern, false, result) {
                        core_search::SearchOutcome::NoMatches => {
                            self.tui_state
                                .set_status_message(format!("No matches for /{pattern}/"));
                        }
                        core_search::SearchOutcome::ShowResults {
                            query,
                            diff_only,
                            matches,
                        } => {
                            self.tui_state.search_results = Some(SearchResults {
                                query,
                                diff_only,
                                matches,
                                selected: 0,
                                scroll: 0,
                            });
                            self.tui_state.clear_status_message();
                        }
                    },
                    Err(e) => {
                        self.tui_state
                            .set_status_message(format!("Search error: {e}"));
                    }
                }
            }
            Command::SearchDiff { pattern } => {
                self.tui_state.set_status_message("Searching diff files...");
                let _ = self.render_current_frame();

                match self.app.search_codebase(&pattern, "diff").await {
                    Ok(result) => match core_search::search_outcome(&pattern, true, result) {
                        core_search::SearchOutcome::NoMatches => {
                            self.tui_state
                                .set_status_message(format!("No matches for /{pattern}/ in diff"));
                        }
                        core_search::SearchOutcome::ShowResults {
                            query,
                            diff_only,
                            matches,
                        } => {
                            self.tui_state.search_results = Some(SearchResults {
                                query,
                                diff_only,
                                matches,
                                selected: 0,
                                scroll: 0,
                            });
                            self.tui_state.clear_status_message();
                        }
                    },
                    Err(e) => {
                        self.tui_state
                            .set_status_message(format!("Search error: {e}"));
                    }
                }
            }
            Command::FindDefinition { symbol } => {
                self.tui_state.set_status_message("Finding definition...");
                let _ = self.render_current_frame();

                let context_file = self
                    .app
                    .state
                    .selected_file_entry()
                    .map(|e| e.change.path.clone());
                match self
                    .app
                    .find_definition(&symbol, context_file.as_deref())
                    .await
                {
                    Ok(result) => {
                        match core_search::definition_outcome(
                            &symbol,
                            result,
                            &self.app.state.files,
                        ) {
                            core_search::DefinitionOutcome::NoDefinitions => {
                                self.tui_state.set_status_message(format!(
                                    "No definitions found for '{symbol}'"
                                ));
                            }
                            core_search::DefinitionOutcome::Navigate(target) => {
                                self.tui_state.clear_status_message();
                                navigate_to_location_from_tui(
                                    &mut self.app.state,
                                    &mut self.tui_state,
                                    target,
                                );
                            }
                            core_search::DefinitionOutcome::ShowResults {
                                symbol,
                                definitions,
                            } => {
                                self.tui_state.definition_results = Some(DefinitionResults {
                                    symbol,
                                    definitions,
                                    selected: 0,
                                });
                                self.tui_state.clear_status_message();
                            }
                        }
                    }
                    Err(e) => {
                        self.tui_state
                            .set_status_message(format!("Definition error: {e}"));
                    }
                }
            }
            Command::ViewFile { path, line_number } => {
                // view_file <path> <line>
                // For now, show a status message since read-only view
                // for non-diff files would require a separate content mode.
                self.tui_state
                    .set_status_message(format!("File not in diff: {path} {line_number}"));
            }
            Command::Quit
            | Command::SetBlame(_)
            | Command::SetComments(_)
            | Command::SetWhitespaceIgnored(_)
            | Command::Unknown { .. } => {
                self.tui_state
                    .set_status_message("Unsupported pending command");
            }
        }
    }

    /// Select the word under the given semantic position and copy it.
    fn select_word_at(&mut self, pane: PaneFocus, anchor: TextAnchor) -> bool {
        let (text, base_col) = match pane {
            PaneFocus::FileList => (&self.tui_state.file_list_rendered_text, 0),
            PaneFocus::Diff => (
                &self.tui_state.diff_rendered_text,
                self.tui_state.diff_content_start_col(),
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
        self.tui_state.mouse_selection = Some(MouseSelection {
            pane,
            start: start_anchor,
            end: end_anchor,
            word_selected: true,
        });
        self.tui_state
            .set_status_message(format!("Copied identifier: {word}"));
        true
    }

    /// Double-click in the file list: copy the full file path to the clipboard.
    fn copy_file_path_at(&mut self, row: usize) {
        if let Some(&Some(file_idx)) = self.tui_state.file_list_row_to_file.get(row) {
            if let Some(entry) = self.app.state.files.get(file_idx) {
                let path = &entry.change.path;
                copy_to_clipboard(path);
                self.tui_state.set_status_message(format!("Copied: {path}"));
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
fn navigate_to_location_from_tui(
    state: &mut AppState,
    tui_state: &mut TuiState,
    target: core_search::LocationTarget,
) {
    match target {
        core_search::LocationTarget::InDiff {
            file_index,
            line_number,
        } => {
            push_jump_stack_from_tui(state);
            state.selected_file = file_index;
            state.on_file_changed();
            state.diff_line_cursor = (line_number as usize).saturating_sub(1);
            tui_state.clamp_cursor_and_scroll(state);
        }
        core_search::LocationTarget::External {
            file_path,
            line_number,
        } => {
            push_jump_stack_from_tui(state);
            tui_state.set_status_message(format!(
                "Definition in file not in diff: {file_path}:{line_number}"
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

/// Extract the selected text from the rendered content stored in TUI state.
fn extract_selected_text(tui_state: &TuiState, sel: &MouseSelection) -> String {
    let (text, base_col) = match sel.pane {
        PaneFocus::Diff => (
            &tui_state.diff_rendered_text,
            tui_state.diff_content_start_col(),
        ),
        PaneFocus::FileList => (&tui_state.file_list_rendered_text, 0),
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
        let mut tui_state = TuiState::default();
        tui_state.file_list_rendered_text = vec![
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

        assert_eq!(extract_selected_text(&tui_state, &sel), "app.rs\nsrc/core");
    }

    #[test]
    fn extract_selected_text_uses_diff_content_anchors() {
        let mut tui_state = TuiState::default();
        tui_state.diff_gutter_cols = 4;
        tui_state.diff_rendered_text = vec![
            "     + first line".to_string(),
            "       second line".to_string(),
        ];

        let sel = MouseSelection {
            pane: PaneFocus::Diff,
            start: TextAnchor { line: 0, column: 0 },
            end: TextAnchor { line: 1, column: 5 },
            word_selected: false,
        };

        assert_eq!(
            extract_selected_text(&tui_state, &sel),
            "first line\nsecond"
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
