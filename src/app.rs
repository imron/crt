//! Application state and input dispatch.

pub mod model;
mod update;

use std::path::PathBuf;

use self::model::AppModel;
use crate::client::{Client, Notification};
use crate::config::Config;
use crate::core::InputEvent;
use crate::core::diff;
use crate::core::interaction::{CoreEffect, CoreInteractionEngine, InteractionContext};
use crate::core::review;
use crate::review_types::{
    ConnectionContext, ContentMode, DefinitionLocation, FileEntry, PaneFocus, RenderVariant,
    SearchMatch,
};
use anyhow::Result;

pub use update::{AppOutput, AppViewport, StatusUpdate};

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
    client: Option<Client>,
    config_path: Option<PathBuf>,
}

impl App {
    pub fn new(config: Config, context: ConnectionContext, files: Vec<FileEntry>) -> Self {
        let diff_algorithm =
            diff::resolve_diff_algorithm(&context.worktree, config.layout.diff_algorithm);
        let state = AppState::new(diff_algorithm, context, files);

        Self {
            state,
            config,
            client: None,
            config_path: None,
        }
    }

    pub async fn load(client: Client, context: ConnectionContext) -> Result<Self> {
        let config = crate::config::load();
        let files = match client.list_changed_files().await {
            Ok(result) => result.files,
            Err(e) => {
                eprintln!("Warning: could not load files: {e}");
                Vec::new()
            }
        };
        let config_path = crate::config::config_path();
        let mut app = Self::new(config, context, files);
        app.client = Some(client);
        app.config_path = config_path;
        app.state.on_file_changed();
        Ok(app)
    }

    pub fn config_path(&self) -> Option<&PathBuf> {
        self.config_path.as_ref()
    }

    fn client(&self) -> Result<&Client> {
        self.client
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("app client is not available for this workflow"))
    }

    pub async fn list_changed_files(&self) -> Result<crate::review_types::ListChangedFilesResult> {
        self.client()?.list_changed_files().await
    }

    pub async fn mark_reviewed(
        &self,
        file_path: &str,
    ) -> Result<crate::review_types::ReviewActionResult> {
        self.client()?.mark_reviewed(file_path).await
    }

    pub async fn unmark_reviewed(
        &self,
        file_path: &str,
    ) -> Result<crate::review_types::ReviewActionResult> {
        self.client()?.unmark_reviewed(file_path).await
    }

    pub async fn search_codebase(
        &self,
        pattern: &str,
        scope: &str,
    ) -> Result<crate::review_types::SearchCodebaseResult> {
        self.client()?.search_codebase(pattern, scope).await
    }

    pub async fn find_definition(
        &self,
        symbol: &str,
        context_file: Option<&str>,
    ) -> Result<crate::review_types::FindDefinitionResult> {
        self.client()?.find_definition(symbol, context_file).await
    }

    pub async fn drain_notifications(&self) -> Result<Vec<Notification>> {
        Ok(self.client()?.drain_notifications().await)
    }

    pub fn model(&self) -> AppModel {
        AppModel::from_state(&self.state)
    }

    pub fn interaction_context(&self) -> InteractionContext {
        update::interaction_context(&self.state)
    }

    pub fn prompt_submit_context(&self, viewport: &impl AppViewport) -> InteractionContext {
        update::prompt_submit_context(&self.state, viewport)
    }

    pub fn handle_input(
        &mut self,
        event: InputEvent,
        context: &InteractionContext,
    ) -> Vec<CoreEffect> {
        self.state.core_interaction.handle_input(event, context)
    }

    pub fn apply_core_effects(
        &mut self,
        viewport: &impl AppViewport,
        effects: Vec<CoreEffect>,
    ) -> AppOutput {
        update::apply_core_effects(&mut self.state, viewport, effects)
    }

    pub fn navigate_to_search_match(
        &mut self,
        viewport: &impl AppViewport,
        search_match: &SearchMatch,
    ) -> AppOutput {
        update::navigate_to_search_match(&mut self.state, viewport, search_match)
    }

    pub fn navigate_to_definition(
        &mut self,
        viewport: &impl AppViewport,
        definition: &DefinitionLocation,
    ) -> AppOutput {
        update::navigate_to_definition(&mut self.state, viewport, definition)
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
    /// Vertical scroll offset in the file list (in display rows).
    pub file_list_scroll: usize,
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
    /// to the current rendered diff text. Recomputed when query or content
    /// changes.
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
            file_list_scroll: 0,
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

    /// Called after `selected_file` changes. Resets diff state and loads
    /// the appropriate file content from the working tree.
    pub fn on_file_changed(&mut self) {
        self.reviewed_diff_expanded = false;
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
    use crate::review_types::{ChangeKind, DiffContent, FileChange, FileEntry, ReviewStatus};

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

    #[test]
    fn app_owns_config_and_state() {
        let config = Config::default();
        let expected_width = config.layout.file_list_width;

        let app = App::new(config, test_context(), vec![test_file("src/lib.rs")]);

        assert_eq!(app.config.layout.file_list_width, expected_width);
        assert_eq!(app.state.files.len(), 1);
    }
}
