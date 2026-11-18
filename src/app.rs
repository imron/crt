//! Application state and input dispatch.

use ratatui::layout::Rect;

pub mod model;

use self::model::AppModel;
use crate::config::Config;
use crate::core::diff;
use crate::core::interaction::CoreInteractionEngine;
use crate::core::review;
use crate::core::{PaneId, PointerSemanticHit, TextAnchor};
use crate::model::{ConnectionContext, ContentMode, FileEntry, PaneFocus, RenderVariant};

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
// Application state
// ---------------------------------------------------------------------------

/// Application core owned by UI frontends.
///
/// Runtime configuration lives with the app core, while `AppState` remains the
/// mutable review/session state.
pub struct App {
    pub state: AppState,
    pub config: Config,
}

impl App {
    pub fn new(config: Config, context: ConnectionContext, files: Vec<FileEntry>) -> Self {
        let diff_algorithm =
            diff::resolve_diff_algorithm(&context.worktree, config.layout.diff_algorithm);
        let state = AppState::new(diff_algorithm, context, files);

        Self { state, config }
    }

    pub fn model(&self) -> AppModel {
        AppModel::from_state(&self.state)
    }
}

/// Central application state for review data and domain interaction.
/// Input events mutate it, sometimes by sending requests to the server.
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
    /// Jump stack for Ctrl-] / Ctrl-t navigation.
    pub jump_stack: Vec<JumpLocation>,
    /// Active diff search query (the confirmed search term).
    pub diff_search_query: Option<String>,
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
}

impl AppState {
    pub fn new(
        diff_algorithm: crate::config::DiffAlgorithm,
        context: ConnectionContext,
        mut files: Vec<FileEntry>,
    ) -> Self {
        review::sort_files(&mut files);

        Self {
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
            diff_rendered_text: Vec::new(),
            file_list_rendered_text: Vec::new(),
            file_list_row_to_file: Vec::new(),
            file_list_scroll: 0,
            file_list_area: Rect::default(),
            diff_area: Rect::default(),
            diff_algorithm,
            default_diff_algorithm: diff_algorithm,
            ignore_whitespace: false,
            show_blame: false,
            head_blame: Vec::new(),
            base_blame: Vec::new(),
            jump_stack: Vec::new(),
            diff_search_query: None,
            diff_search_matches: Vec::new(),
            diff_search_current: 0,
            show_merge_base: false,
            core_interaction: CoreInteractionEngine::new(),
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

    /// Determine which pane a screen coordinate falls in.
    pub fn pane_at(
        &self,
        col: u16,
        row: u16,
        show_file_list: bool,
        show_diff_pane: bool,
    ) -> Option<PaneFocus> {
        if show_file_list && self.file_list_area.contains((col, row).into()) {
            Some(PaneFocus::FileList)
        } else if show_diff_pane && self.diff_area.contains((col, row).into()) {
            Some(PaneFocus::Diff)
        } else {
            None
        }
    }

    pub fn area_for_pane(&self, pane: PaneFocus) -> Rect {
        match pane {
            PaneFocus::FileList => self.file_list_area,
            PaneFocus::Diff => self.diff_area,
        }
    }

    pub fn pointer_semantic_hit(
        &self,
        pane: PaneFocus,
        column: u16,
        row: u16,
    ) -> Option<PointerSemanticHit> {
        let pane_id = match pane {
            PaneFocus::FileList => PaneId::FileList,
            PaneFocus::Diff => PaneId::Diff,
        };

        let text_anchor = self.pointer_text_anchor_for_pane(pane, column, row, false);

        Some(PointerSemanticHit {
            pane_id,
            region_id: self.pointer_region_id(pane, text_anchor),
            text_anchor,
        })
    }

    pub fn pointer_text_anchor_for_pane(
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
    pub fn refresh_current_file_diff(&mut self) {
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
            crate::config::DiffAlgorithm::Myers,
            test_context(),
            vec![test_file("src/lib.rs")],
        )
    }

    #[test]
    fn app_owns_config_and_state() {
        let config = Config::default();
        let expected_width = config.layout.file_list_width;

        let app = App::new(config, test_context(), vec![test_file("src/lib.rs")]);

        assert_eq!(app.config.layout.file_list_width, expected_width);
        assert_eq!(app.state.files.len(), 1);
    }

    #[test]
    fn pointer_semantic_hit_maps_file_list_anchor() {
        let mut state = test_state();
        state.file_list_area = Rect::new(0, 0, 30, 10);
        state.file_list_row_to_file = vec![Some(0)];

        let hit = state
            .pointer_semantic_hit(PaneFocus::FileList, 5, 1)
            .expect("expected pointer hit");

        assert_eq!(
            hit,
            PointerSemanticHit {
                pane_id: PaneId::FileList,
                region_id: Some("file:src/lib.rs".to_string()),
                text_anchor: Some(TextAnchor { line: 0, column: 4 }),
            }
        );
    }

    #[test]
    fn pointer_semantic_hit_maps_diff_anchor() {
        let mut state = test_state();
        state.diff_area = Rect::new(0, 0, 80, 20);
        state.diff_scroll = 10;
        state.diff_gutter_cols = 4;

        let hit = state
            .pointer_semantic_hit(PaneFocus::Diff, 12, 3)
            .expect("expected pointer hit");

        assert_eq!(
            hit,
            PointerSemanticHit {
                pane_id: PaneId::Diff,
                region_id: Some("diff-line:12".to_string()),
                text_anchor: Some(TextAnchor {
                    line: 12,
                    column: 4,
                }),
            }
        );
    }
}
