//! Application state, event loop, and input dispatch.
//!
//! The TUI operates as a client connected to the crt server (persistent or
//! embedded). The event loop polls for crossterm keyboard/resize events,
//! dispatches them to the key handler, and re-renders on every cycle.

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
use crate::keys;
use crate::model::{ConnectionContext, ContentMode, FileEntry, PaneFocus, RenderVariant, ReviewStatus};
use crate::ui;

/// Polling interval for crossterm events. Re-render happens after each
/// event or after this timeout (to pick up server notifications later).
const EVENT_POLL_TIMEOUT: Duration = Duration::from_millis(250);

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
    /// Whether the help overlay is visible.
    pub show_help: bool,
    /// Set to true to suspend the process (Ctrl-Z).
    pub should_suspend: bool,
    /// Set to true to exit the event loop.
    pub should_quit: bool,
}

impl AppState {
    fn new(context: ConnectionContext, mut files: Vec<FileEntry>) -> Self {
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
            context,
            files,
            // Index 0 is the first unreviewed file (due to sort order).
            selected_file: 0,
            pane_focus: PaneFocus::FileList,
            content_mode: ContentMode::Diff,
            render_variant: RenderVariant::Inline,
            diff_scroll: 0,
            diff_content_height: 0,
            diff_view_height: 0,
            show_comments: false,
            show_file_list: true,
            show_diff_pane: true,
            mouse_selection: None,
            diff_rendered_text: Vec::new(),
            file_list_rendered_text: Vec::new(),
            file_list_scroll: 0,
            file_list_area: Rect::default(),
            diff_area: Rect::default(),
            status_message: None,
            pending_ctrl_w: false,
            show_help: false,
            should_suspend: false,
            should_quit: false,
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
}

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

/// The TUI application. Owns the terminal, client connection, and state.
pub struct App {
    pub state: AppState,
    #[allow(dead_code)] // Will be used by key handlers in later stages.
    client: Client,
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl App {
    /// Create a new application, loading initial data from the server.
    pub async fn new(client: Client, context: ConnectionContext) -> Result<Self> {
        // Load file list from server.
        let files = match client.list_changed_files().await {
            Ok(result) => result.files,
            Err(e) => {
                // If the server method is unavailable, start with empty list.
                eprintln!("Warning: could not load files: {e}");
                Vec::new()
            }
        };

        let state = AppState::new(context, files);
        let terminal = setup_terminal().context("Failed to set up terminal")?;

        Ok(Self {
            state,
            client,
            terminal,
        })
    }

    /// Run the event loop. Returns when the user quits.
    pub fn run(&mut self) -> Result<()> {
        // Install a panic hook that restores the terminal before printing
        // the panic message, so the user's shell isn't left broken.
        let original_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = restore_terminal_raw();
            original_hook(info);
        }));

        let result = self.event_loop();

        // Restore terminal regardless of how the loop exited.
        let _ = restore_terminal(&mut self.terminal);

        // Remove our panic hook.
        let _ = std::panic::take_hook();

        result
    }

    /// Core event loop: render → poll → dispatch → repeat.
    fn event_loop(&mut self) -> Result<()> {
        loop {
            // Render current state.
            let state = &mut self.state;
            self.terminal.draw(|frame| ui::draw(frame, state))?;

            // Poll for a crossterm event (blocks up to EVENT_POLL_TIMEOUT).
            if event::poll(EVENT_POLL_TIMEOUT).context("Event poll failed")? {
                let ev = event::read().context("Event read failed")?;
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

            // TODO (stage 9+): check for server notifications here and
            // update state accordingly.

            if self.state.should_suspend {
                self.state.should_suspend = false;
                self.suspend()?;
            }

            if self.state.should_quit {
                break;
            }
        }

        Ok(())
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

    /// Handle mouse events: selection, scroll wheel.
    fn handle_mouse_event(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Start a new selection in whichever pane was clicked.
                if let Some(pane) = self.state.pane_at(mouse.column, mouse.row) {
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
                // Extend the selection, clamped to the originating pane.
                if let Some(sel) = &mut self.state.mouse_selection {
                    let area = sel.pane_area;
                    sel.end_col = mouse.column.clamp(area.x + 1, area.right().saturating_sub(2));
                    sel.end_row = mouse.row.clamp(area.y + 1, area.bottom().saturating_sub(2));
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
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

        let col_start = if screen_row == start_row {
            (start_col as usize).saturating_sub(inner_left as usize)
        } else {
            0
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
