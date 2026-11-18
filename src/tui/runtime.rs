//! Terminal UI runtime.
//!
//! Owns terminal setup, event polling, rendering, mouse handling, and
//! terminal-specific presentation state.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
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
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::effects::apply_core_effects;
use super::input::{CoreInputDispatch, KeyInputResult};
use super::state::{InputMode, LastPointerClick, MouseSelection, STATUS_MSG_TIMEOUT, TuiState};
use super::{input, render};
use crate::app::{App, VisualSelection};
use crate::core::{
    CoreEffect, InputEvent, InteractionContext, PaneId, TextAnchor, VisualSelectionEffect,
};
use crate::review_types::PaneFocus;

/// How long the "Press Ctrl-C again" prompt stays active.
const CTRL_C_TIMEOUT: Duration = Duration::from_secs(3);

/// How often the TUI gives the app a chance to process background work while
/// waiting for terminal input.
const APP_TICK_INTERVAL: Duration = Duration::from_millis(100);
const COMMENT_EDITOR_SEPARATOR: &str = "------ Write your comment after this line ----";

/// The terminal UI runtime. Owns terminal interaction and presentation state.
pub struct Tui {
    app: App,
    tui_state: TuiState,
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

struct EventPump {
    tx: UnboundedSender<Event>,
    rx: UnboundedReceiver<Event>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl EventPump {
    fn start() -> Self {
        let (tx, rx) = unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let handle = Some(spawn_event_poll_thread(tx.clone(), Arc::clone(&stop)));
        Self {
            tx,
            rx,
            stop,
            handle,
        }
    }

    async fn recv(&mut self) -> Option<Event> {
        self.rx.recv().await
    }

    fn try_recv(&mut self) -> Result<Event, tokio::sync::mpsc::error::TryRecvError> {
        self.rx.try_recv()
    }

    fn stop_polling(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    fn restart(&mut self) {
        self.stop_polling();
        self.stop = Arc::new(AtomicBool::new(false));
        self.handle = Some(spawn_event_poll_thread(
            self.tx.clone(),
            Arc::clone(&self.stop),
        ));
    }

    fn drain(&mut self) {
        while self.rx.try_recv().is_ok() {}
    }
}

fn spawn_event_poll_thread(tx: UnboundedSender<Event>, stop: Arc<AtomicBool>) -> JoinHandle<()> {
    thread::spawn(move || {
        loop {
            if stop.load(Ordering::Acquire) || tx.is_closed() {
                break;
            }
            if event::poll(Duration::from_millis(50)).unwrap_or(false) {
                if let Ok(ev) = event::read() {
                    if tx.send(ev).is_err() {
                        break;
                    }
                }
            }
        }
    })
}

impl Tui {
    /// Create a terminal runtime for an already-loaded app.
    pub fn new(app: App) -> Result<Self> {
        let terminal = setup_terminal().context("Failed to set up terminal")?;

        Ok(Self {
            app,
            tui_state: TuiState::new(),
            terminal,
        })
    }

    pub fn into_app(self) -> App {
        self.app
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

    /// Async event loop: render app model, wait for input or app tick, update.
    ///
    /// Terminal events are polled on a blocking thread and sent over an
    /// async channel, keeping the tokio runtime responsive for server
    /// calls and notifications.
    async fn event_loop(&mut self) -> Result<()> {
        let mut event_pump = EventPump::start();

        let mut last_rendered_revision = None;
        let mut tui_dirty = true;

        loop {
            let current_revision = self.app.state.model_revision();
            if tui_dirty || last_rendered_revision != Some(current_revision) {
                last_rendered_revision = Some(self.render_current_frame()?);
                tui_dirty = false;
            }

            tokio::select! {
                maybe_ev = event_pump.recv() => {
                    match maybe_ev {
                        Some(ev) => {
                            tui_dirty = true;
                            self.handle_event(ev, &mut event_pump);

                            while let Ok(ev) = event_pump.try_recv() {
                                self.handle_event(ev, &mut event_pump);
                            }
                        }
                        None => break,
                    }
                }
                _ = tokio::time::sleep(APP_TICK_INTERVAL) => {}
            }

            if self.tui_state.expire_status_message(STATUS_MSG_TIMEOUT) {
                tui_dirty = true;
            }

            let status = self.app.process_background_work().await;
            if status.is_some() {
                tui_dirty = true;
            }
            self.tui_state.apply_status_update(status);

            if self.tui_state.should_suspend {
                self.tui_state.should_suspend = false;
                tui_dirty = true;
                event_pump.stop_polling();
                event_pump.drain();
                self.suspend()?;
                event_pump.restart();
                event_pump.drain();
            }

            if self.tui_state.should_quit {
                break;
            }
        }

        event_pump.stop_polling();

        Ok(())
    }

    /// Dispatch a single terminal event.
    fn handle_event(&mut self, ev: Event, event_pump: &mut EventPump) {
        match ev {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if self.tui_state.input_mode != InputMode::Comment {
                    self.tui_state.mouse_selection = None;
                    self.tui_state.mouse_down_anchor = None;
                }
                match input::handle_key_event(&mut self.tui_state, key) {
                    KeyInputResult::Core(dispatch) => {
                        let handled = self.dispatch_core_input(dispatch);
                        if !handled {
                            self.tui_state.clear_status_message();
                        }
                    }
                    KeyInputResult::OpenEditor => {
                        if let Err(e) = self.edit_comment_in_editor(event_pump) {
                            self.tui_state
                                .set_status_message(format!("Editor error: {e}"));
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

    fn render_current_frame(&mut self) -> Result<u64> {
        let model = self.app.model();
        let revision = model.revision;
        let styles = &self.app.config.style;
        let tui_state = &mut self.tui_state;
        self.terminal
            .draw(|frame| render::draw(frame, &model, tui_state, styles))?;
        let before = (
            self.app.state.diff_line_cursor,
            self.app.state.diff_scroll,
            self.app.state.file_list_scroll,
        );
        self.tui_state.clamp_cursor_and_scroll(&mut self.app.state);
        self.tui_state.clamp_file_list_scroll(&mut self.app.state);
        let after = (
            self.app.state.diff_line_cursor,
            self.app.state.diff_scroll,
            self.app.state.file_list_scroll,
        );
        if before != after {
            self.app.state.mark_model_changed();
        }
        if self.app.refresh_active_diff_search(&self.tui_state) {
            return Ok(self.app.state.model_revision());
        }
        Ok(revision)
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

    fn edit_comment_in_editor(&mut self, event_pump: &mut EventPump) -> Result<()> {
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
        let path = comment_editor_path();
        let (document, template) = comment_editor_document(
            self.app.state.pending_comment_anchor.as_ref(),
            &self.tui_state.comment_input,
        );
        std::fs::write(&path, document)
            .with_context(|| format!("Failed to write {}", path.display()))?;

        event_pump.stop_polling();
        event_pump.drain();
        let edit_result = (|| -> Result<()> {
            drain_terminal_events()?;
            restore_terminal(&mut self.terminal)?;
            io::stdout().flush().context("Failed to flush stdout")?;
            let status = ProcessCommand::new(&editor)
                .arg(&path)
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status();
            drain_terminal_events()?;
            self.terminal = setup_terminal().context("Failed to re-setup terminal after editor")?;
            let status = status.with_context(|| format!("Failed to launch editor `{editor}`"))?;

            if !status.success() {
                let _ = std::fs::remove_file(&path);
                anyhow::bail!("editor exited with status {status}");
            }

            let body = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            let body = strip_comment_editor_template(body, &template);
            self.tui_state.comment_cursor = body.len();
            self.tui_state.comment_input = body;
            Ok(())
        })();
        event_pump.restart();
        event_pump.drain();
        let _ = std::fs::remove_file(&path);
        edit_result
    }

    /// Check if a mouse column is on the border between file list and diff panes.
    fn is_on_pane_border(&self, col: u16, row: u16) -> bool {
        let model = self.app.model();
        if !model.layout.file_list_visible || !model.layout.diff_visible {
            return false;
        }
        let border_col = self.tui_state.file_list_area.right().saturating_sub(1);
        col == border_col
            && row >= self.tui_state.file_list_area.y
            && row < self.tui_state.file_list_area.bottom()
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
                    self.cancel_visual_selection();
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
                    // Record mouse-down anchor; drag starts selection. A
                    // plain click still cancels any prior visual selection.
                    self.tui_state.mouse_selection = None;
                    self.tui_state.mouse_down_anchor = Some((pane, anchor));
                    self.cancel_visual_selection();
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
                            self.app.set_file_list_width(new_width);
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
                            if let Some((start_pane, start)) = self.tui_state.mouse_down_anchor {
                                if start_pane == pane {
                                    match pane {
                                        PaneFocus::Diff => {
                                            apply_core_effects(
                                                &mut self.app,
                                                &mut self.tui_state,
                                                vec![
                                                    CoreEffect::VisualSelection(
                                                        VisualSelectionEffect::StartText {
                                                            anchor: start,
                                                        },
                                                    ),
                                                    CoreEffect::VisualSelection(
                                                        VisualSelectionEffect::ExtendTo { anchor },
                                                    ),
                                                ],
                                            );
                                        }
                                        PaneFocus::FileList => {
                                            if let Some(sel) = &mut self.tui_state.mouse_selection {
                                                sel.end = anchor;
                                            } else {
                                                self.tui_state.mouse_selection =
                                                    Some(MouseSelection {
                                                        pane,
                                                        start,
                                                        end: anchor,
                                                        word_selected: false,
                                                    });
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    MouseEventKind::Up(MouseButton::Left) => {
                        if self.tui_state.dragging_border {
                            self.tui_state.dragging_border = false;
                            return;
                        }
                        let mouse_down_anchor = self.tui_state.mouse_down_anchor.take();
                        if matches!(mouse_down_anchor, Some((PaneFocus::Diff, _))) {
                            if let Some(selection) = self.app.state.visual_selection.as_ref() {
                                let text =
                                    extract_diff_visual_selection_text(&self.tui_state, selection);
                                if !text.is_empty() {
                                    copy_to_clipboard(&text);
                                }
                            }
                            return;
                        }
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

    fn cancel_visual_selection(&mut self) {
        if self.app.state.visual_selection.is_some() {
            apply_core_effects(
                &mut self.app,
                &mut self.tui_state,
                vec![CoreEffect::VisualSelection(VisualSelectionEffect::Cancel)],
            );
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
        if pane == PaneFocus::Diff {
            apply_core_effects(
                &mut self.app,
                &mut self.tui_state,
                vec![
                    CoreEffect::VisualSelection(VisualSelectionEffect::StartText {
                        anchor: start_anchor,
                    }),
                    CoreEffect::VisualSelection(VisualSelectionEffect::ExtendTo {
                        anchor: end_anchor,
                    }),
                ],
            );
        } else {
            self.tui_state.mouse_selection = Some(MouseSelection {
                pane,
                start: start_anchor,
                end: end_anchor,
                word_selected: true,
            });
        }
        self.tui_state
            .set_status_message(format!("Copied identifier: {word}"));
        true
    }

    /// Double-click in the file list: copy the full file path to the clipboard.
    fn copy_file_path_at(&mut self, row: usize) {
        if let Some(&Some(file_idx)) = self.tui_state.file_list_row_to_file.get(row) {
            let model = self.app.model();
            let path = model
                .file_list
                .sections
                .iter()
                .flat_map(|section| section.rows.iter())
                .find(|row| row.file_index == file_idx)
                .map(|row| row.path.as_str());
            if let Some(path) = path {
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

fn comment_editor_path() -> PathBuf {
    std::env::temp_dir().join(format!("crt-comment-{}.md", std::process::id()))
}

fn comment_editor_document(
    anchor: Option<&crate::app::CommentAnchorCapture>,
    body: &str,
) -> (String, String) {
    let Some(anchor) = anchor else {
        return (body.to_string(), String::new());
    };

    let mut prefix = String::new();
    if !anchor.context_before.is_empty() {
        prefix.push_str(&anchor.context_before);
        prefix.push('\n');
    }
    prefix.push_str(&anchor.anchor_text);
    prefix.push('\n');
    if !anchor.context_after.is_empty() {
        prefix.push_str(&anchor.context_after);
        prefix.push('\n');
    }
    prefix.push('\n');
    prefix.push_str(COMMENT_EDITOR_SEPARATOR);
    prefix.push('\n');

    let mut document = prefix.clone();
    document.push_str(body);
    (document, prefix)
}

fn strip_comment_editor_template(body: String, prefix: &str) -> String {
    if prefix.is_empty() {
        return body;
    }
    body.strip_prefix(prefix)
        .map_or(body.clone(), ToString::to_string)
}

fn drain_terminal_events() -> Result<()> {
    while event::poll(Duration::ZERO).context("Failed to poll terminal events")? {
        let _ = event::read().context("Failed to read terminal event")?;
    }
    Ok(())
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

fn extract_diff_visual_selection_text(tui_state: &TuiState, selection: &VisualSelection) -> String {
    let sel = MouseSelection {
        pane: PaneFocus::Diff,
        start: selection.start,
        end: selection.end,
        word_selected: false,
    };
    extract_selected_text(tui_state, &sel)
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
    fn extract_diff_visual_selection_uses_app_selection_anchors() {
        let mut tui_state = TuiState::default();
        tui_state.diff_gutter_cols = 4;
        tui_state.diff_rendered_text = vec![
            "     + first line".to_string(),
            "       second line".to_string(),
        ];
        let selection = VisualSelection {
            mode: crate::app::VisualSelectionMode::Text,
            start: TextAnchor { line: 0, column: 0 },
            end: TextAnchor { line: 1, column: 5 },
        };

        assert_eq!(
            extract_diff_visual_selection_text(&tui_state, &selection),
            "first line\nsecond"
        );
    }

    #[test]
    fn editor_document_wraps_comment_with_anchor_context() {
        let anchor = crate::app::CommentAnchorCapture {
            file_path: "src/main.rs".to_string(),
            line_start: 4,
            line_end: 5,
            char_start: None,
            char_end: None,
            anchor_text: "selected one\nselected two".to_string(),
            context_before: "before".to_string(),
            context_after: "after".to_string(),
        };

        let (document, prefix) = comment_editor_document(Some(&anchor), "draft");

        assert_eq!(
            document,
            "before\n\
             selected one\n\
             selected two\n\
             after\n\
             \n\
             ------ Write your comment after this line ----\n\
             draft"
        );
        assert_eq!(strip_comment_editor_template(document, &prefix), "draft");
    }

    #[test]
    fn editor_template_strip_keeps_changed_context() {
        let anchor = crate::app::CommentAnchorCapture {
            file_path: "src/main.rs".to_string(),
            line_start: 4,
            line_end: 4,
            char_start: None,
            char_end: None,
            anchor_text: "selected".to_string(),
            context_before: String::new(),
            context_after: String::new(),
        };
        let (_document, prefix) = comment_editor_document(Some(&anchor), "draft");
        let edited = "changed\n\
                      ------ Write your comment after this line ----\n\
                      draft"
            .to_string();

        assert_eq!(
            strip_comment_editor_template(edited.clone(), &prefix),
            edited
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
