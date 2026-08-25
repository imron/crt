//! Application state and input dispatch.

pub mod document;
pub mod model;
mod update;

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;

use self::model::AppModel;
use crate::client::{Client, ClientEvent, CommentScope, Notification};
use crate::config::Config;
use crate::core::command::Command;
use crate::core::diff;
use crate::core::interaction::{CoreEffect, CoreInteractionEngine, InteractionContext};
use crate::core::review;
use crate::core::search as core_search;
use crate::core::{ConnectionState, InputEvent};
use crate::protocol::NotificationKind;
use crate::review_types::{
    CommentAnchor, CommentAnchorSegment, ConnectionContext, ContentMode, CreateCommentParams,
    DefinitionLocation, DiffContent, FileEntry, FileStatusEntry, PaneFocus, RenderVariant,
    ReviewActionResult, ReviewStatus, SearchMatch,
};
use anyhow::{Context, Result};

pub use update::{AppOutput, StatusUpdate, ViewportMetrics};

#[derive(Debug, Clone, PartialEq, Eq)]
enum AppWork {
    ToggleSelectedReview,
    UndoLastAction,
    /// Refresh review statuses / changed-file set without re-fetching full
    /// diffs for every file.
    RefreshReviewStatuses,
    RunCommand(Command),
    CreateComment {
        anchor: CommentAnchorCapture,
        body: String,
    },
    UpdateComment {
        id: i64,
        body: String,
    },
    ResolveComment {
        id: i64,
    },
    UnresolveComment {
        id: i64,
    },
    DeleteComment {
        id: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum UndoAction {
    UnresolveComment { id: i64 },
    UnapproveReview { file_path: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResetReviewSummary {
    pub merge_base_short: String,
    pub head_ref: String,
    pub cleared: u64,
}

pub enum ReviewStartup {
    Review(App),
    Reset(ResetReviewSummary),
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

#[derive(Debug, Clone)]
pub struct SearchResults {
    pub query: String,
    pub diff_only: bool,
    pub matches: Vec<SearchMatch>,
    pub selected: usize,
}

#[derive(Debug, Clone)]
pub struct DefinitionResults {
    pub symbol: String,
    pub definitions: Vec<DefinitionLocation>,
    pub selected: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualSelectionMode {
    Line,
    Text,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisualSelection {
    pub mode: VisualSelectionMode,
    pub start: document::DocumentPosition,
    pub end: document::DocumentPosition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentAnchorCapture {
    pub segments: Vec<CommentAnchorSegment>,
    pub file_path: String,
    pub line_start: i64,
    pub line_end: i64,
    pub char_start: Option<i64>,
    pub char_end: Option<i64>,
    pub anchor_text: String,
    pub context_before: String,
    pub context_after: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffWordSelection {
    pub word: String,
    pub start: document::DocumentPosition,
    pub end: document::DocumentPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileListSectionFocus {
    Unreviewed,
    Reviewed,
    UnresolvedComments,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedFilePosition {
    pub diff_scroll: usize,
    pub diff_line_cursor: usize,
    pub diff_col_cursor: usize,
    pub content_mode: ContentMode,
    pub render_variant: RenderVariant,
    /// File content identity at save time. Restore is skipped when this no
    /// longer matches the selected file's `content_id`.
    pub content_id: String,
}

/// Active document projection plus state expressed in that document's row space.
#[derive(Debug, Clone)]
pub struct ActiveDocumentState {
    document: document::ActiveDocument,
    scroll: document::RowIndex,
    cursor: document::DocumentPosition,
    visual_selection: Option<VisualSelection>,
}

impl ActiveDocumentState {
    pub fn document(&self) -> &document::ActiveDocument {
        &self.document
    }

    pub fn document_mut(&mut self) -> &mut document::ActiveDocument {
        &mut self.document
    }

    pub fn scroll(&self) -> document::RowIndex {
        self.scroll
    }

    pub fn cursor(&self) -> document::DocumentPosition {
        self.cursor
    }

    pub fn visual_selection(&self) -> Option<&VisualSelection> {
        self.visual_selection.as_ref()
    }
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
    pending_work: VecDeque<AppWork>,
    last_undo_action: Option<UndoAction>,
}

impl App {
    pub fn new(config: Config, context: ConnectionContext, files: Vec<FileEntry>) -> Self {
        let diff_algorithm =
            diff::resolve_diff_algorithm(&context.worktree, config.layout.diff_algorithm);
        let file_list_width = config.layout.file_list_width;
        let state = AppState::new(diff_algorithm, context, files, file_list_width);

        Self {
            state,
            config,
            client: None,
            config_path: None,
            pending_work: VecDeque::new(),
            last_undo_action: None,
        }
    }

    pub async fn start_review(
        base: Option<&str>,
        root: bool,
        reset: bool,
        standalone: bool,
    ) -> Result<ReviewStartup> {
        let socket_path = default_socket_path()?;
        let cwd = std::env::current_dir().context("Failed to determine current directory")?;
        let config = crate::config::load();
        let diff_algorithm =
            diff::resolve_diff_algorithm(&cwd.to_string_lossy(), config.layout.diff_algorithm);
        let client = Client::connect_or_start(&socket_path, standalone).await?;
        let init = if root {
            client
                .init_with_options(&cwd.to_string_lossy(), "", true, Some(diff_algorithm))
                .await?
        } else {
            let base = base.context("A base ref is required unless --root is specified")?;
            client
                .init_with_options(&cwd.to_string_lossy(), base, false, Some(diff_algorithm))
                .await?
        };

        if reset {
            let result = client.reset_reviews().await?;
            let summary = ResetReviewSummary {
                merge_base_short: crate::git::short_hash(&init.merge_base),
                head_ref: init.head_ref,
                cleared: result.cleared,
            };
            client.shutdown().await;
            return Ok(ReviewStartup::Reset(summary));
        }

        Ok(ReviewStartup::Review(
            Self::load_with_config(client, init, config).await?,
        ))
    }

    pub async fn load(client: Client, context: ConnectionContext) -> Result<Self> {
        let config = crate::config::load();
        Self::load_with_config(client, context, config).await
    }

    async fn load_with_config(
        client: Client,
        context: ConnectionContext,
        config: Config,
    ) -> Result<Self> {
        // Prefer compact statuses for large reviews; full hunks load lazily for
        // the selected file via on_file_changed.
        let files = match client.list_file_statuses().await {
            Ok(result) => result
                .files
                .into_iter()
                .map(file_entry_from_status)
                .collect(),
            Err(e) => {
                eprintln!("Warning: could not load files: {e}");
                Vec::new()
            }
        };
        let comments = match client
            .list_current_and_previous_unresolved_comments(None, CommentScope::CurrentWithResolved)
            .await
        {
            Ok(result) => result.comments,
            Err(e) => {
                eprintln!("Warning: could not load comments: {e}");
                Vec::new()
            }
        };
        let config_path = crate::config::config_path();
        let mut app = Self::new(config, context, files);
        app.state.comments = comments;
        app.client = Some(client);
        app.config_path = config_path;
        app.state.on_file_changed();
        Ok(app)
    }

    pub async fn shutdown(mut self) {
        if let Some(client) = self.client.take() {
            client.shutdown().await;
        }
    }

    fn config_path(&self) -> Option<&PathBuf> {
        self.config_path.as_ref()
    }

    pub fn set_file_list_width(&mut self, width: u16) {
        if self.state.file_list_width == width && self.config.layout.file_list_width == width {
            return;
        }
        self.config.layout.file_list_width = width;
        self.state.set_file_list_width(width);
        self.save_layout_config();
    }

    fn save_layout_config(&self) {
        if let Some(path) = self.config_path() {
            let layout = crate::config::LayoutConfig {
                file_list_width: self.config.layout.file_list_width,
                diff_algorithm: Some(self.state.diff_algorithm),
            };
            crate::config::save_layout(path, &layout);
        }
    }

    fn client(&self) -> Result<&Client> {
        self.client
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("app client is not available for this workflow"))
    }

    pub async fn list_changed_files(&self) -> Result<crate::review_types::ListChangedFilesResult> {
        self.client()?.list_changed_files().await
    }

    pub async fn list_file_statuses(&self) -> Result<crate::review_types::ListFileStatusesResult> {
        self.client()?.list_file_statuses().await
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

    pub async fn list_comments(&self) -> Result<crate::review_types::ListCommentsResult> {
        self.client()?
            .list_current_and_previous_unresolved_comments(None, CommentScope::CurrentWithResolved)
            .await
    }

    async fn drain_notifications(&self) -> Result<Vec<Notification>> {
        Ok(self.client()?.drain_notifications().await)
    }

    async fn drain_client_events(&self) -> Result<Vec<ClientEvent>> {
        Ok(self.client()?.drain_events().await)
    }

    async fn recover_client_if_disconnected(&self) -> Result<Option<ConnectionState>> {
        self.client()?.recover_if_disconnected().await
    }

    pub async fn process_background_work(&mut self) -> Option<StatusUpdate> {
        let mut status = self.process_pending_work().await;

        // Consume connection events emitted by foreground requests handled in
        // the previous tick, such as a failed review/search call entering
        // Reconnecting state.
        if let Some(connection_status) = self.process_connection_events().await {
            status = Some(connection_status);
        }

        if let Some(reconnect_status) = self.process_reconnect().await {
            status = Some(reconnect_status);
        }

        if let Some(notification_status) = self.process_notifications().await {
            status = Some(notification_status);
        }

        // Reconnect may emit Reconnected during this tick. Process it before
        // returning so the snapshot resync happens before steady-state
        // notification handling continues on the next tick.
        if let Some(connection_status) = self.process_connection_events().await {
            status = Some(connection_status);
        }

        if let Some(work_status) = self.process_pending_work().await {
            status = Some(work_status);
        }

        status
    }

    async fn process_connection_events(&mut self) -> Option<StatusUpdate> {
        let events = match self.drain_client_events().await {
            Ok(events) => events,
            Err(e) => return Some(StatusUpdate::Set(format!("Connection event error: {e}"))),
        };

        let mut status = None;
        for event in events {
            match event {
                ClientEvent::ConnectionState(ConnectionState::Reconnecting) => {
                    self.state.clear_transient_interaction_state();
                    status = Some(StatusUpdate::ConnectionState(ConnectionState::Reconnecting));
                }
                ClientEvent::ConnectionState(ConnectionState::Reconnected) => {
                    self.state.clear_transient_interaction_state();
                    if let Some(reload_status) = self.reload_file_snapshot().await {
                        status = Some(reload_status);
                    } else {
                        status = Some(StatusUpdate::ConnectionState(ConnectionState::Reconnected));
                    }
                }
                ClientEvent::ConnectionState(ConnectionState::Connected) => {}
                ClientEvent::ConnectionState(ConnectionState::Disconnected) => {
                    self.state.clear_transient_interaction_state();
                    status = Some(StatusUpdate::ConnectionState(ConnectionState::Disconnected));
                }
            }
        }

        status
    }

    async fn process_reconnect(&mut self) -> Option<StatusUpdate> {
        match self.recover_client_if_disconnected().await {
            Ok(Some(ConnectionState::Reconnected)) => None,
            Ok(Some(ConnectionState::Reconnecting)) => {
                Some(StatusUpdate::ConnectionState(ConnectionState::Reconnecting))
            }
            Ok(Some(ConnectionState::Disconnected)) => {
                Some(StatusUpdate::ConnectionState(ConnectionState::Disconnected))
            }
            Ok(Some(ConnectionState::Connected)) | Ok(None) => None,
            Err(e) => Some(StatusUpdate::Set(format!("Reconnect error: {e}"))),
        }
    }

    async fn process_pending_work(&mut self) -> Option<StatusUpdate> {
        let mut status = None;
        while let Some(work) = self.pending_work.pop_front() {
            let work_status = match work {
                AppWork::ToggleSelectedReview => self.toggle_selected_review().await,
                AppWork::UndoLastAction => self.undo_last_action().await,
                AppWork::RefreshReviewStatuses => self.refresh_review_statuses().await,
                AppWork::RunCommand(command) => self.run_command(command).await,
                AppWork::CreateComment { anchor, body } => {
                    self.create_comment_from_anchor(anchor, body).await
                }
                AppWork::UpdateComment { id, body } => self.update_comment(id, body).await,
                AppWork::ResolveComment { id } => self.resolve_comment(id).await,
                AppWork::UnresolveComment { id } => self.unresolve_comment(id).await,
                AppWork::DeleteComment { id } => self.delete_comment(id).await,
            };
            if work_status.is_some() {
                status = work_status;
            }
        }
        status
    }

    async fn process_notifications(&mut self) -> Option<StatusUpdate> {
        let notifications = match self.drain_notifications().await {
            Ok(notifications) => notifications,
            Err(e) => return Some(StatusUpdate::Set(format!("Notification error: {e}"))),
        };

        if notification_requires_snapshot_reload(&notifications) {
            return self.reload_file_snapshot().await;
        }
        if notification_has_review_status_patch(&notifications) {
            self.apply_review_change_notifications(&notifications);
            // If any review notification lacked a status payload (older
            // servers), fall back to a compact status refresh.
            if notification_requires_status_refresh(&notifications) {
                return self.refresh_review_statuses().await;
            }
            return None;
        }
        if notification_requires_comment_reload(&notifications) {
            return self.reload_comments().await;
        }

        None
    }

    pub fn model(&self) -> AppModel<'_> {
        AppModel::from_state(&self.state)
    }

    pub fn interaction_context(&self) -> InteractionContext {
        update::interaction_context(&self.state)
    }

    pub fn prompt_submit_context(&self, viewport: &impl ViewportMetrics) -> InteractionContext {
        update::prompt_submit_context(&self.state, viewport)
    }

    pub fn handle_input(
        &mut self,
        event: InputEvent,
        context: &InteractionContext,
    ) -> Vec<CoreEffect> {
        if matches!(event, InputEvent::FocusGained) {
            // Cheap status refresh: detect external review/diff changes without
            // re-diffing every file.
            self.pending_work.push_back(AppWork::RefreshReviewStatuses);
            return Vec::new();
        }
        self.state.core_interaction.handle_input(event, context)
    }

    pub fn apply_core_effects(
        &mut self,
        viewport: &impl ViewportMetrics,
        effects: Vec<CoreEffect>,
    ) -> AppOutput {
        self.state.ensure_active_document();
        let mut output = update::apply_core_effects(&mut self.state, viewport, effects);
        if output.take_pending_review_toggle() {
            self.pending_work.push_back(AppWork::ToggleSelectedReview);
        }
        if output.take_pending_undo() {
            self.pending_work.push_back(AppWork::UndoLastAction);
        }
        if let Some(command) = output.take_pending_command() {
            self.pending_work.push_back(AppWork::RunCommand(command));
        }
        if let Some((anchor, body)) = output.take_pending_comment_create() {
            self.pending_work
                .push_back(AppWork::CreateComment { anchor, body });
        }
        if let Some((id, body)) = output.take_pending_comment_update() {
            self.pending_work
                .push_back(AppWork::UpdateComment { id, body });
        }
        if let Some(id) = output.take_pending_comment_resolve() {
            self.pending_work.push_back(AppWork::ResolveComment { id });
        }
        if let Some(id) = output.take_pending_comment_unresolve() {
            self.pending_work
                .push_back(AppWork::UnresolveComment { id });
        }
        if let Some(id) = output.take_pending_comment_delete() {
            self.pending_work.push_back(AppWork::DeleteComment { id });
        }
        if output.save_layout {
            self.save_layout_config();
        }
        output
    }

    pub fn navigate_to_search_match(
        &mut self,
        viewport: &impl ViewportMetrics,
        search_match: &SearchMatch,
    ) -> AppOutput {
        update::navigate_to_search_match(&mut self.state, viewport, search_match)
    }

    pub fn navigate_to_definition(
        &mut self,
        viewport: &impl ViewportMetrics,
        definition: &DefinitionLocation,
    ) -> AppOutput {
        update::navigate_to_definition(&mut self.state, viewport, definition)
    }

    pub fn refresh_active_diff_search(&mut self, viewport: &impl ViewportMetrics) -> bool {
        update::diff_search::refresh_active_diff_search(&mut self.state, viewport)
    }

    async fn toggle_selected_review(&mut self) -> Option<StatusUpdate> {
        let entry = self.state.files.get(self.state.selected_file)?;
        let path = entry.change.path.clone();
        let is_reviewed = matches!(entry.status, ReviewStatus::Reviewed { .. });

        let result = if is_reviewed {
            self.unmark_reviewed(&path).await
        } else {
            self.mark_reviewed(&path).await
        };

        match result {
            Ok(action_result) => {
                if !is_reviewed {
                    self.last_undo_action = Some(UndoAction::UnapproveReview {
                        file_path: path.clone(),
                    });
                }
                self.apply_review_result(&action_result);
                None
            }
            Err(e) => {
                let verb = if is_reviewed { "unmark" } else { "mark" };
                Some(StatusUpdate::Set(format!("Failed to {verb} reviewed: {e}")))
            }
        }
    }

    async fn undo_last_action(&mut self) -> Option<StatusUpdate> {
        let Some(action) = self.last_undo_action.take() else {
            return Some(StatusUpdate::Set("Nothing to undo".to_string()));
        };

        match action {
            UndoAction::UnresolveComment { id } => {
                let status = self.unresolve_comment(id).await;
                status.or_else(|| Some(StatusUpdate::Set(format!("Undid resolve #{id}"))))
            }
            UndoAction::UnapproveReview { file_path } => {
                match self.unmark_reviewed(&file_path).await {
                    Ok(action_result) => {
                        self.apply_review_result(&action_result);
                        Some(StatusUpdate::Set(format!("Undid approval for {file_path}")))
                    }
                    Err(e) => Some(StatusUpdate::Set(format!("Failed to undo approval: {e}"))),
                }
            }
        }
    }

    async fn reload_file_snapshot(&mut self) -> Option<StatusUpdate> {
        match self.list_file_statuses().await {
            Ok(result) => {
                let files = result
                    .files
                    .into_iter()
                    .map(file_entry_from_status)
                    .collect();
                self.replace_file_snapshot(files);
                if let Some(status) = self.reload_comments().await {
                    return Some(status);
                }
                None
            }
            Err(e) => Some(StatusUpdate::Set(format!("Failed to reload files: {e}"))),
        }
    }

    async fn refresh_review_statuses(&mut self) -> Option<StatusUpdate> {
        match self.list_file_statuses().await {
            Ok(result) => {
                let files = result
                    .files
                    .into_iter()
                    .map(file_entry_from_status)
                    .collect();
                self.merge_file_statuses(files);
                None
            }
            Err(e) => Some(StatusUpdate::Set(format!(
                "Failed to refresh file statuses: {e}"
            ))),
        }
    }

    async fn reload_comments(&mut self) -> Option<StatusUpdate> {
        match self.list_comments().await {
            Ok(result) => {
                self.state.comments = result.comments;
                self.state.selected_comment_id = self
                    .state
                    .selected_comment_id
                    .filter(|id| self.state.comments.iter().any(|comment| comment.id == *id));
                self.state.mark_model_changed();
                None
            }
            Err(e) => Some(StatusUpdate::Set(format!("Failed to reload comments: {e}"))),
        }
    }

    fn replace_file_snapshot(&mut self, files: Vec<FileEntry>) {
        let selected_path = review::selected_path(&self.state.files, self.state.selected_file);
        let saved_cursor = self.state.diff_line_cursor;
        let saved_col = self.state.diff_col_cursor;
        let saved_scroll = self.state.diff_scroll;
        let saved_content_mode = self.state.content_mode;
        let saved_render_variant = self.state.render_variant;

        let files = preserve_loaded_diffs(&self.state.files, files);
        self.state.files = files;
        review::sort_files(&mut self.state.files);
        self.state.selected_file =
            review::restore_selection_by_path(&self.state.files, selected_path.as_deref());
        self.state.file_list_section_focus = self.state.focus_for_file(self.state.selected_file);

        let restored_same_file = selected_path.as_deref().is_some_and(|path| {
            self.state
                .files
                .get(self.state.selected_file)
                .is_some_and(|entry| entry.change.path == path)
        });

        // Load full hunks only when the selected entry still has an empty
        // diff body (or hash changed and cache was dropped).
        self.state.refresh_current_file_diff();
        self.state.load_head_content();
        self.state.load_blame();

        if restored_same_file {
            self.state.content_mode = saved_content_mode;
            self.state.render_variant = saved_render_variant;
            self.state.diff_line_cursor = saved_cursor;
            self.state.diff_col_cursor = saved_col;
            self.state.diff_scroll = saved_scroll;
            self.state.rebuild_active_document();
        } else {
            self.state.on_file_changed();
        }
        self.state.mark_model_changed();
    }

    /// Merge a compact status snapshot into the current file list, preserving
    /// already-loaded full diffs when the hash is unchanged.
    fn merge_file_statuses(&mut self, files: Vec<FileEntry>) {
        let selected_path = review::selected_path(&self.state.files, self.state.selected_file);

        let files = preserve_loaded_diffs(&self.state.files, files);
        self.state.files = files;
        review::sort_files(&mut self.state.files);
        self.state.selected_file =
            review::restore_selection_by_path(&self.state.files, selected_path.as_deref());
        self.state.file_list_section_focus = self.state.focus_for_file(self.state.selected_file);

        let still_same_file = selected_path.as_deref().is_some_and(|path| {
            self.state
                .files
                .get(self.state.selected_file)
                .is_some_and(|entry| entry.change.path == path)
        });
        let selected_needs_diff = self
            .state
            .selected_file_entry()
            .is_some_and(|entry| entry.diff.hunks.is_empty());

        if still_same_file {
            // Stay put if the open file mutated under us; reload bodies only.
            if selected_needs_diff {
                self.state.refresh_selected_file_content();
            } else {
                self.state.mark_model_changed();
            }
        } else {
            self.state.on_file_changed();
        }
    }

    fn apply_review_change_notifications(
        &mut self,
        notifications: &[crate::protocol::Notification],
    ) {
        let selected_path = review::selected_path(&self.state.files, self.state.selected_file);
        let mut patched = false;
        for notification in notifications {
            let NotificationKind::ReviewChanged {
                file_path,
                status: Some(status),
            } = &notification.kind
            else {
                continue;
            };
            if let Some(entry) = self
                .state
                .files
                .iter_mut()
                .find(|entry| entry.change.path == *file_path)
            {
                entry.status = status.clone();
                patched = true;
            }
        }
        if !patched {
            return;
        }
        review::sort_files(&mut self.state.files);
        self.state.selected_file =
            review::restore_selection_by_path(&self.state.files, selected_path.as_deref());
        self.state.file_list_section_focus = self.state.focus_for_file(self.state.selected_file);
        self.state.mark_model_changed();
    }

    fn apply_review_result(&mut self, result: &ReviewActionResult) {
        self.state.save_selected_file_position();
        self.state.selected_file =
            review::apply_review_result(&mut self.state.files, self.state.selected_file, result);
        self.state.file_list_section_focus = self.state.focus_for_file(self.state.selected_file);
        self.state.on_file_changed();
        self.state.restore_selected_file_position();
    }

    async fn run_command(&mut self, command: Command) -> Option<StatusUpdate> {
        match command {
            Command::SearchAll { pattern } => self.run_search(pattern, false).await,
            Command::SearchDiff { pattern } => self.run_search(pattern, true).await,
            Command::FindDefinition { symbol } => self.run_definition_lookup(symbol).await,
            Command::SetMergeBase(Some(refspec)) => self.set_merge_base(refspec).await,
            Command::ViewFile { path, line_number } => Some(StatusUpdate::Set(format!(
                "File not in diff: {path} {line_number}"
            ))),
            Command::Quit
            | Command::SetBlame(_)
            | Command::SetComments(_)
            | Command::SetWhitespaceIgnored(_)
            | Command::SetMergeBase(None)
            | Command::Unknown { .. } => {
                Some(StatusUpdate::Set("Unsupported pending command".to_string()))
            }
        }
    }

    async fn set_merge_base(&mut self, refspec: String) -> Option<StatusUpdate> {
        let client = match self.client() {
            Ok(client) => client,
            Err(e) => return Some(StatusUpdate::Set(format!("Failed to set merge base: {e}"))),
        };

        let context = match client.set_merge_base(&refspec).await {
            Ok(result) => result.context,
            Err(e) => return Some(StatusUpdate::Set(format!("Failed to set merge base: {e}"))),
        };

        self.state.context = context;
        self.state.clear_transient_interaction_state();
        match self.reload_file_snapshot().await {
            Some(status) => Some(status),
            None => Some(StatusUpdate::Set(format!(
                "Merge base set to {}",
                self.state.context.merge_base
            ))),
        }
    }

    async fn create_comment_from_anchor(
        &mut self,
        anchor: CommentAnchorCapture,
        body: String,
    ) -> Option<StatusUpdate> {
        let params = match create_comment_params_from_capture(anchor, body) {
            Ok(params) => params,
            Err(e) => return Some(StatusUpdate::Set(format!("Failed to create comment: {e}"))),
        };

        let client = match self.client() {
            Ok(client) => client,
            Err(e) => return Some(StatusUpdate::Set(format!("Failed to create comment: {e}"))),
        };

        match client.create_comment(params).await {
            Ok(result) => {
                self.state.pending_comment_anchor = None;
                self.state.visual_selection = None;
                upsert_comment(&mut self.state.comments, result.comment.clone());
                self.state.mark_model_changed();
                Some(StatusUpdate::Set(format!(
                    "Created comment #{}",
                    result.comment.id
                )))
            }
            Err(e) => Some(StatusUpdate::Set(format!("Failed to create comment: {e}"))),
        }
    }

    async fn update_comment(&mut self, id: i64, body: String) -> Option<StatusUpdate> {
        let body = body.trim_end().to_string();
        if body.is_empty() {
            return Some(StatusUpdate::Set("Empty comment ignored".to_string()));
        }
        let client = match self.client() {
            Ok(client) => client,
            Err(e) => return Some(StatusUpdate::Set(format!("Failed to update comment: {e}"))),
        };

        match client.update_comment(id, &body).await {
            Ok(result) => {
                upsert_comment(&mut self.state.comments, result.comment.clone());
                self.state.mark_model_changed();
                Some(StatusUpdate::Set(format!(
                    "Updated comment #{}",
                    result.comment.id
                )))
            }
            Err(e) => Some(StatusUpdate::Set(format!("Failed to update comment: {e}"))),
        }
    }

    async fn resolve_comment(&mut self, id: i64) -> Option<StatusUpdate> {
        let client = match self.client() {
            Ok(client) => client,
            Err(e) => return Some(StatusUpdate::Set(format!("Failed to resolve comment: {e}"))),
        };

        match client.resolve_comment(id).await {
            Ok(result) => {
                upsert_comment(&mut self.state.comments, result.comment.clone());
                self.last_undo_action = Some(UndoAction::UnresolveComment {
                    id: result.comment.id,
                });
                self.state.mark_model_changed();
                Some(StatusUpdate::Set(format!(
                    "Resolved comment #{}",
                    result.comment.id
                )))
            }
            Err(e) => Some(StatusUpdate::Set(format!("Failed to resolve comment: {e}"))),
        }
    }

    async fn unresolve_comment(&mut self, id: i64) -> Option<StatusUpdate> {
        let client = match self.client() {
            Ok(client) => client,
            Err(e) => {
                return Some(StatusUpdate::Set(format!(
                    "Failed to unresolve comment: {e}"
                )));
            }
        };

        match client.unresolve_comment(id).await {
            Ok(result) => {
                upsert_comment(&mut self.state.comments, result.comment.clone());
                self.state.mark_model_changed();
                Some(StatusUpdate::Set(format!(
                    "Unresolved comment #{}",
                    result.comment.id
                )))
            }
            Err(e) => Some(StatusUpdate::Set(format!(
                "Failed to unresolve comment: {e}"
            ))),
        }
    }

    async fn delete_comment(&mut self, id: i64) -> Option<StatusUpdate> {
        let client = match self.client() {
            Ok(client) => client,
            Err(e) => return Some(StatusUpdate::Set(format!("Failed to delete comment: {e}"))),
        };

        match client.delete_comment(id).await {
            Ok(result) => {
                if result.deleted {
                    self.state.comments.retain(|comment| comment.id != id);
                    self.state.expanded_comment_ids.remove(&id);
                    if self.state.selected_comment_id == Some(id) {
                        self.state.selected_comment_id = None;
                    }
                    self.state.pending_delete_comment_id = None;
                    self.state.mark_model_changed();
                    Some(StatusUpdate::Set(format!("Deleted comment #{id}")))
                } else {
                    Some(StatusUpdate::Set(format!("Comment #{id} was not deleted")))
                }
            }
            Err(e) => Some(StatusUpdate::Set(format!("Failed to delete comment: {e}"))),
        }
    }

    async fn run_search(&mut self, pattern: String, diff_only: bool) -> Option<StatusUpdate> {
        let scope = if diff_only { "diff" } else { "all" };
        match self.search_codebase(&pattern, scope).await {
            Ok(result) => match core_search::search_outcome(&pattern, diff_only, result) {
                core_search::SearchOutcome::NoMatches => {
                    let suffix = if diff_only { " in diff" } else { "" };
                    Some(StatusUpdate::Set(format!(
                        "No matches for /{pattern}/{suffix}"
                    )))
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
                    });
                    self.state.mark_model_changed();
                    Some(StatusUpdate::Clear)
                }
            },
            Err(e) => Some(StatusUpdate::Set(format!("Search error: {e}"))),
        }
    }

    async fn run_definition_lookup(&mut self, symbol: String) -> Option<StatusUpdate> {
        let context_file = self
            .state
            .selected_file_entry()
            .map(|entry| entry.change.path.clone());
        match self.find_definition(&symbol, context_file.as_deref()).await {
            Ok(result) => {
                match core_search::definition_outcome(&symbol, result, &self.state.files) {
                    core_search::DefinitionOutcome::NoDefinitions => Some(StatusUpdate::Set(
                        format!("No definitions found for '{symbol}'"),
                    )),
                    core_search::DefinitionOutcome::Navigate(target) => {
                        self.navigate_to_location_target(target)
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
                        self.state.mark_model_changed();
                        Some(StatusUpdate::Clear)
                    }
                }
            }
            Err(e) => Some(StatusUpdate::Set(format!("Definition error: {e}"))),
        }
    }

    fn navigate_to_location_target(
        &mut self,
        target: core_search::LocationTarget,
    ) -> Option<StatusUpdate> {
        self.state.jump_stack.push(JumpLocation {
            file_index: self.state.selected_file,
            diff_scroll: self.state.diff_scroll,
            diff_line_cursor: self.state.diff_line_cursor,
            content_mode: self.state.content_mode,
            render_variant: self.state.render_variant,
        });

        match target {
            core_search::LocationTarget::InDiff {
                file_index,
                line_number,
            } => {
                let focus = self.state.focus_for_file(file_index);
                self.state.select_file(file_index, focus, false);
                self.state.diff_line_cursor = (line_number as usize).saturating_sub(1);
                Some(StatusUpdate::Clear)
            }
            core_search::LocationTarget::External {
                file_path,
                line_number,
            } => Some(StatusUpdate::Set(format!(
                "Definition in file not in diff: {file_path}:{line_number}"
            ))),
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.client.take();
    }
}

/// Get the default socket path (~/.crt/server.sock).
pub fn default_socket_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    let crt_dir = PathBuf::from(home).join(".crt");
    if !crt_dir.exists() {
        std::fs::create_dir_all(&crt_dir)
            .with_context(|| format!("Failed to create {}", crt_dir.display()))?;
    }
    Ok(crt_dir.join("server.sock"))
}

fn notification_requires_snapshot_reload(notifications: &[Notification]) -> bool {
    notifications.iter().any(|notification| {
        matches!(
            notification.kind,
            NotificationKind::ReviewsCleared | NotificationKind::ReviewsMigrated { .. }
        )
    })
}

fn notification_has_review_status_patch(notifications: &[Notification]) -> bool {
    notifications
        .iter()
        .any(|notification| matches!(notification.kind, NotificationKind::ReviewChanged { .. }))
}

fn notification_requires_status_refresh(notifications: &[Notification]) -> bool {
    notifications.iter().any(|notification| {
        matches!(
            notification.kind,
            NotificationKind::ReviewChanged { status: None, .. }
        )
    })
}

fn notification_requires_comment_reload(notifications: &[Notification]) -> bool {
    notifications
        .iter()
        .any(|notification| matches!(notification.kind, NotificationKind::CommentChanged { .. }))
}

fn file_entry_from_status(entry: FileStatusEntry) -> FileEntry {
    FileEntry {
        change: entry.change,
        status: entry.status,
        diff: DiffContent {
            hunks: Vec::new(),
            is_binary: entry.diff.is_binary,
            diff_hash: entry.diff.diff_hash,
            content_id: entry.diff.content_id,
        },
    }
}

/// Keep previously loaded full hunk bodies when file content identity is
/// unchanged. `content_id` is algorithm-independent; display algorithm is
/// tracked separately on the document key.
fn preserve_loaded_diffs(previous: &[FileEntry], mut next: Vec<FileEntry>) -> Vec<FileEntry> {
    let previous_by_path: HashMap<&str, &FileEntry> = previous
        .iter()
        .map(|entry| (entry.change.path.as_str(), entry))
        .collect();
    for entry in &mut next {
        if !entry.diff.hunks.is_empty() {
            continue;
        }
        if let Some(prev) = previous_by_path.get(entry.change.path.as_str()) {
            let same_content =
                !prev.diff.content_id.is_empty() && prev.diff.content_id == entry.diff.content_id;
            if !prev.diff.hunks.is_empty() && same_content {
                // Preserve hunk bodies; keep the status snapshot's content_id
                // and leave display patch hash as previously loaded.
                let mut preserved = prev.diff.clone();
                preserved.content_id = entry.diff.content_id.clone();
                entry.diff = preserved;
            }
        }
    }
    next
}

fn upsert_comment(
    comments: &mut Vec<crate::review_types::Comment>,
    comment: crate::review_types::Comment,
) {
    if let Some(existing) = comments
        .iter_mut()
        .find(|existing| existing.id == comment.id)
    {
        *existing = comment;
    } else {
        comments.push(comment);
    }
}

fn create_comment_params_from_capture(
    anchor: CommentAnchorCapture,
    body: String,
) -> Result<CreateCommentParams> {
    let anchor = CommentAnchor::try_new(anchor.segments)?;
    Ok(CreateCommentParams { anchor, body })
}

/// Central application state for review data and domain interaction.
/// Input events mutate it, sometimes by sending requests to the server.
pub struct AppState {
    /// Monotonic revision of the UI-renderable app model.
    model_revision: u64,
    /// Resolved context from the server (repo root, worktree, refs).
    pub context: ConnectionContext,
    /// All files changed in base..HEAD, with their review status and diffs.
    pub files: Vec<FileEntry>,
    /// Review comments in the current review scope.
    pub comments: Vec<crate::review_types::Comment>,
    /// Whether the comments panel is visible under the diff pane.
    pub show_comments_panel: bool,
    /// Comment selected in the comments panel.
    pub selected_comment_id: Option<i64>,
    /// Resolved/collapsed comments explicitly expanded by the user.
    pub expanded_comment_ids: BTreeSet<i64>,
    /// Comment awaiting delete confirmation.
    pub pending_delete_comment_id: Option<i64>,
    /// Index of the currently selected file in `files`.
    pub selected_file: usize,
    /// Which pane has keyboard focus.
    pub pane_focus: PaneFocus,
    /// Whether the file list pane is conceptually visible.
    pub show_file_list: bool,
    /// Whether the diff pane is conceptually visible.
    pub show_diff_pane: bool,
    /// App-owned file list pane width from layout config.
    pub file_list_width: u16,
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
    /// File-list section currently targeted by section navigation.
    pub file_list_section_focus: FileListSectionFocus,
    /// Last visited diff location for each file path.
    pub saved_file_positions: HashMap<String, SavedFilePosition>,
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
    pub head_blame: Vec<model::BlameLine>,
    /// Blame data for the base version of the current file.
    pub base_blame: Vec<model::BlameLine>,
    /// Cached active document projection and document-local state.
    pub active_document: Option<ActiveDocumentState>,
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
    /// Active visual selection in the diff pane.
    pub visual_selection: Option<VisualSelection>,
    /// Most recently committed comment anchor capture.
    pub pending_comment_anchor: Option<CommentAnchorCapture>,
    /// When true, force diffs to use merge_base even for reviewed files.
    /// Toggled by the `m` keybinding.
    pub show_merge_base: bool,
    /// Active codebase search results overlay state.
    pub search_results: Option<SearchResults>,
    /// Active definition lookup results overlay state.
    pub definition_results: Option<DefinitionResults>,
    /// Core interaction entrypoint used by UI adapters.
    pub core_interaction: CoreInteractionEngine,
}

impl AppState {
    pub fn new(
        diff_algorithm: crate::config::DiffAlgorithm,
        context: ConnectionContext,
        mut files: Vec<FileEntry>,
        file_list_width: u16,
    ) -> Self {
        review::sort_files(&mut files);

        Self {
            model_revision: 0,
            context,
            files,
            comments: Vec::new(),
            show_comments_panel: false,
            selected_comment_id: None,
            expanded_comment_ids: BTreeSet::new(),
            pending_delete_comment_id: None,
            // Index 0 is the first unreviewed file (due to sort order).
            selected_file: 0,
            pane_focus: PaneFocus::FileList,
            show_file_list: true,
            show_diff_pane: true,
            file_list_width,
            content_mode: ContentMode::Diff,
            render_variant: RenderVariant::Inline,
            diff_scroll: 0,
            diff_line_cursor: 0,
            diff_col_cursor: 0,
            reviewed_diff_expanded: false,
            head_content: None,
            base_content: None,
            file_list_scroll: 0,
            file_list_section_focus: FileListSectionFocus::Unreviewed,
            saved_file_positions: HashMap::new(),
            diff_algorithm,
            default_diff_algorithm: diff_algorithm,
            ignore_whitespace: false,
            show_blame: false,
            head_blame: Vec::new(),
            base_blame: Vec::new(),
            active_document: None,
            jump_stack: Vec::new(),
            diff_search_query: None,
            diff_search_matches: Vec::new(),
            diff_search_current: 0,
            visual_selection: None,
            pending_comment_anchor: None,
            show_merge_base: false,
            search_results: None,
            definition_results: None,
            core_interaction: CoreInteractionEngine::new(),
        }
    }

    pub fn model_revision(&self) -> u64 {
        self.model_revision
    }

    pub fn mark_model_changed(&mut self) {
        self.refresh_current_active_document_overlays();
        self.model_revision = self.model_revision.wrapping_add(1);
    }

    fn refresh_current_active_document_overlays(&mut self) {
        let Some(key) = self.current_document_key() else {
            self.active_document = None;
            return;
        };
        let is_current = self
            .active_document
            .as_ref()
            .is_some_and(|active| active.document().key == key);
        if is_current {
            self.refresh_document_overlays();
        }
    }

    pub fn current_document_key(&self) -> Option<document::DocumentKey> {
        model::active_document_key(self)
    }

    pub fn rebuild_active_document(&mut self) {
        self.active_document = self.current_document_key().and_then(|key| {
            model::build_active_document(self, key)
                .map(|document| self.active_document_state(document))
        });
    }

    pub fn ensure_active_document(&mut self) {
        let Some(key) = self.current_document_key() else {
            self.active_document = None;
            return;
        };
        let needs_rebuild = self
            .active_document
            .as_ref()
            .is_none_or(|active| active.document().key != key);
        if needs_rebuild {
            self.active_document = model::build_active_document(self, key)
                .map(|document| self.active_document_state(document));
        } else {
            self.refresh_document_overlays();
        }
    }

    fn active_document_state(&self, document: document::ActiveDocument) -> ActiveDocumentState {
        ActiveDocumentState {
            document,
            scroll: document::RowIndex(self.diff_scroll),
            cursor: document::DocumentPosition {
                row: document::RowIndex(self.diff_line_cursor),
                column: document::ColumnIndex(self.diff_col_cursor),
            },
            visual_selection: self.visual_selection.clone(),
        }
    }

    fn refresh_document_overlays(&mut self) {
        let Some(file_path) = self
            .selected_file_entry()
            .map(|entry| entry.change.path.clone())
        else {
            self.active_document = None;
            return;
        };
        let Some(active) = self.active_document.as_mut() else {
            return;
        };
        active.scroll = document::RowIndex(self.diff_scroll);
        active.cursor = document::DocumentPosition {
            row: document::RowIndex(self.diff_line_cursor),
            column: document::ColumnIndex(self.diff_col_cursor),
        };
        active.visual_selection = self.visual_selection.clone();
        active.document.refresh_overlays(
            &file_path,
            &self.comments,
            self.selected_comment_id,
            Some(document::RowIndex(self.diff_line_cursor)),
            model::document_visible_selection(self.visual_selection.as_ref()),
            Some(document::DocumentPosition {
                row: document::RowIndex(self.diff_line_cursor),
                column: document::ColumnIndex(self.diff_col_cursor),
            }),
            self.diff_search_query.as_deref(),
        );
    }

    pub fn set_file_list_width(&mut self, width: u16) {
        if self.file_list_width != width {
            self.file_list_width = width;
            self.mark_model_changed();
        }
    }

    pub fn clamp_diff_viewport(
        &mut self,
        rendered_line_count: usize,
        visible_line_count: usize,
    ) -> bool {
        let before = (self.diff_line_cursor, self.diff_scroll);
        let max = rendered_line_count.saturating_sub(1);
        self.diff_line_cursor = self.diff_line_cursor.min(max);
        if self.diff_line_cursor < self.diff_scroll {
            self.diff_scroll = self.diff_line_cursor;
        }
        if visible_line_count > 0
            && self.diff_line_cursor >= self.diff_scroll.saturating_add(visible_line_count)
        {
            self.diff_scroll = self
                .diff_line_cursor
                .saturating_sub(visible_line_count.saturating_sub(1));
        }
        self.diff_scroll = self.diff_scroll.min(max);
        before != (self.diff_line_cursor, self.diff_scroll)
    }

    pub fn clamp_file_list_viewport(
        &mut self,
        selected_row: Option<usize>,
        visible_rows: usize,
    ) -> bool {
        let Some(selected_row) = selected_row else {
            return false;
        };
        if visible_rows == 0 {
            return false;
        }

        let before = self.file_list_scroll;
        if selected_row < self.file_list_scroll {
            self.file_list_scroll = selected_row;
        } else if selected_row >= self.file_list_scroll.saturating_add(visible_rows) {
            self.file_list_scroll = selected_row.saturating_sub(visible_rows).saturating_add(1);
        }
        before != self.file_list_scroll
    }

    /// The currently selected file, if any.
    pub fn selected_file_entry(&self) -> Option<&FileEntry> {
        self.files.get(self.selected_file)
    }

    pub fn selected_diff_text(&self) -> Option<String> {
        let selection = model::document_visible_selection(self.visual_selection.as_ref())?;
        let active = self.active_document.as_ref()?;
        Some(active.document().diff.selected_text(selection))
    }

    pub fn diff_word_selection_at(&self, row: usize, column: usize) -> Option<DiffWordSelection> {
        let text = self
            .active_document
            .as_ref()
            .and_then(|active| active.document().diff.text_at(document::RowIndex(row)))?;
        let chars: Vec<char> = text.chars().collect();
        let ch = *chars.get(column)?;
        if !is_word_selection_char(ch) {
            return None;
        }

        let mut start = column;
        while start > 0 && is_word_selection_char(chars[start - 1]) {
            start -= 1;
        }

        let mut end = column;
        while end + 1 < chars.len() && is_word_selection_char(chars[end + 1]) {
            end += 1;
        }

        Some(DiffWordSelection {
            word: chars[start..=end].iter().collect(),
            start: document::DocumentPosition {
                row: document::RowIndex(row),
                column: document::ColumnIndex(start),
            },
            end: document::DocumentPosition {
                row: document::RowIndex(row),
                column: document::ColumnIndex(end),
            },
        })
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

    pub fn focus_for_file(&self, file_index: usize) -> FileListSectionFocus {
        if file_index < self.unreviewed_count() {
            FileListSectionFocus::Unreviewed
        } else {
            FileListSectionFocus::Reviewed
        }
    }

    pub fn save_selected_file_position(&mut self) {
        let Some(entry) = self.selected_file_entry() else {
            return;
        };
        let path = entry.change.path.clone();
        let content_id = entry.diff.content_id.clone();
        self.saved_file_positions.insert(
            path,
            SavedFilePosition {
                diff_scroll: self.diff_scroll,
                diff_line_cursor: self.diff_line_cursor,
                diff_col_cursor: self.diff_col_cursor,
                content_mode: self.content_mode,
                render_variant: self.render_variant,
                content_id,
            },
        );
    }

    pub fn restore_selected_file_position(&mut self) {
        let Some(entry) = self.selected_file_entry() else {
            return;
        };
        let Some(position) = self.saved_file_positions.get(&entry.change.path).cloned() else {
            return;
        };
        let current_content_id = entry.diff.content_id.clone();

        if position.content_id != current_content_id {
            self.reset_view_for_changed_file();
            return;
        }

        self.content_mode = position.content_mode;
        self.render_variant = position.render_variant;
        if self.current_view_needs_base_content() {
            self.load_base_content_raw();
        }
        self.diff_scroll = position.diff_scroll;
        self.diff_line_cursor = position.diff_line_cursor;
        self.diff_col_cursor = position.diff_col_cursor;
        self.rebuild_active_document();
        self.mark_model_changed();
    }

    fn reset_view_for_changed_file(&mut self) {
        self.content_mode = ContentMode::Diff;
        if !matches!(
            self.render_variant,
            RenderVariant::Inline | RenderVariant::SideBySide
        ) {
            self.render_variant = RenderVariant::Inline;
        }
        self.place_cursor_at_first_hunk();
        if self.current_view_needs_base_content() {
            self.load_base_content_raw();
        } else {
            self.base_content = None;
        }
        self.rebuild_active_document();
        self.mark_model_changed();
    }

    pub fn select_file(
        &mut self,
        file_index: usize,
        section_focus: FileListSectionFocus,
        restore_position: bool,
    ) -> bool {
        if self.files.get(file_index).is_none() {
            return false;
        }
        if file_index == self.selected_file {
            self.file_list_section_focus = section_focus;
            self.mark_model_changed();
            return true;
        }

        self.save_selected_file_position();
        self.selected_file = file_index;
        self.file_list_section_focus = section_focus;
        self.on_file_changed();
        if restore_position {
            self.restore_selected_file_position();
        }
        true
    }

    /// Load content for the currently selected file from the working tree.
    pub fn load_head_content(&mut self) {
        self.load_head_content_raw();
        self.mark_model_changed();
    }

    fn load_head_content_raw(&mut self) {
        self.head_content = None;
        if let Some(entry) = self.files.get(self.selected_file) {
            let path = entry.change.path.clone();
            self.head_content = diff::workdir_file_content(&self.context.worktree, &path);
        }
    }

    /// Load base content for the currently selected file from git.
    pub fn load_base_content(&mut self) {
        self.load_base_content_raw();
        self.mark_model_changed();
    }

    fn load_base_content_raw(&mut self) {
        self.base_content = None;
        if let Some(entry) = self.files.get(self.selected_file) {
            let path = entry.change.path.clone();
            self.base_content = diff::review_base_file_content(
                &self.context.worktree,
                &self.context.base_ref,
                &self.context.merge_base,
                &path,
            );
        }
    }

    /// Load blame data for the currently selected file.
    pub fn load_blame(&mut self) {
        self.load_blame_raw();
        self.mark_model_changed();
    }

    fn load_blame_raw(&mut self) {
        self.head_blame.clear();
        self.base_blame.clear();

        if let Some(entry) = self.files.get(self.selected_file) {
            let path = entry.change.path.clone();
            let (head, base) = diff::blame_pair(
                &self.context.worktree,
                &self.context.base_ref,
                &self.context.merge_base,
                &path,
                self.show_blame,
            );
            self.head_blame = head.into_iter().map(model::BlameLine::from).collect();
            self.base_blame = base.into_iter().map(model::BlameLine::from).collect();
        }
    }

    /// Refresh the diff for the currently selected file from the working tree,
    /// using the current diff algorithm and whitespace settings. Updates the
    /// cached diff in place without changing cursor position.
    pub fn refresh_current_file_diff(&mut self) {
        self.refresh_current_file_diff_raw();
        self.mark_model_changed();
    }

    fn refresh_current_file_diff_raw(&mut self) {
        // Reuse a previously loaded full diff when present. Callers that change
        // algorithm/whitespace use reload_current_diff_raw, which always
        // recomputes.
        if self
            .files
            .get(self.selected_file)
            .is_some_and(|entry| !entry.diff.hunks.is_empty())
        {
            return;
        }

        self.load_selected_file_diff(false);
    }

    /// Load the selected file's full diff from the worktree.
    ///
    /// When `force` is false this is only used for empty hunk bodies. The
    /// server-provided `content_id` is preserved so review-status cache keys
    /// stay stable even when the display algorithm differs.
    fn load_selected_file_diff(&mut self, force: bool) {
        let Some(entry) = self.files.get(self.selected_file) else {
            return;
        };
        if !force && !entry.diff.hunks.is_empty() {
            return;
        }

        let path = entry.change.path.clone();
        let content_id = entry.diff.content_id.clone();
        let diff_base = self.effective_diff_base().to_string();
        let merge_base = self.context.merge_base.clone();
        let Some(mut diff) = diff::diff_with_fallback(
            &self.context.worktree,
            &self.context.base_ref,
            &diff_base,
            &merge_base,
            &path,
            self.diff_algorithm,
            self.ignore_whitespace,
        ) else {
            return;
        };

        // Prefer the status snapshot content_id when present so cache and
        // status comparisons stay aligned with the server review identity.
        if !content_id.is_empty() {
            diff.content_id = content_id;
        }

        if let Some(entry) = self.files.get_mut(self.selected_file) {
            entry.diff = diff;
        }
    }

    /// Reload the diff for the currently selected file, respecting
    /// the `ignore_whitespace` flag.
    pub fn reload_current_diff(&mut self) {
        self.reload_current_diff_raw();
        self.mark_model_changed();
    }

    fn reload_current_diff_raw(&mut self) {
        self.load_selected_file_diff(true);
        self.diff_scroll = self
            .selected_file_entry()
            .and_then(|e| e.diff.hunks.first())
            .map(|h| (h.new_start as usize).saturating_sub(1))
            .unwrap_or(0);
    }

    /// Reload the selected file's bodies without moving cursor or view mode.
    /// Used when the open file mutates under us.
    pub fn refresh_selected_file_content(&mut self) {
        self.refresh_current_file_diff_raw();
        self.load_head_content_raw();
        self.load_blame_raw();
        if self.current_view_needs_base_content() {
            self.load_base_content_raw();
        } else {
            self.base_content = None;
        }
        self.invalidate_diff_search_matches();
        self.rebuild_active_document();
        self.mark_model_changed();
    }

    fn place_cursor_at_first_hunk(&mut self) {
        let first_hunk_row = self
            .selected_file_entry()
            .and_then(|e| e.diff.hunks.first())
            .map(|h| (h.new_start as usize).saturating_sub(1))
            .unwrap_or(0);
        self.diff_line_cursor = first_hunk_row;
        self.diff_col_cursor = 0;
        self.diff_scroll = first_hunk_row;
    }

    /// Called after `selected_file` changes. Resets diff state and loads
    /// the appropriate file content from the working tree.
    pub fn on_file_changed(&mut self) {
        self.reviewed_diff_expanded = false;
        self.refresh_current_file_diff_raw();
        self.load_head_content_raw();
        self.load_blame_raw();

        // Reload base content if the current view needs it.
        if self.current_view_needs_base_content() {
            self.load_base_content_raw();
        } else {
            self.base_content = None;
        }

        self.place_cursor_at_first_hunk();
        self.visual_selection = None;
        self.pending_comment_anchor = None;
        self.selected_comment_id = None;
        self.pending_delete_comment_id = None;
        self.invalidate_diff_search_matches();
        self.rebuild_active_document();
        self.mark_model_changed();
    }

    fn current_view_needs_base_content(&self) -> bool {
        matches!(
            (self.content_mode, self.render_variant),
            (ContentMode::Diff, RenderVariant::SideBySide)
                | (ContentMode::FullFile, RenderVariant::BaseVersion)
        )
    }

    pub fn invalidate_diff_search_matches(&mut self) {
        if self.diff_search_query.is_some()
            && (!self.diff_search_matches.is_empty() || self.diff_search_current != 0)
        {
            self.diff_search_matches.clear();
            self.diff_search_current = 0;
        }
    }

    pub fn clear_transient_interaction_state(&mut self) {
        self.search_results = None;
        self.definition_results = None;
        self.diff_search_matches.clear();
        self.diff_search_current = 0;
        self.visual_selection = None;
        self.pending_comment_anchor = None;
        self.core_interaction.reset_prompt_state();
        self.mark_model_changed();
    }
}

fn is_word_selection_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::document::{DiffDocument, DocumentRow, RowIndex};
    use crate::core::TextAnchor;
    use crate::core::navigation::Direction;
    use crate::core::{
        CommentEffect, CommentsPanelEffect, CoreEffect, DefinitionResultsEffect, DiffCursorEffect,
        PaneEffect, PaneId, SearchResultsEffect, VisualSelectionEffect,
    };
    use crate::protocol::NotificationKind;
    use crate::review_types::{
        AnchorMatchMethod, AnchorPlacementStatus, ChangeKind, CommentAnchorSide, DiffContent,
        DiffHunk, DiffLine, FileChange, FileEntry, LineKind, ReviewStatus,
    };

    struct EmptyViewport;

    impl ViewportMetrics for EmptyViewport {
        fn diff_view_height(&self) -> usize {
            0
        }
    }

    struct RenderedViewport;

    impl RenderedViewport {
        fn new(_lines: Vec<&str>) -> Self {
            Self
        }
    }

    impl ViewportMetrics for RenderedViewport {
        fn diff_view_height(&self) -> usize {
            10
        }
    }

    fn row_has_head_line(row: &DocumentRow, line: u32) -> bool {
        match row {
            DocumentRow::Content(content) => content
                .source
                .line_for_side(CommentAnchorSide::Head)
                .is_some_and(|source_line| source_line == line),
            DocumentRow::Spacer => false,
        }
    }

    fn test_context() -> ConnectionContext {
        ConnectionContext {
            repo_root: "/repo".to_string(),
            worktree: "/repo".to_string(),
            base_ref: "main".to_string(),
            head_ref: "feature".to_string(),
            merge_base: "abc123".to_string(),
        }
    }

    fn empty_state() -> AppState {
        AppState::new(
            crate::config::DiffAlgorithm::Myers,
            test_context(),
            Vec::new(),
            40,
        )
    }

    #[test]
    fn app_clamps_diff_cursor_and_scroll_from_rendered_viewport_counts() {
        let mut state = empty_state();
        state.diff_line_cursor = 50;
        state.diff_scroll = 45;

        assert!(state.clamp_diff_viewport(30, 10));
        assert_eq!(state.diff_line_cursor, 29);
        assert_eq!(state.diff_scroll, 29);

        state.diff_line_cursor = 5;
        state.diff_scroll = 20;
        assert!(state.clamp_diff_viewport(30, 10));
        assert_eq!(state.diff_line_cursor, 5);
        assert_eq!(state.diff_scroll, 5);

        state.diff_line_cursor = 18;
        state.diff_scroll = 5;
        assert!(state.clamp_diff_viewport(30, 10));
        assert_eq!(state.diff_line_cursor, 18);
        assert_eq!(state.diff_scroll, 9);
    }

    #[test]
    fn app_clamps_file_list_scroll_from_selected_rendered_row() {
        let mut state = empty_state();
        state.file_list_scroll = 10;

        assert!(state.clamp_file_list_viewport(Some(4), 5));
        assert_eq!(state.file_list_scroll, 4);

        state.file_list_scroll = 4;
        assert!(state.clamp_file_list_viewport(Some(12), 5));
        assert_eq!(state.file_list_scroll, 8);

        assert!(!state.clamp_file_list_viewport(None, 5));
        assert!(!state.clamp_file_list_viewport(Some(20), 0));
        assert_eq!(state.file_list_scroll, 8);
    }

    fn setup_content_repo(path: &str, base_content: &str, head_content: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temp repo");
        let repo = init_test_repo(dir.path());
        let file_path = dir.path().join(path);
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dirs");
        }
        std::fs::write(&file_path, base_content).expect("write base content");
        commit_all(&repo, "base");
        tag_head(&repo, "base");
        std::fs::write(&file_path, head_content).expect("write head content");
        dir
    }

    fn setup_worktree_repo(files: &[(&str, String)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temp repo");
        let repo = init_test_repo(dir.path());
        for (path, content) in files {
            let file_path = dir.path().join(path);
            if let Some(parent) = file_path.parent() {
                std::fs::create_dir_all(parent).expect("create parent dirs");
            }
            std::fs::write(&file_path, content).expect("write file content");
        }
        commit_all(&repo, "base");
        dir
    }

    fn init_test_repo(path: &std::path::Path) -> git2::Repository {
        let repo = git2::Repository::init(path).expect("init repo");
        let mut config = repo.config().expect("repo config");
        config
            .set_str("user.email", "test@test.com")
            .expect("set email");
        config.set_str("user.name", "Test").expect("set name");
        drop(config);
        repo
    }

    fn commit_all(repo: &git2::Repository, message: &str) -> git2::Oid {
        let mut index = repo.index().expect("repo index");
        index
            .add_all(["."], git2::IndexAddOption::DEFAULT, None)
            .expect("add all");
        index.write().expect("write index");
        let tree_id = index.write_tree().expect("write tree");
        let tree = repo.find_tree(tree_id).expect("find tree");
        let signature = git2::Signature::now("Test", "test@test.com").expect("signature");
        let parent = repo
            .head()
            .ok()
            .and_then(|head| head.target())
            .and_then(|oid| repo.find_commit(oid).ok());
        let parents = parent.iter().collect::<Vec<_>>();

        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            parents.as_slice(),
        )
        .expect("commit")
    }

    fn tag_head(repo: &git2::Repository, name: &str) {
        let head_id = repo.head().expect("head").target().expect("head oid");
        let target = repo.find_object(head_id, None).expect("head object");
        repo.tag_lightweight(name, &target, false)
            .expect("tag head");
    }

    fn test_file(path: &str) -> FileEntry {
        test_file_with_status(path, ReviewStatus::Unreviewed)
    }

    fn with_content_id(mut entry: FileEntry, content_id: &str) -> FileEntry {
        entry.diff.content_id = content_id.to_string();
        entry
    }

    fn test_file_with_status(path: &str, status: ReviewStatus) -> FileEntry {
        FileEntry {
            change: FileChange {
                path: path.to_string(),
                old_path: None,
                kind: ChangeKind::Modified,
            },
            status,
            diff: DiffContent {
                hunks: Vec::new(),
                is_binary: false,
                diff_hash: format!("hash-{path}"),
                content_id: String::new(),
            },
        }
    }

    fn test_file_with_hunk(path: &str) -> FileEntry {
        FileEntry {
            change: FileChange {
                path: path.to_string(),
                old_path: None,
                kind: ChangeKind::Modified,
            },
            status: ReviewStatus::Unreviewed,
            diff: DiffContent {
                hunks: vec![DiffHunk {
                    old_start: 10,
                    old_lines: 6,
                    new_start: 10,
                    new_lines: 6,
                    header: "@@ -10,6 +10,6 @@".to_string(),
                    lines: vec![
                        DiffLine {
                            kind: LineKind::Context,
                            content: "before one".to_string(),
                            old_lineno: Some(10),
                            new_lineno: Some(10),
                        },
                        DiffLine {
                            kind: LineKind::Addition,
                            content: "let alpha = beta;".to_string(),
                            old_lineno: None,
                            new_lineno: Some(11),
                        },
                        DiffLine {
                            kind: LineKind::Addition,
                            content: "let gamma = delta;".to_string(),
                            old_lineno: None,
                            new_lineno: Some(12),
                        },
                        DiffLine {
                            kind: LineKind::Context,
                            content: "after one".to_string(),
                            old_lineno: Some(11),
                            new_lineno: Some(13),
                        },
                        DiffLine {
                            kind: LineKind::Context,
                            content: "after two".to_string(),
                            old_lineno: Some(12),
                            new_lineno: Some(14),
                        },
                        DiffLine {
                            kind: LineKind::Context,
                            content: "after three".to_string(),
                            old_lineno: Some(13),
                            new_lineno: Some(15),
                        },
                    ],
                }],
                is_binary: false,
                diff_hash: format!("hash-{path}"),
                content_id: String::new(),
            },
        }
    }

    fn test_file_with_replacement_hunk(path: &str) -> FileEntry {
        FileEntry {
            change: FileChange {
                path: path.to_string(),
                old_path: None,
                kind: ChangeKind::Modified,
            },
            status: ReviewStatus::Unreviewed,
            diff: DiffContent {
                hunks: vec![DiffHunk {
                    old_start: 15,
                    old_lines: 3,
                    new_start: 15,
                    new_lines: 3,
                    header: "@@ -15,3 +15,3 @@".to_string(),
                    lines: vec![
                        DiffLine {
                            kind: LineKind::Context,
                            content: "before".to_string(),
                            old_lineno: Some(15),
                            new_lineno: Some(15),
                        },
                        DiffLine {
                            kind: LineKind::Deletion,
                            content: "old".to_string(),
                            old_lineno: Some(16),
                            new_lineno: None,
                        },
                        DiffLine {
                            kind: LineKind::Addition,
                            content: "new".to_string(),
                            old_lineno: None,
                            new_lineno: Some(16),
                        },
                        DiffLine {
                            kind: LineKind::Context,
                            content: "after".to_string(),
                            old_lineno: Some(17),
                            new_lineno: Some(17),
                        },
                    ],
                }],
                is_binary: false,
                diff_hash: format!("hash-{path}"),
                content_id: String::new(),
            },
        }
    }

    fn test_file_with_offset_multiline_replacement_hunk(path: &str) -> FileEntry {
        FileEntry {
            change: FileChange {
                path: path.to_string(),
                old_path: None,
                kind: ChangeKind::Modified,
            },
            status: ReviewStatus::Unreviewed,
            diff: DiffContent {
                hunks: vec![DiffHunk {
                    old_start: 26,
                    old_lines: 1,
                    new_start: 27,
                    new_lines: 2,
                    header: "@@ -26 +27,2 @@".to_string(),
                    lines: vec![
                        DiffLine {
                            kind: LineKind::Deletion,
                            content: "old line".to_string(),
                            old_lineno: Some(26),
                            new_lineno: None,
                        },
                        DiffLine {
                            kind: LineKind::Addition,
                            content: "new line one".to_string(),
                            old_lineno: None,
                            new_lineno: Some(27),
                        },
                        DiffLine {
                            kind: LineKind::Addition,
                            content: "new line two".to_string(),
                            old_lineno: None,
                            new_lineno: Some(28),
                        },
                    ],
                }],
                is_binary: false,
                diff_hash: format!("hash-{path}"),
                content_id: String::new(),
            },
        }
    }

    fn test_file_with_deletion_to_later_head_context_hunk(path: &str) -> FileEntry {
        let mut lines = vec![DiffLine {
            kind: LineKind::Deletion,
            content: "navigate_to_comment(state, view, &comment);".to_string(),
            old_lineno: Some(154),
            new_lineno: None,
        }];
        for line in 172..=175 {
            lines.push(DiffLine {
                kind: LineKind::Addition,
                content: format!("head change {line}"),
                old_lineno: None,
                new_lineno: Some(line),
            });
        }
        for offset in 0..13 {
            let old_lineno = 155 + offset;
            let new_lineno = 176 + offset;
            lines.push(DiffLine {
                kind: LineKind::Context,
                content: format!("shared context {old_lineno}/{new_lineno}"),
                old_lineno: Some(old_lineno),
                new_lineno: Some(new_lineno),
            });
        }

        FileEntry {
            change: FileChange {
                path: path.to_string(),
                old_path: None,
                kind: ChangeKind::Modified,
            },
            status: ReviewStatus::Unreviewed,
            diff: DiffContent {
                hunks: vec![DiffHunk {
                    old_start: 154,
                    old_lines: 14,
                    new_start: 172,
                    new_lines: 17,
                    header: "@@ -154,14 +172,17 @@".to_string(),
                    lines,
                }],
                is_binary: false,
                diff_hash: format!("hash-{path}"),
                content_id: String::new(),
            },
        }
    }

    fn test_file_with_shared_tail_offset_hunk(path: &str) -> FileEntry {
        let mut lines = vec![
            DiffLine {
                kind: LineKind::Deletion,
                content: "old changed".to_string(),
                old_lineno: Some(18),
                new_lineno: None,
            },
            DiffLine {
                kind: LineKind::Addition,
                content: "new changed".to_string(),
                old_lineno: None,
                new_lineno: Some(20),
            },
        ];
        for offset in 0..8 {
            let old_lineno = 19 + offset;
            let new_lineno = 21 + offset;
            lines.push(DiffLine {
                kind: LineKind::Context,
                content: format!("context {old_lineno}/{new_lineno}"),
                old_lineno: Some(old_lineno),
                new_lineno: Some(new_lineno),
            });
        }
        lines.push(DiffLine {
            kind: LineKind::Addition,
            content: "new trailing addition".to_string(),
            old_lineno: None,
            new_lineno: Some(29),
        });

        FileEntry {
            change: FileChange {
                path: path.to_string(),
                old_path: None,
                kind: ChangeKind::Modified,
            },
            status: ReviewStatus::Unreviewed,
            diff: DiffContent {
                hunks: vec![DiffHunk {
                    old_start: 18,
                    old_lines: 9,
                    new_start: 20,
                    new_lines: 10,
                    header: "@@ -18,9 +20,10 @@".to_string(),
                    lines,
                }],
                is_binary: false,
                diff_hash: format!("hash-{path}"),
                content_id: String::new(),
            },
        }
    }

    fn test_file_with_migration_offset_replacement_hunk(path: &str) -> FileEntry {
        FileEntry {
            change: FileChange {
                path: path.to_string(),
                old_path: None,
                kind: ChangeKind::Modified,
            },
            status: ReviewStatus::Unreviewed,
            diff: DiffContent {
                hunks: vec![DiffHunk {
                    old_start: 29,
                    old_lines: 1,
                    new_start: 31,
                    new_lines: 1,
                    header: "@@ -29 +31 @@".to_string(),
                    lines: vec![
                        DiffLine {
                            kind: LineKind::Deletion,
                            content: "a.context_before, a.context_after, a.status".to_string(),
                            old_lineno: Some(29),
                            new_lineno: None,
                        },
                        DiffLine {
                            kind: LineKind::Addition,
                            content: "a.context_before, a.context_after, a.status, ''".to_string(),
                            old_lineno: None,
                            new_lineno: Some(31),
                        },
                    ],
                }],
                is_binary: false,
                diff_hash: format!("hash-{path}"),
                content_id: String::new(),
            },
        }
    }

    fn test_file_with_long_offset_replacement_hunk(path: &str) -> FileEntry {
        let mut lines = vec![
            DiffLine {
                kind: LineKind::Deletion,
                content: "let line = current_head_line(state)?;".to_string(),
                old_lineno: Some(18),
                new_lineno: None,
            },
            DiffLine {
                kind: LineKind::Addition,
                content: "let line = current_visible_line(state)?;".to_string(),
                old_lineno: None,
                new_lineno: Some(20),
            },
        ];
        for offset in 0..30 {
            lines.push(DiffLine {
                kind: LineKind::Context,
                content: format!("context {}", offset + 1),
                old_lineno: Some(19 + offset),
                new_lineno: Some(21 + offset),
            });
        }

        FileEntry {
            change: FileChange {
                path: path.to_string(),
                old_path: None,
                kind: ChangeKind::Modified,
            },
            status: ReviewStatus::Unreviewed,
            diff: DiffContent {
                hunks: vec![DiffHunk {
                    old_start: 18,
                    old_lines: 31,
                    new_start: 20,
                    new_lines: 31,
                    header: "@@ -18,31 +20,31 @@".to_string(),
                    lines,
                }],
                is_binary: false,
                diff_hash: format!("hash-{path}"),
                content_id: String::new(),
            },
        }
    }

    fn test_file_with_leading_deletion_hunk(path: &str) -> FileEntry {
        FileEntry {
            change: FileChange {
                path: path.to_string(),
                old_path: None,
                kind: ChangeKind::Modified,
            },
            status: ReviewStatus::Unreviewed,
            diff: DiffContent {
                hunks: vec![DiffHunk {
                    old_start: 16,
                    old_lines: 2,
                    new_start: 16,
                    new_lines: 1,
                    header: "@@ -16,2 +16,1 @@".to_string(),
                    lines: vec![
                        DiffLine {
                            kind: LineKind::Deletion,
                            content: "removed".to_string(),
                            old_lineno: Some(16),
                            new_lineno: None,
                        },
                        DiffLine {
                            kind: LineKind::Context,
                            content: "kept".to_string(),
                            old_lineno: Some(17),
                            new_lineno: Some(16),
                        },
                    ],
                }],
                is_binary: false,
                diff_hash: format!("hash-{path}"),
                content_id: String::new(),
            },
        }
    }

    fn test_file_with_two_hunks(path: &str) -> FileEntry {
        FileEntry {
            change: FileChange {
                path: path.to_string(),
                old_path: None,
                kind: ChangeKind::Modified,
            },
            status: ReviewStatus::Unreviewed,
            diff: DiffContent {
                hunks: vec![
                    DiffHunk {
                        old_start: 10,
                        old_lines: 2,
                        new_start: 10,
                        new_lines: 2,
                        header: "@@ -10,2 +10,2 @@".to_string(),
                        lines: vec![
                            DiffLine {
                                kind: LineKind::Deletion,
                                content: "old first".to_string(),
                                old_lineno: Some(10),
                                new_lineno: None,
                            },
                            DiffLine {
                                kind: LineKind::Addition,
                                content: "new first".to_string(),
                                old_lineno: None,
                                new_lineno: Some(10),
                            },
                        ],
                    },
                    DiffHunk {
                        old_start: 20,
                        old_lines: 2,
                        new_start: 20,
                        new_lines: 2,
                        header: "@@ -20,2 +20,2 @@".to_string(),
                        lines: vec![
                            DiffLine {
                                kind: LineKind::Deletion,
                                content: "old second".to_string(),
                                old_lineno: Some(20),
                                new_lineno: None,
                            },
                            DiffLine {
                                kind: LineKind::Addition,
                                content: "new second".to_string(),
                                old_lineno: None,
                                new_lineno: Some(20),
                            },
                        ],
                    },
                ],
                is_binary: false,
                diff_hash: format!("hash-{path}"),
                content_id: String::new(),
            },
        }
    }

    fn reviewed() -> ReviewStatus {
        ReviewStatus::Reviewed {
            at: "2026-06-22T00:00:00Z".to_string(),
            reviewed_commit: Some("reviewed-head".to_string()),
        }
    }

    fn notification(kind: NotificationKind) -> Notification {
        Notification {
            base_ref: "main".to_string(),
            head_ref: "feature".to_string(),
            kind,
        }
    }

    fn stored_comment(id: i64, file_path: &str) -> crate::review_types::Comment {
        crate::review_types::Comment::new(crate::review_types::CommentInit {
            id,
            merge_base: "abc123".to_string(),
            head_ref: "feature".to_string(),
            created_head_commit: "head-commit".to_string(),
            anchor: test_anchor(file_path, 2, 2, "anchor"),
            body: "comment".to_string(),
            resolved: false,
            created_at: "2026-06-28T00:00:00+10:00".to_string(),
            updated_at: "2026-06-28T00:00:00+10:00".to_string(),
            anchor_status: crate::review_types::AnchorStatus::Anchored,
        })
        .expect("test comment anchor should be valid")
    }

    fn test_anchor(
        file_path: &str,
        line_start: i64,
        line_end: i64,
        anchor_text: &str,
    ) -> crate::review_types::CommentAnchor {
        crate::review_types::CommentAnchor {
            segments: vec![crate::review_types::CommentAnchorSegment {
                side: crate::review_types::CommentAnchorSide::Head,
                file_path: file_path.to_string(),
                line_start,
                line_end,
                char_start: None,
                char_end: None,
                anchor_text: anchor_text.to_string(),
                context_before: String::new(),
                context_after: String::new(),
                placement_status: crate::review_types::AnchorPlacementStatus::Anchored,
                match_method: crate::review_types::AnchorMatchMethod::ExactAtLine,
            }],
            aggregate_status: crate::review_types::AnchorAggregateStatus::Anchored,
        }
    }

    fn move_comment_head_range(
        comment: &mut crate::review_types::Comment,
        line_start: i64,
        line_end: i64,
    ) {
        let file_path = comment.file_path().to_string();
        let anchor_text = comment.anchor_text().to_string();
        comment
            .replace_anchor(test_anchor(&file_path, line_start, line_end, &anchor_text))
            .expect("test comment anchor should be valid");
    }

    fn move_comment_to_base_head_ranges(
        comment: &mut crate::review_types::Comment,
        base_line_start: i64,
        base_line_end: i64,
        head_line_start: i64,
        head_line_end: i64,
    ) {
        let file_path = comment.file_path().to_string();
        let anchor = crate::review_types::CommentAnchor {
            segments: vec![
                crate::review_types::CommentAnchorSegment {
                    side: crate::review_types::CommentAnchorSide::Base,
                    file_path: file_path.clone(),
                    line_start: base_line_start,
                    line_end: base_line_end,
                    char_start: None,
                    char_end: None,
                    anchor_text: "old line".to_string(),
                    context_before: String::new(),
                    context_after: String::new(),
                    placement_status: crate::review_types::AnchorPlacementStatus::Anchored,
                    match_method: crate::review_types::AnchorMatchMethod::ExactAtLine,
                },
                crate::review_types::CommentAnchorSegment {
                    side: crate::review_types::CommentAnchorSide::Head,
                    file_path,
                    line_start: head_line_start,
                    line_end: head_line_end,
                    char_start: None,
                    char_end: None,
                    anchor_text: "new line".to_string(),
                    context_before: String::new(),
                    context_after: String::new(),
                    placement_status: crate::review_types::AnchorPlacementStatus::Anchored,
                    match_method: crate::review_types::AnchorMatchMethod::ExactAtLine,
                },
            ],
            aggregate_status: crate::review_types::AnchorAggregateStatus::Anchored,
        };
        comment
            .replace_anchor(anchor)
            .expect("test comment anchor should be valid");
    }

    fn segment_for_side(
        capture: &CommentAnchorCapture,
        side: CommentAnchorSide,
    ) -> Option<&CommentAnchorSegment> {
        capture.segments.iter().find(|segment| segment.side == side)
    }

    fn active_document_len(state: &AppState) -> usize {
        state
            .active_document
            .as_ref()
            .map(|document| document.document().diff.len())
            .unwrap_or(0)
    }

    fn replace_active_document_with_empty_sentinel(state: &mut AppState) {
        let key = state
            .active_document
            .as_ref()
            .expect("active document")
            .document()
            .key
            .clone();
        let document = document::ActiveDocument {
            key,
            diff: document::DiffDocument::Unified(document::Document::new(Vec::new(), Vec::new())),
        };
        state.active_document = Some(state.active_document_state(document));
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
    fn file_load_builds_active_document_in_app_state() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_hunk("src/lib.rs")],
        );

        app.state.on_file_changed();

        assert!(app.state.active_document.is_some());
        assert_eq!(
            app.state.active_document.as_ref().map(|document| document
                .document
                .key
                .file_id
                .as_str()),
            Some("src/lib.rs")
        );
        assert!(active_document_len(&app.state) > 0);
    }

    #[test]
    fn selecting_file_rebuilds_active_document_for_new_key() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![
                test_file_with_hunk("src/a.rs"),
                test_file_with_hunk("src/b.rs"),
            ],
        );
        app.state.on_file_changed();

        let original_key = app
            .state
            .active_document
            .as_ref()
            .expect("active document")
            .document()
            .key
            .clone();

        assert!(
            app.state
                .select_file(1, FileListSectionFocus::Unreviewed, false)
        );

        let active_document = app.state.active_document.as_ref().expect("active document");
        assert_ne!(active_document.document().key, original_key);
        assert_eq!(active_document.document().key.file_id, "src/b.rs");
    }

    #[test]
    fn mark_model_changed_refreshes_active_document_state() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_hunk("src/lib.rs")],
        );
        app.state.on_file_changed();
        app.state.diff_scroll = 3;
        app.state.diff_line_cursor = 4;
        app.state.diff_col_cursor = 2;
        app.state.visual_selection = Some(VisualSelection {
            mode: VisualSelectionMode::Text,
            start: document::DocumentPosition {
                row: document::RowIndex(1),
                column: document::ColumnIndex(2),
            },
            end: document::DocumentPosition {
                row: document::RowIndex(4),
                column: document::ColumnIndex(8),
            },
        });

        app.state.mark_model_changed();

        let active_document = app.state.active_document.as_ref().expect("active document");
        assert_eq!(active_document.scroll(), document::RowIndex(3));
        assert_eq!(
            active_document.cursor(),
            document::DocumentPosition {
                row: document::RowIndex(4),
                column: document::ColumnIndex(2),
            }
        );
        assert_eq!(
            active_document.visual_selection(),
            app.state.visual_selection.as_ref()
        );
    }

    #[test]
    fn render_mode_change_rebuilds_active_document_for_new_key() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_hunk("src/lib.rs")],
        );
        app.state.on_file_changed();
        replace_active_document_with_empty_sentinel(&mut app.state);

        app.state.render_variant = RenderVariant::SideBySide;
        app.state.ensure_active_document();

        let active_document = app.state.active_document.as_ref().expect("active document");
        assert_eq!(
            active_document.document().key.content_mode,
            ContentMode::Diff
        );
        assert_eq!(
            active_document.document().key.render_variant,
            RenderVariant::SideBySide
        );
        assert!(active_document.document().diff.len() > 0);
    }

    #[test]
    fn cursor_scroll_search_and_comment_changes_refresh_without_rebuild() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_hunk("src/lib.rs")],
        );
        app.state.on_file_changed();
        replace_active_document_with_empty_sentinel(&mut app.state);

        app.state.diff_line_cursor = 4;
        app.state.diff_col_cursor = 2;
        app.state.diff_scroll = 3;
        app.state.diff_search_query = Some("line".to_string());
        app.state.comments = vec![stored_comment(1, "src/lib.rs")];
        app.state.comments[0].resolved = true;
        app.state.selected_comment_id = Some(1);
        app.state.ensure_active_document();

        assert_eq!(active_document_len(&app.state), 0);

        app.state.comments[0].resolved = false;
        app.state.ensure_active_document();

        assert_eq!(active_document_len(&app.state), 0);
    }

    #[test]
    fn show_blame_change_rebuilds_active_document_for_new_key() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_hunk("src/lib.rs")],
        );
        app.state.on_file_changed();
        replace_active_document_with_empty_sentinel(&mut app.state);

        app.state.show_blame = true;
        app.state.ensure_active_document();

        let active_document = app.state.active_document.as_ref().expect("active document");
        assert!(active_document.document().key.show_blame);
        assert!(active_document.document().diff.len() > 0);
    }

    #[test]
    fn app_model_exposes_existing_active_document_from_state() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_hunk("src/lib.rs")],
        );
        app.state.on_file_changed();
        replace_active_document_with_empty_sentinel(&mut app.state);

        let model = app.model();

        assert_eq!(
            model
                .active_document
                .as_ref()
                .map(|document| document.document().diff.len()),
            Some(0)
        );
    }

    #[test]
    fn focus_gained_queues_status_refresh_in_app() {
        let mut app = App::new(Config::default(), test_context(), vec![test_file("a.rs")]);

        let effects = app.handle_input(InputEvent::FocusGained, &InteractionContext::default());

        assert!(effects.is_empty());
        assert_eq!(
            app.pending_work.pop_front(),
            Some(AppWork::RefreshReviewStatuses)
        );
    }

    #[test]
    fn review_effect_queues_review_work_in_app() {
        let mut app = App::new(Config::default(), test_context(), vec![test_file("a.rs")]);

        let output = app.apply_core_effects(&EmptyViewport, vec![CoreEffect::ReviewToggle]);

        assert!(output.handled);
        assert_eq!(
            app.pending_work.pop_front(),
            Some(AppWork::ToggleSelectedReview)
        );
    }

    #[test]
    fn undo_effect_queues_undo_work_in_app() {
        let mut app = App::new(Config::default(), test_context(), vec![test_file("a.rs")]);

        let output = app.apply_core_effects(&EmptyViewport, vec![CoreEffect::Undo]);

        assert!(output.handled);
        assert_eq!(app.pending_work.pop_front(), Some(AppWork::UndoLastAction));
    }

    #[test]
    fn app_applies_review_result_and_advances_selection() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("a.rs"), test_file("b.rs")],
        );

        app.apply_review_result(&ReviewActionResult {
            file_path: "a.rs".to_string(),
            status: reviewed(),
        });

        assert_eq!(app.state.files[app.state.selected_file].change.path, "b.rs");
        assert!(matches!(
            app.state
                .files
                .iter()
                .find(|entry| entry.change.path == "a.rs")
                .map(|entry| &entry.status),
            Some(ReviewStatus::Reviewed { .. })
        ));
    }

    #[test]
    fn app_snapshot_replacement_preserves_selected_path_and_cursor() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("a.rs"), test_file("b.rs")],
        );
        app.state.selected_file = 1;
        app.state.diff_line_cursor = 7;
        app.state.diff_col_cursor = 3;
        app.state.diff_scroll = 5;
        app.state.content_mode = ContentMode::FullFile;
        app.state.render_variant = RenderVariant::HeadVersion;

        app.replace_file_snapshot(vec![
            test_file("c.rs"),
            test_file_with_status("b.rs", reviewed()),
            test_file("a.rs"),
        ]);

        let selected = &app.state.files[app.state.selected_file];
        assert_eq!(selected.change.path, "b.rs");
        assert_eq!(app.state.diff_line_cursor, 7);
        assert_eq!(app.state.diff_col_cursor, 3);
        assert_eq!(app.state.diff_scroll, 5);
        assert_eq!(app.state.content_mode, ContentMode::FullFile);
        assert_eq!(app.state.render_variant, RenderVariant::HeadVersion);
    }

    #[test]
    fn file_navigation_restores_last_position_per_file() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("a.rs"), test_file("b.rs")],
        );
        app.state.diff_line_cursor = 42;
        app.state.diff_col_cursor = 3;
        app.state.diff_scroll = 40;

        app.apply_core_effects(
            &EmptyViewport,
            vec![CoreEffect::NavigateFile(Direction::Next)],
        );
        assert_eq!(app.state.files[app.state.selected_file].change.path, "b.rs");

        app.state.diff_line_cursor = 7;
        app.state.diff_col_cursor = 2;
        app.state.diff_scroll = 5;
        app.apply_core_effects(
            &EmptyViewport,
            vec![CoreEffect::NavigateFile(Direction::Prev)],
        );

        assert_eq!(app.state.files[app.state.selected_file].change.path, "a.rs");
        assert_eq!(app.state.diff_line_cursor, 42);
        assert_eq!(app.state.diff_col_cursor, 3);
        assert_eq!(app.state.diff_scroll, 40);

        app.apply_core_effects(
            &EmptyViewport,
            vec![CoreEffect::NavigateFile(Direction::Next)],
        );

        assert_eq!(app.state.files[app.state.selected_file].change.path, "b.rs");
        assert_eq!(app.state.diff_line_cursor, 7);
        assert_eq!(app.state.diff_col_cursor, 2);
        assert_eq!(app.state.diff_scroll, 5);
    }

    #[test]
    fn file_navigation_restores_view_when_content_id_unchanged() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![
                with_content_id(test_file_with_hunk("a.rs"), "v1:a:1"),
                with_content_id(test_file_with_hunk("b.rs"), "v1:b:1"),
            ],
        );
        app.state.content_mode = ContentMode::FullFile;
        app.state.render_variant = RenderVariant::HeadVersion;
        app.state.diff_line_cursor = 42;
        app.state.diff_col_cursor = 3;
        app.state.diff_scroll = 40;

        app.apply_core_effects(
            &EmptyViewport,
            vec![CoreEffect::NavigateFile(Direction::Next)],
        );
        app.apply_core_effects(
            &EmptyViewport,
            vec![CoreEffect::NavigateFile(Direction::Prev)],
        );

        assert_eq!(app.state.files[app.state.selected_file].change.path, "a.rs");
        assert_eq!(app.state.content_mode, ContentMode::FullFile);
        assert_eq!(app.state.render_variant, RenderVariant::HeadVersion);
        assert_eq!(app.state.diff_line_cursor, 42);
        assert_eq!(app.state.diff_col_cursor, 3);
        assert_eq!(app.state.diff_scroll, 40);
    }

    #[test]
    fn file_navigation_resets_changed_file_to_diff_first_hunk() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![
                with_content_id(test_file_with_hunk("a.rs"), "v1:a:1"),
                with_content_id(test_file_with_hunk("b.rs"), "v1:b:1"),
            ],
        );
        app.state.content_mode = ContentMode::FullFile;
        app.state.render_variant = RenderVariant::HeadVersion;
        app.state.diff_line_cursor = 42;
        app.state.diff_scroll = 40;

        app.apply_core_effects(
            &EmptyViewport,
            vec![CoreEffect::NavigateFile(Direction::Next)],
        );
        app.state.content_mode = ContentMode::Diff;
        app.state.render_variant = RenderVariant::SideBySide;

        if let Some(entry) = app
            .state
            .files
            .iter_mut()
            .find(|entry| entry.change.path == "a.rs")
        {
            entry.diff.content_id = "v1:a:2".to_string();
        }

        app.apply_core_effects(
            &EmptyViewport,
            vec![CoreEffect::NavigateFile(Direction::Prev)],
        );

        assert_eq!(app.state.files[app.state.selected_file].change.path, "a.rs");
        assert_eq!(app.state.content_mode, ContentMode::Diff);
        assert_eq!(app.state.render_variant, RenderVariant::SideBySide);
        assert_eq!(app.state.diff_line_cursor, 9);
        assert_eq!(app.state.diff_col_cursor, 0);
        assert_eq!(app.state.diff_scroll, 9);
    }

    #[test]
    fn file_navigation_uses_inline_diff_when_returning_to_changed_file_from_full_file() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![
                with_content_id(test_file_with_hunk("a.rs"), "v1:a:1"),
                with_content_id(test_file_with_hunk("b.rs"), "v1:b:1"),
            ],
        );
        app.state.diff_line_cursor = 42;

        app.apply_core_effects(
            &EmptyViewport,
            vec![CoreEffect::NavigateFile(Direction::Next)],
        );
        app.state.content_mode = ContentMode::FullFile;
        app.state.render_variant = RenderVariant::HeadVersion;

        if let Some(entry) = app
            .state
            .files
            .iter_mut()
            .find(|entry| entry.change.path == "a.rs")
        {
            entry.diff.content_id = "v1:a:2".to_string();
        }

        app.apply_core_effects(
            &EmptyViewport,
            vec![CoreEffect::NavigateFile(Direction::Prev)],
        );

        assert_eq!(app.state.content_mode, ContentMode::Diff);
        assert_eq!(app.state.render_variant, RenderVariant::Inline);
        assert_eq!(app.state.diff_line_cursor, 9);
    }

    #[test]
    fn merge_file_statuses_keeps_cursor_when_open_file_content_changes() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![with_content_id(test_file_with_hunk("a.rs"), "v1:a:1")],
        );
        app.state.content_mode = ContentMode::FullFile;
        app.state.render_variant = RenderVariant::HeadVersion;
        app.state.diff_line_cursor = 7;
        app.state.diff_col_cursor = 3;
        app.state.diff_scroll = 5;

        app.merge_file_statuses(vec![with_content_id(test_file("a.rs"), "v1:a:2")]);

        assert_eq!(app.state.files[app.state.selected_file].change.path, "a.rs");
        assert_eq!(app.state.content_mode, ContentMode::FullFile);
        assert_eq!(app.state.render_variant, RenderVariant::HeadVersion);
        assert_eq!(app.state.diff_line_cursor, 7);
        assert_eq!(app.state.diff_col_cursor, 3);
        assert_eq!(app.state.diff_scroll, 5);
    }

    #[test]
    fn section_navigation_cycles_to_unresolved_comments() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("a.rs"), test_file_with_status("b.rs", reviewed())],
        );
        app.state.comments = vec![stored_comment(9, "b.rs")];

        app.apply_core_effects(
            &EmptyViewport,
            vec![CoreEffect::NavigateFileSection(Direction::Next)],
        );

        assert_eq!(app.state.files[app.state.selected_file].change.path, "b.rs");
        assert_eq!(
            app.state.file_list_section_focus,
            FileListSectionFocus::Reviewed
        );

        app.apply_core_effects(
            &EmptyViewport,
            vec![CoreEffect::NavigateFileSection(Direction::Next)],
        );

        assert_eq!(app.state.files[app.state.selected_file].change.path, "b.rs");
        assert_eq!(
            app.state.file_list_section_focus,
            FileListSectionFocus::UnresolvedComments
        );
        assert_eq!(app.state.selected_comment_id, Some(9));
    }

    #[test]
    fn unresolved_comment_section_navigation_scans_comment_rows() {
        let repo = setup_worktree_repo(&[
            (
                "a.rs",
                (1..=12)
                    .map(|line| format!("a {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            (
                "b.rs",
                (1..=12)
                    .map(|line| format!("b {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        ]);
        let mut context = test_context();
        context.repo_root = repo.path().to_string_lossy().into_owned();
        context.worktree = context.repo_root.clone();
        let mut app = App::new(
            Config::default(),
            context,
            vec![test_file("a.rs"), test_file("b.rs")],
        );
        let mut first = stored_comment(3, "a.rs");
        move_comment_head_range(&mut first, 2, 2);
        let mut second = stored_comment(5, "b.rs");
        move_comment_head_range(&mut second, 6, 6);
        app.state.comments = vec![first, second];
        app.state.pane_focus = PaneFocus::FileList;
        app.state.file_list_section_focus = FileListSectionFocus::UnresolvedComments;
        app.state.selected_comment_id = Some(3);
        app.state.content_mode = ContentMode::FullFile;
        app.state.render_variant = RenderVariant::HeadVersion;

        app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 12]),
            vec![CoreEffect::NavigateFile(Direction::Next)],
        );

        assert_eq!(app.state.selected_comment_id, Some(5));
        assert_eq!(app.state.files[app.state.selected_file].change.path, "b.rs");
        assert_eq!(app.state.pane_focus, PaneFocus::Diff);
        assert!(app.state.show_comments_panel);
        assert_eq!(app.state.diff_line_cursor, 5);
        assert_eq!(
            app.state.file_list_section_focus,
            FileListSectionFocus::UnresolvedComments
        );

        app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 12]),
            vec![CoreEffect::NavigateFile(Direction::Prev)],
        );

        assert_eq!(app.state.selected_comment_id, Some(3));
        assert_eq!(app.state.files[app.state.selected_file].change.path, "a.rs");
        assert_eq!(app.state.diff_line_cursor, 1);
    }

    #[test]
    fn activating_unresolved_comment_row_jumps_to_comment_anchor() {
        let repo = setup_worktree_repo(&[
            (
                "a.rs",
                (1..=12)
                    .map(|line| format!("a {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            (
                "b.rs",
                (1..=12)
                    .map(|line| format!("b {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        ]);
        let mut context = test_context();
        context.repo_root = repo.path().to_string_lossy().into_owned();
        context.worktree = context.repo_root.clone();
        let mut app = App::new(
            Config::default(),
            context,
            vec![test_file("a.rs"), test_file("b.rs")],
        );
        let mut comment = stored_comment(9, "b.rs");
        move_comment_head_range(&mut comment, 4, 4);
        app.state.comments = vec![comment];
        app.state.selected_file = 1;
        app.state.pane_focus = PaneFocus::FileList;
        app.state.file_list_section_focus = FileListSectionFocus::UnresolvedComments;
        app.state.selected_comment_id = Some(9);
        app.state.content_mode = ContentMode::FullFile;
        app.state.render_variant = RenderVariant::HeadVersion;
        app.state.load_head_content();

        app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 12]),
            vec![CoreEffect::Pane(PaneEffect::ActivateFileListSelection)],
        );

        assert_eq!(app.state.pane_focus, PaneFocus::Diff);
        assert_eq!(app.state.selected_comment_id, Some(9));
        assert_eq!(app.state.diff_line_cursor, 3);
        assert_eq!(
            app.state.file_list_section_focus,
            FileListSectionFocus::UnresolvedComments
        );
    }

    #[test]
    fn unresolved_comment_shortcut_advances_from_current_line_to_next_file() {
        let repo = setup_worktree_repo(&[
            (
                "a.rs",
                (1..=12)
                    .map(|line| format!("a {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            (
                "b.rs",
                (1..=12)
                    .map(|line| format!("b {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        ]);
        let mut context = test_context();
        context.repo_root = repo.path().to_string_lossy().into_owned();
        context.worktree = context.repo_root.clone();
        let mut app = App::new(
            Config::default(),
            context,
            vec![test_file("a.rs"), test_file("b.rs")],
        );
        let mut first = stored_comment(3, "a.rs");
        move_comment_head_range(&mut first, 4, 4);
        let mut second = stored_comment(5, "b.rs");
        move_comment_head_range(&mut second, 2, 2);
        app.state.comments = vec![first, second];
        app.state.file_list_section_focus = FileListSectionFocus::UnresolvedComments;
        app.state.content_mode = ContentMode::FullFile;
        app.state.render_variant = RenderVariant::HeadVersion;
        app.state.diff_line_cursor = 4;

        app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 12]),
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Next)],
        );

        assert_eq!(app.state.files[app.state.selected_file].change.path, "b.rs");
        assert_eq!(app.state.selected_comment_id, Some(5));
        assert_eq!(app.state.diff_line_cursor, 1);
        assert_eq!(app.state.pane_focus, PaneFocus::Diff);
        assert!(app.state.show_comments_panel);

        app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 12]),
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Prev)],
        );

        assert_eq!(app.state.files[app.state.selected_file].change.path, "a.rs");
        assert_eq!(app.state.selected_comment_id, Some(3));
        assert_eq!(app.state.diff_line_cursor, 3);
    }

    #[test]
    fn unresolved_comment_shortcut_reports_empty_list() {
        let mut app = App::new(Config::default(), test_context(), vec![test_file("a.rs")]);
        app.state.file_list_section_focus = FileListSectionFocus::UnresolvedComments;
        app.state.selected_comment_id = Some(99);

        let output = app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 12]),
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Next)],
        );

        assert_eq!(
            output.status,
            Some(StatusUpdate::Set("No unresolved comments".to_string()))
        );
        assert_eq!(app.state.selected_comment_id, None);
    }

    #[test]
    fn unresolved_comment_shortcut_visits_same_start_line_comments() {
        let repo = setup_worktree_repo(&[
            (
                "a.rs",
                (1..=40)
                    .map(|line| format!("a {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            (
                "b.rs",
                (1..=40)
                    .map(|line| format!("b {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        ]);
        let mut context = test_context();
        context.repo_root = repo.path().to_string_lossy().into_owned();
        context.worktree = context.repo_root.clone();
        let mut app = App::new(
            Config::default(),
            context,
            vec![test_file("a.rs"), test_file("b.rs")],
        );
        let mut outer = stored_comment(9, "a.rs");
        move_comment_head_range(&mut outer, 18, 29);
        let mut middle = stored_comment(14, "a.rs");
        move_comment_head_range(&mut middle, 18, 21);
        let mut single = stored_comment(15, "a.rs");
        move_comment_head_range(&mut single, 18, 18);
        let mut next_file = stored_comment(20, "b.rs");
        move_comment_head_range(&mut next_file, 4, 4);
        app.state.comments = vec![outer, next_file, single, middle];
        app.state.file_list_section_focus = FileListSectionFocus::UnresolvedComments;
        app.state.content_mode = ContentMode::FullFile;
        app.state.render_variant = RenderVariant::HeadVersion;
        app.state.load_head_content();
        app.state.diff_line_cursor = 16;

        app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 40]),
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Next)],
        );
        assert_eq!(app.state.selected_comment_id, Some(15));
        assert_eq!(app.state.diff_line_cursor, 17);

        app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 40]),
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Next)],
        );
        assert_eq!(app.state.selected_comment_id, Some(14));
        assert_eq!(app.state.diff_line_cursor, 17);

        app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 40]),
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Next)],
        );
        assert_eq!(app.state.selected_comment_id, Some(9));
        assert_eq!(app.state.diff_line_cursor, 17);

        app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 40]),
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Prev)],
        );
        assert_eq!(app.state.selected_comment_id, Some(14));
        assert_eq!(app.state.diff_line_cursor, 17);

        app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 40]),
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Prev)],
        );
        assert_eq!(app.state.selected_comment_id, Some(15));
        assert_eq!(app.state.diff_line_cursor, 17);

        app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 40]),
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Next)],
        );
        assert_eq!(app.state.selected_comment_id, Some(14));

        app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 40]),
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Next)],
        );
        assert_eq!(app.state.selected_comment_id, Some(9));

        app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 40]),
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Next)],
        );
        assert_eq!(app.state.files[app.state.selected_file].change.path, "b.rs");
        assert_eq!(app.state.selected_comment_id, Some(20));

        app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 40]),
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Prev)],
        );
        assert_eq!(app.state.files[app.state.selected_file].change.path, "a.rs");
        assert_eq!(app.state.selected_comment_id, Some(9));
    }

    #[test]
    fn unresolved_comment_shortcut_advances_from_deletion_row() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_offset_multiline_replacement_hunk("a.rs")],
        );
        let mut current = stored_comment(1, "a.rs");
        move_comment_to_base_head_ranges(&mut current, 26, 26, 27, 27);
        let mut next = stored_comment(2, "a.rs");
        move_comment_head_range(&mut next, 28, 28);
        app.state.comments = vec![current, next];
        app.state.file_list_section_focus = FileListSectionFocus::UnresolvedComments;
        app.state.content_mode = ContentMode::Diff;
        app.state.render_variant = RenderVariant::Inline;
        app.state.diff_line_cursor = 0;
        app.state.selected_comment_id = Some(1);

        app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 12]),
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Next)],
        );

        assert_eq!(app.state.selected_comment_id, Some(2));
        assert_eq!(app.state.diff_line_cursor, 2);
    }

    #[test]
    fn unresolved_comment_shortcut_uses_document_row_order_before_crossing_files() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_hunk("a.rs"), test_file_with_hunk("b.rs")],
        );
        let mut current = stored_comment(1, "a.rs");
        move_comment_head_range(&mut current, 11, 11);
        let mut next_same_file = stored_comment(2, "a.rs");
        move_comment_head_range(&mut next_same_file, 12, 12);
        let mut next_file = stored_comment(3, "b.rs");
        move_comment_head_range(&mut next_file, 11, 11);
        app.state.comments = vec![current, next_file, next_same_file];
        app.state.file_list_section_focus = FileListSectionFocus::UnresolvedComments;
        app.state.content_mode = ContentMode::Diff;
        app.state.render_variant = RenderVariant::Inline;
        app.state.diff_line_cursor = 0;
        app.state.selected_comment_id = Some(1);
        app.state.ensure_active_document();

        app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 12]),
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Next)],
        );

        assert_eq!(app.state.files[app.state.selected_file].change.path, "a.rs");
        assert_eq!(app.state.selected_comment_id, Some(2));

        app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 12]),
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Next)],
        );

        assert_eq!(app.state.files[app.state.selected_file].change.path, "b.rs");
        assert_eq!(app.state.selected_comment_id, Some(3));
    }

    #[test]
    fn unresolved_comment_shortcut_cross_file_uses_target_document_row() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![
                test_file("a.rs"),
                test_file_with_offset_multiline_replacement_hunk("b.rs"),
            ],
        );
        let mut current = stored_comment(1, "a.rs");
        move_comment_head_range(&mut current, 2, 2);
        let mut next_file = stored_comment(2, "b.rs");
        move_comment_to_base_head_ranges(&mut next_file, 26, 26, 27, 28);
        app.state.comments = vec![current, next_file];
        app.state.file_list_section_focus = FileListSectionFocus::UnresolvedComments;
        app.state.content_mode = ContentMode::Diff;
        app.state.render_variant = RenderVariant::Inline;
        app.state.diff_line_cursor = 1;
        app.state.selected_comment_id = Some(1);
        app.state.ensure_active_document();

        app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 12]),
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Next)],
        );

        let document = app
            .state
            .active_document
            .as_ref()
            .expect("target file should have an active document");
        let Some(span) = document.document().diff.comment_span(2) else {
            panic!("target comment should be projected into the active document");
        };
        assert_eq!(app.state.files[app.state.selected_file].change.path, "b.rs");
        assert_eq!(app.state.selected_comment_id, Some(2));
        assert_eq!(app.state.diff_line_cursor, span.start.0);
        assert_eq!(span.start.0, 0);
    }

    #[test]
    fn unresolved_comment_shortcut_cross_file_clamps_against_target_document() {
        let plan_path = "docs/plans/completed/29d-document-comment-overlays.md";
        let app_path = "src/app.rs";
        let plan_content = (1..=132)
            .map(|line| format!("plan {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let app_content = (1..=1600)
            .map(|line| format!("app {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let repo =
            setup_worktree_repo(&[(plan_path, plan_content.clone()), (app_path, app_content)]);
        let mut context = test_context();
        context.repo_root = repo.path().to_string_lossy().into_owned();
        context.worktree = context.repo_root.clone();
        let mut app = App::new(
            Config::default(),
            context,
            vec![test_file(plan_path), test_file(app_path)],
        );
        let mut comment = stored_comment(59, app_path);
        move_comment_head_range(&mut comment, 1505, 1513);
        app.state.comments = vec![comment];
        app.state.file_list_section_focus = FileListSectionFocus::UnresolvedComments;
        app.state.content_mode = ContentMode::FullFile;
        app.state.render_variant = RenderVariant::HeadVersion;
        app.state.selected_file = 0;
        app.state.head_content = Some(plan_content);
        app.state.diff_line_cursor = 131;
        app.state.diff_scroll = 122;

        app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 132]),
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Next)],
        );

        assert_eq!(
            app.state.files[app.state.selected_file].change.path,
            app_path
        );
        assert_eq!(app.state.selected_comment_id, Some(59));
        assert_eq!(app.state.diff_line_cursor, 1504);
        assert_eq!(app.state.diff_scroll, 1504);
    }

    #[test]
    fn unresolved_comment_shortcut_cross_file_uses_first_target_document_comment() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![
                test_file("a.rs"),
                test_file_with_offset_multiline_replacement_hunk("b.rs"),
            ],
        );
        let mut current = stored_comment(1, "a.rs");
        move_comment_head_range(&mut current, 2, 2);
        let mut hidden_first = stored_comment(2, "b.rs");
        move_comment_head_range(&mut hidden_first, 20, 20);
        let mut visible_second = stored_comment(3, "b.rs");
        move_comment_to_base_head_ranges(&mut visible_second, 26, 26, 27, 28);
        app.state.comments = vec![current, hidden_first, visible_second];
        app.state.file_list_section_focus = FileListSectionFocus::UnresolvedComments;
        app.state.content_mode = ContentMode::Diff;
        app.state.render_variant = RenderVariant::Inline;
        app.state.diff_line_cursor = 1;
        app.state.selected_comment_id = Some(1);
        app.state.ensure_active_document();

        app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 12]),
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Next)],
        );

        assert_eq!(app.state.files[app.state.selected_file].change.path, "b.rs");
        assert_eq!(app.state.selected_comment_id, Some(3));
        assert_eq!(app.state.diff_line_cursor, 0);
    }

    #[test]
    fn unresolved_comment_shortcut_cross_file_does_not_use_source_row_for_hidden_comment() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![
                test_file("a.rs"),
                test_file_with_offset_multiline_replacement_hunk("b.rs"),
            ],
        );
        let mut current = stored_comment(1, "a.rs");
        move_comment_head_range(&mut current, 2, 2);
        let mut hidden = stored_comment(2, "b.rs");
        move_comment_head_range(&mut hidden, 40, 40);
        app.state.comments = vec![current, hidden];
        app.state.file_list_section_focus = FileListSectionFocus::UnresolvedComments;
        app.state.content_mode = ContentMode::Diff;
        app.state.render_variant = RenderVariant::Inline;
        app.state.diff_line_cursor = 1;
        app.state.selected_comment_id = Some(1);
        app.state.ensure_active_document();

        let output = app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 12]),
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Next)],
        );

        assert_eq!(app.state.files[app.state.selected_file].change.path, "b.rs");
        assert_eq!(app.state.selected_comment_id, Some(2));
        assert_ne!(app.state.diff_line_cursor, 39);
        assert_eq!(
            output.status,
            Some(StatusUpdate::Set(
                "Comment is not visible in this view".to_string()
            ))
        );
    }

    #[test]
    fn unresolved_comment_shortcut_prev_skips_current_multiline_range() {
        let mut app = App::new(Config::default(), test_context(), vec![test_file("a.rs")]);
        let mut previous = stored_comment(1, "a.rs");
        move_comment_head_range(&mut previous, 130, 130);
        let mut current = stored_comment(2, "a.rs");
        move_comment_head_range(&mut current, 182, 191);
        app.state.comments = vec![previous, current];
        app.state.file_list_section_focus = FileListSectionFocus::UnresolvedComments;
        app.state.content_mode = ContentMode::FullFile;
        app.state.render_variant = RenderVariant::HeadVersion;
        app.state.head_content = Some(
            (1..=220)
                .map(|line| format!("line {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        app.state.diff_line_cursor = 183;
        app.state.selected_comment_id = Some(2);

        app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 220]),
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Prev)],
        );

        assert_eq!(app.state.selected_comment_id, Some(1));
        assert_eq!(app.state.diff_line_cursor, 129);
    }

    #[test]
    fn unresolved_comment_navigation_selects_out_of_range_comment() {
        let mut app = App::new(Config::default(), test_context(), vec![test_file("a.rs")]);
        let mut comment = stored_comment(8, "mcp.rs");
        move_comment_head_range(&mut comment, 12, 12);
        comment.body = "Previous session feedback".to_string();
        app.state.comments = vec![comment];
        app.state.file_list_section_focus = FileListSectionFocus::UnresolvedComments;

        app.apply_core_effects(
            &RenderedViewport::new(vec!["line"; 20]),
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Next)],
        );

        assert_eq!(app.state.files[app.state.selected_file].change.path, "a.rs");
        assert_eq!(app.state.selected_comment_id, Some(8));
        assert_eq!(app.state.pane_focus, PaneFocus::Comments);
        assert!(app.state.show_comments_panel);

        let model = app.model();
        assert_eq!(model.comments_panel.comments.len(), 1);
        assert_eq!(model.comments_panel.comments[0].file_path, "mcp.rs");
        assert_eq!(model.comments_panel.comments[0].id, 8);
    }

    #[test]
    fn active_diff_search_recomputes_after_file_change() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_hunk("a.rs"), test_file_with_hunk("b.rs")],
        );
        app.state.diff_search_query = Some("alpha".to_string());
        app.state.diff_search_matches = vec![(8, 1, 8)];
        app.state.diff_search_current = 0;

        app.state.selected_file = 1;
        app.state.on_file_changed();

        assert_eq!(app.state.diff_search_query.as_deref(), Some("alpha"));
        assert!(app.state.diff_search_matches.is_empty());

        let view = RenderedViewport::new(vec!["rendered viewport should be ignored"]);
        assert!(app.refresh_active_diff_search(&view));

        assert_eq!(app.state.diff_search_matches, vec![(1, 4, 9)]);
        assert_eq!(app.state.diff_search_current, 0);
        assert_eq!(app.state.diff_line_cursor, 1);

        app.state.diff_line_cursor = 0;

        assert!(!app.refresh_active_diff_search(&view));
        assert_eq!(app.state.diff_line_cursor, 0);
    }

    #[test]
    fn hunk_navigation_uses_active_document_hunks_not_viewport_rows() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_two_hunks("src/main.rs")],
        );
        app.state.rebuild_active_document();
        let view = RenderedViewport::new(vec!["viewport has no hunk rows"]);

        app.apply_core_effects(&view, vec![CoreEffect::JumpHunk(Direction::Next)]);

        assert_eq!(app.state.diff_line_cursor, 2);
        assert_eq!(app.state.diff_col_cursor, 0);

        app.apply_core_effects(&view, vec![CoreEffect::JumpHunk(Direction::Prev)]);

        assert_eq!(app.state.diff_line_cursor, 0);
        assert_eq!(app.state.diff_col_cursor, 0);
    }

    #[test]
    fn view_mode_cycle_preserves_cursor_from_active_document_source() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_hunk("src/main.rs")],
        );
        app.state.head_content = Some(
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n\
             let alpha = beta;\nlet gamma = delta;\nafter one\n"
                .to_string(),
        );
        app.state.base_content = Some(
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n\
             before one\nafter one\n"
                .to_string(),
        );
        app.state.diff_line_cursor = 0;
        app.state.rebuild_active_document();
        let view = RenderedViewport::new(vec!["viewport text ignored"]);
        let original_source = app
            .state
            .active_document
            .as_ref()
            .and_then(|active| {
                active
                    .document()
                    .diff
                    .source_at(RowIndex(app.state.diff_line_cursor))
            })
            .expect("original source");

        app.apply_core_effects(&view, vec![CoreEffect::CycleViewMode]);

        assert_eq!(app.state.content_mode, ContentMode::FullFile);
        assert_eq!(app.state.render_variant, RenderVariant::HeadVersion);
        assert_eq!(
            app.state
                .active_document
                .as_ref()
                .and_then(|active| {
                    active
                        .document()
                        .diff
                        .source_at(document::RowIndex(app.state.diff_line_cursor))
                })
                .and_then(|source| source.head),
            original_source.head
        );

        app.apply_core_effects(&view, vec![CoreEffect::CycleViewMode]);

        assert_eq!(app.state.render_variant, RenderVariant::BaseVersion);
        assert_eq!(
            app.state
                .active_document
                .as_ref()
                .and_then(|active| {
                    active
                        .document()
                        .diff
                        .source_at(document::RowIndex(app.state.diff_line_cursor))
                })
                .and_then(|source| source.base),
            original_source.base
        );

        app.apply_core_effects(&view, vec![CoreEffect::CycleViewMode]);

        assert_eq!(app.state.content_mode, ContentMode::Diff);
        assert_eq!(app.state.render_variant, RenderVariant::Inline);
        assert_eq!(
            app.state.active_document.as_ref().and_then(|active| {
                active
                    .document()
                    .diff
                    .source_at(RowIndex(app.state.diff_line_cursor))
            }),
            Some(original_source)
        );
    }

    #[test]
    fn inline_diff_toggle_switches_between_inline_and_side_by_side() {
        let mut app = App::new(Config::default(), test_context(), vec![test_file("a.rs")]);
        app.state.diff_search_query = Some("Command".to_string());
        app.state.diff_search_matches = vec![(4, 1, 8)];

        assert_eq!(app.state.render_variant, RenderVariant::Inline);

        app.apply_core_effects(&EmptyViewport, vec![CoreEffect::ToggleInlineDiff]);

        assert_eq!(app.state.render_variant, RenderVariant::SideBySide);
        assert_eq!(app.state.diff_search_query.as_deref(), Some("Command"));
        assert!(app.state.diff_search_matches.is_empty());

        app.apply_core_effects(&EmptyViewport, vec![CoreEffect::ToggleInlineDiff]);

        assert_eq!(app.state.render_variant, RenderVariant::Inline);
    }

    #[test]
    fn inline_diff_toggle_preserves_current_source_line() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_offset_multiline_replacement_hunk(
                "src/main.rs",
            )],
        );
        app.state.render_variant = RenderVariant::Inline;
        app.state.base_content = Some(
            (1..=32)
                .map(|n| format!("base {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        app.state.head_content = Some(
            (1..=34)
                .map(|n| format!("head {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        app.state.diff_line_cursor = 27;

        app.apply_core_effects(&EmptyViewport, vec![CoreEffect::ToggleInlineDiff]);

        assert_eq!(app.state.render_variant, RenderVariant::SideBySide);
        assert_eq!(app.state.diff_line_cursor, 26);
        let source = app
            .state
            .active_document
            .as_ref()
            .and_then(|document| {
                document
                    .document()
                    .diff
                    .source_at(document::RowIndex(app.state.diff_line_cursor))
            })
            .expect("cursor should land on a side-by-side row");
        assert_eq!(source.base, Some(26));
        assert_eq!(source.head, Some(27));

        app.apply_core_effects(&EmptyViewport, vec![CoreEffect::ToggleInlineDiff]);

        assert_eq!(app.state.render_variant, RenderVariant::Inline);
        assert_eq!(app.state.diff_line_cursor, 27);
        let source = app
            .state
            .active_document
            .as_ref()
            .and_then(|document| {
                document
                    .document()
                    .diff
                    .source_at(document::RowIndex(app.state.diff_line_cursor))
            })
            .expect("cursor should land on an inline row");
        assert_eq!(source.base, None);
        assert_eq!(source.head, Some(27));
    }

    #[test]
    fn side_by_side_toggle_loads_base_content_for_shared_tail_rows() {
        let path = "src/app/update/comments.rs";
        let mut base_lines: Vec<String> = (1..=90).map(|n| format!("base {n}")).collect();
        let mut head_lines: Vec<String> = (1..=93).map(|n| format!("head {n}")).collect();
        for base_lineno in 27..=90 {
            let head_lineno = base_lineno + 3;
            let content = format!("shared tail {base_lineno}/{head_lineno}");
            base_lines[(base_lineno - 1) as usize] = content.clone();
            head_lines[(head_lineno - 1) as usize] = content;
        }
        let base_content = format!("{}\n", base_lines.join("\n"));
        let head_content = format!("{}\n", head_lines.join("\n"));
        let repo = setup_content_repo(path, &base_content, &head_content);
        let context = ConnectionContext {
            repo_root: repo.path().to_string_lossy().to_string(),
            worktree: repo.path().to_string_lossy().to_string(),
            base_ref: "base".to_string(),
            head_ref: "HEAD".to_string(),
            merge_base: "base".to_string(),
        };
        let mut app = App::new(
            Config::default(),
            context,
            vec![test_file_with_shared_tail_offset_hunk(path)],
        );

        app.state.load_head_content();
        assert!(app.state.head_content.is_some());
        assert!(app.state.base_content.is_none());

        app.apply_core_effects(&EmptyViewport, vec![CoreEffect::ToggleInlineDiff]);

        assert_eq!(app.state.render_variant, RenderVariant::SideBySide);
        assert!(app.state.base_content.is_some());
        let active_document = app.state.active_document.as_ref().expect("active document");
        let DiffDocument::SideBySide(document) = &active_document.document().diff else {
            panic!("expected side-by-side document");
        };
        let first_tail = (0..document.len())
            .map(RowIndex)
            .find(|row| {
                document
                    .source_at(*row)
                    .is_some_and(|source| source.base == Some(27) && source.head == Some(30))
            })
            .expect("base 27 should be paired with head 30");
        assert_eq!(
            document.base().text_at(first_tail),
            Some("shared tail 27/30")
        );
        assert_eq!(
            document.head().text_at(first_tail),
            Some("shared tail 27/30")
        );
    }

    #[test]
    fn line_visual_selection_extends_and_captures_anchor_context() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_hunk("src/main.rs")],
        );
        // 10 leading lines so that the hunk's new_lineno values 11/12 land on the right text.
        // 15-line HEAD so the hunk's new_lineno values 11/12 are correct.
        app.state.head_content = Some(
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n\
             let alpha = beta;\nlet gamma = delta;\nafter one\nafter two\nafter three\n"
                .to_string(),
        );
        // Must match the 15-line head_content above so diff_source_lines aligns.
        let view = RenderedViewport::new(vec![
            "line1",
            "line2",
            "line3",
            "line4",
            "line5",
            "line6",
            "line7",
            "line8",
            "line9",
            "line10",
            "let alpha = beta;",
            "let gamma = delta;",
            "after one",
            "after two",
            "after three",
        ]);
        app.state.pane_focus = PaneFocus::Diff;
        app.state.diff_line_cursor = 10; // first line of the hunk in the HEAD view

        app.apply_core_effects(
            &view,
            vec![
                CoreEffect::VisualSelection(VisualSelectionEffect::StartLine),
                CoreEffect::VisualSelection(VisualSelectionEffect::Move(
                    DiffCursorEffect::LineDown,
                )),
                CoreEffect::VisualSelection(VisualSelectionEffect::Commit),
            ],
        );

        let capture = app
            .state
            .pending_comment_anchor
            .as_ref()
            .expect("selection should capture anchor data");
        assert_eq!(capture.file_path, "src/main.rs");
        assert_eq!(capture.line_start, 11);
        assert_eq!(capture.line_end, 12);
        assert_eq!(capture.char_start, None);
        assert_eq!(capture.char_end, None);
        assert_eq!(capture.anchor_text, "let alpha = beta;\nlet gamma = delta;");
        assert_eq!(capture.context_before, "line8\nline9\nline10");
        assert_eq!(capture.context_after, "after one\nafter two\nafter three");
        assert_eq!(capture.segments.len(), 1);
        let head = segment_for_side(capture, CommentAnchorSide::Head)
            .expect("added-line selection should capture a head segment");
        assert_eq!(head.file_path, "src/main.rs");
        assert_eq!(head.line_start, 11);
        assert_eq!(head.line_end, 12);
        assert_eq!(head.anchor_text, "let alpha = beta;\nlet gamma = delta;");
        assert!(app.state.visual_selection.is_some());
    }

    #[test]
    fn context_line_visual_selection_captures_base_and_head_segments() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_hunk("src/main.rs")],
        );
        app.state.base_content = Some(
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\n\
             before one\nafter one\nafter two\nafter three\n"
                .to_string(),
        );
        app.state.head_content = Some(
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\n\
             before one\nlet alpha = beta;\nlet gamma = delta;\nafter one\n\
             after two\nafter three\n"
                .to_string(),
        );
        app.state.pane_focus = PaneFocus::Diff;
        app.state.diff_line_cursor = 9;

        app.apply_core_effects(
            &EmptyViewport,
            vec![
                CoreEffect::VisualSelection(VisualSelectionEffect::StartLine),
                CoreEffect::VisualSelection(VisualSelectionEffect::Commit),
            ],
        );

        let capture = app
            .state
            .pending_comment_anchor
            .as_ref()
            .expect("context selection should capture anchor data");
        assert_eq!(capture.segments.len(), 2);
        let base = segment_for_side(capture, CommentAnchorSide::Base)
            .expect("context selection should capture a base segment");
        let head = segment_for_side(capture, CommentAnchorSide::Head)
            .expect("context selection should capture a head segment");
        assert_eq!(base.line_start, 10);
        assert_eq!(base.line_end, 10);
        assert_eq!(base.anchor_text, "before one");
        assert_eq!(head.line_start, 10);
        assert_eq!(head.line_end, 10);
        assert_eq!(head.anchor_text, "before one");
        assert_eq!(capture.line_start, 10);
        assert_eq!(capture.line_end, 10);
        assert_eq!(capture.anchor_text, "before one");
    }

    #[test]
    fn lowercase_visual_selection_captures_full_line_anchor() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_hunk("src/main.rs")],
        );
        // 15-line HEAD so the hunk's new_lineno value 11 is correct.
        app.state.head_content = Some(
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n\
             let alpha = beta;\nlet gamma = delta;\nafter one\n"
                .to_string(),
        );
        let view = RenderedViewport::new(vec![
            "line1",
            "line2",
            "line3",
            "line4",
            "line5",
            "line6",
            "line7",
            "line8",
            "line9",
            "line10",
            "let alpha = beta;",
            "let gamma = delta;",
            "after one",
        ]);
        app.state.pane_focus = PaneFocus::Diff;
        app.state.diff_line_cursor = 10; // line of the addition in the HEAD view
        app.state.diff_col_cursor = 4;

        app.apply_core_effects(
            &view,
            vec![
                CoreEffect::VisualSelection(VisualSelectionEffect::StartLine),
                CoreEffect::VisualSelection(VisualSelectionEffect::Move(
                    DiffCursorEffect::CharRight,
                )),
                CoreEffect::VisualSelection(VisualSelectionEffect::Move(
                    DiffCursorEffect::CharRight,
                )),
                CoreEffect::VisualSelection(VisualSelectionEffect::Commit),
            ],
        );

        let capture = app
            .state
            .pending_comment_anchor
            .as_ref()
            .expect("selection should capture anchor data");
        assert_eq!(capture.line_start, 11);
        assert_eq!(capture.line_end, 11);
        assert_eq!(capture.char_start, None);
        assert_eq!(capture.char_end, None);
        assert_eq!(capture.anchor_text, "let alpha = beta;");
    }

    #[test]
    fn text_visual_selection_uses_same_anchor_capture_path() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_hunk("src/main.rs")],
        );
        app.state.head_content = Some(
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n\
             let alpha = beta;\nlet gamma = delta;\nafter one\n"
                .to_string(),
        );
        let view = RenderedViewport::new(vec![
            "line1",
            "line2",
            "line3",
            "line4",
            "line5",
            "line6",
            "line7",
            "line8",
            "line9",
            "line10",
            "let alpha = beta;",
            "let gamma = delta;",
            "after one",
        ]);

        app.apply_core_effects(
            &view,
            vec![
                CoreEffect::VisualSelection(VisualSelectionEffect::StartText {
                    anchor: TextAnchor {
                        line: 10,
                        column: 4,
                    },
                }),
                CoreEffect::VisualSelection(VisualSelectionEffect::ExtendTo {
                    anchor: TextAnchor {
                        line: 11,
                        column: 8,
                    },
                }),
                CoreEffect::VisualSelection(VisualSelectionEffect::Commit),
            ],
        );

        let selection = app
            .state
            .visual_selection
            .as_ref()
            .expect("text selection should remain visible");
        assert_eq!(selection.mode, VisualSelectionMode::Text);

        let capture = app
            .state
            .pending_comment_anchor
            .as_ref()
            .expect("selection should capture anchor data");
        assert_eq!(capture.line_start, 11);
        assert_eq!(capture.line_end, 12);
        assert_eq!(capture.char_start, None);
        assert_eq!(capture.char_end, None);
        assert_eq!(capture.anchor_text, "let alpha = beta;\nlet gamma = delta;");
    }

    #[test]
    fn deleted_line_visual_selection_captures_base_only_segment() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_leading_deletion_hunk("src/main.rs")],
        );
        app.state.base_content = Some(
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n\
             line11\nline12\nline13\nline14\nline15\nremoved\nkept\n"
                .to_string(),
        );
        app.state.head_content = Some(
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n\
             line11\nline12\nline13\nline14\nline15\nkept\n"
                .to_string(),
        );
        app.state.pane_focus = PaneFocus::Diff;
        app.state.diff_line_cursor = 15;

        app.apply_core_effects(
            &EmptyViewport,
            vec![
                CoreEffect::VisualSelection(VisualSelectionEffect::StartLine),
                CoreEffect::VisualSelection(VisualSelectionEffect::Commit),
            ],
        );

        let capture = app
            .state
            .pending_comment_anchor
            .as_ref()
            .expect("deleted-line selection should capture anchor data");
        assert_eq!(capture.segments.len(), 1);
        assert!(segment_for_side(capture, CommentAnchorSide::Head).is_none());
        let base = segment_for_side(capture, CommentAnchorSide::Base)
            .expect("deleted-line selection should capture a base segment");
        assert_eq!(base.file_path, "src/main.rs");
        assert_eq!(base.line_start, 16);
        assert_eq!(base.line_end, 16);
        assert_eq!(base.anchor_text, "removed");
        assert_eq!(capture.file_path, "src/main.rs");
        assert_eq!(capture.line_start, 16);
        assert_eq!(capture.line_end, 16);
        assert_eq!(capture.anchor_text, "removed");
    }

    #[test]
    fn replacement_visual_selection_captures_base_and_head_segments() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_replacement_hunk("src/main.rs")],
        );
        app.state.base_content = Some(
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n\
             line11\nline12\nline13\nline14\nbefore\nold\nafter\n"
                .to_string(),
        );
        app.state.head_content = Some(
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n\
             line11\nline12\nline13\nline14\nbefore\nnew\nafter\n"
                .to_string(),
        );
        app.state.pane_focus = PaneFocus::Diff;

        app.apply_core_effects(
            &EmptyViewport,
            vec![
                CoreEffect::VisualSelection(VisualSelectionEffect::StartText {
                    anchor: TextAnchor {
                        line: 15,
                        column: 0,
                    },
                }),
                CoreEffect::VisualSelection(VisualSelectionEffect::ExtendTo {
                    anchor: TextAnchor {
                        line: 16,
                        column: 0,
                    },
                }),
                CoreEffect::VisualSelection(VisualSelectionEffect::Commit),
            ],
        );

        let capture = app
            .state
            .pending_comment_anchor
            .as_ref()
            .expect("replacement selection should capture anchor data");
        assert_eq!(capture.segments.len(), 2);
        let base = segment_for_side(capture, CommentAnchorSide::Base)
            .expect("replacement selection should capture a base segment");
        let head = segment_for_side(capture, CommentAnchorSide::Head)
            .expect("replacement selection should capture a head segment");
        assert_eq!(base.line_start, 16);
        assert_eq!(base.line_end, 16);
        assert_eq!(base.anchor_text, "old");
        assert_eq!(head.line_start, 16);
        assert_eq!(head.line_end, 16);
        assert_eq!(head.anchor_text, "new");
        assert_eq!(capture.file_path, "src/main.rs");
        assert_eq!(capture.line_start, 16);
        assert_eq!(capture.line_end, 16);
        assert_eq!(capture.anchor_text, "old\nnew");
    }

    #[test]
    fn side_by_side_replacement_selection_captures_base_and_head_segments() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_replacement_hunk("src/main.rs")],
        );
        app.state.render_variant = RenderVariant::SideBySide;
        app.state.base_content = Some(
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n\
             line11\nline12\nline13\nline14\nbefore\nold\nafter\n"
                .to_string(),
        );
        app.state.head_content = Some(
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n\
             line11\nline12\nline13\nline14\nbefore\nnew\nafter\n"
                .to_string(),
        );
        app.state.pane_focus = PaneFocus::Diff;

        app.apply_core_effects(
            &EmptyViewport,
            vec![
                CoreEffect::VisualSelection(VisualSelectionEffect::StartLine),
                CoreEffect::VisualSelection(VisualSelectionEffect::Move(
                    DiffCursorEffect::LineDown,
                )),
                CoreEffect::VisualSelection(VisualSelectionEffect::Commit),
            ],
        );

        let capture = app
            .state
            .pending_comment_anchor
            .as_ref()
            .expect("side-by-side replacement selection should capture anchor data");
        assert_eq!(capture.segments.len(), 2);
        let base = segment_for_side(capture, CommentAnchorSide::Base)
            .expect("side-by-side selection should capture a base segment");
        let head = segment_for_side(capture, CommentAnchorSide::Head)
            .expect("side-by-side selection should capture a head segment");
        assert_eq!(base.line_start, 16);
        assert_eq!(base.line_end, 16);
        assert_eq!(base.anchor_text, "old");
        assert_eq!(head.line_start, 16);
        assert_eq!(head.line_end, 16);
        assert_eq!(head.anchor_text, "new");
    }

    #[test]
    fn side_by_side_visual_selection_uses_render_row_mapping_for_offset_replacement() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_offset_multiline_replacement_hunk(
                "src/main.rs",
            )],
        );
        app.state.render_variant = RenderVariant::SideBySide;
        let mut base_lines: Vec<String> = (1..=32).map(|n| format!("base {n}")).collect();
        let mut head_lines: Vec<String> = (1..=34).map(|n| format!("head {n}")).collect();
        base_lines[25] = "old line".to_string();
        head_lines[26] = "new line one".to_string();
        head_lines[27] = "new line two".to_string();
        app.state.base_content = Some(format!("{}\n", base_lines.join("\n")));
        app.state.head_content = Some(format!("{}\n", head_lines.join("\n")));
        app.state.pane_focus = PaneFocus::Diff;
        app.state.diff_line_cursor = 26;

        app.apply_core_effects(
            &EmptyViewport,
            vec![
                CoreEffect::VisualSelection(VisualSelectionEffect::StartLine),
                CoreEffect::VisualSelection(VisualSelectionEffect::Move(
                    DiffCursorEffect::LineDown,
                )),
                CoreEffect::VisualSelection(VisualSelectionEffect::Commit),
            ],
        );

        let capture = app
            .state
            .pending_comment_anchor
            .as_ref()
            .expect("side-by-side offset replacement should capture anchor data");
        assert_eq!(capture.segments.len(), 2);
        let base = segment_for_side(capture, CommentAnchorSide::Base)
            .expect("side-by-side selection should capture a base segment");
        let head = segment_for_side(capture, CommentAnchorSide::Head)
            .expect("side-by-side selection should capture a head segment");
        assert_eq!(base.line_start, 26);
        assert_eq!(base.line_end, 26);
        assert_eq!(base.anchor_text, "old line");
        assert_eq!(head.line_start, 27);
        assert_eq!(head.line_end, 28);
        assert_eq!(head.anchor_text, "new line one\nnew line two");
        assert_eq!(capture.line_start, 26);
        assert_eq!(capture.line_end, 28);
    }

    #[test]
    fn side_by_side_visual_selection_captures_shared_tail_base_and_head_segments() {
        let path = "src/app/update/comments.rs";
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_shared_tail_offset_hunk(path)],
        );
        app.state.render_variant = RenderVariant::SideBySide;
        let mut base_lines: Vec<String> = (1..=90).map(|n| format!("base {n}")).collect();
        let mut head_lines: Vec<String> = (1..=93).map(|n| format!("head {n}")).collect();
        for base_lineno in 27..=90 {
            let head_lineno = base_lineno + 3;
            let content = format!("shared tail {base_lineno}/{head_lineno}");
            base_lines[(base_lineno - 1) as usize] = content.clone();
            head_lines[(head_lineno - 1) as usize] = content;
        }
        app.state.base_content = Some(format!("{}\n", base_lines.join("\n")));
        app.state.head_content = Some(format!("{}\n", head_lines.join("\n")));
        app.state.pane_focus = PaneFocus::Diff;
        app.state.diff_line_cursor = 29;

        app.apply_core_effects(
            &EmptyViewport,
            vec![
                CoreEffect::VisualSelection(VisualSelectionEffect::StartLine),
                CoreEffect::VisualSelection(VisualSelectionEffect::Commit),
            ],
        );

        let capture = app
            .state
            .pending_comment_anchor
            .as_ref()
            .expect("side-by-side shared tail should capture anchor data");
        assert_eq!(capture.segments.len(), 2);
        let base = segment_for_side(capture, CommentAnchorSide::Base)
            .expect("shared tail selection should capture a base segment");
        let head = segment_for_side(capture, CommentAnchorSide::Head)
            .expect("shared tail selection should capture a head segment");
        assert_eq!(base.line_start, 27);
        assert_eq!(base.line_end, 27);
        assert_eq!(base.anchor_text, "shared tail 27/30");
        assert_eq!(head.line_start, 30);
        assert_eq!(head.line_end, 30);
        assert_eq!(head.anchor_text, "shared tail 27/30");
    }

    #[test]
    fn side_by_side_context_selection_uses_diff_base_entry_without_base_content() {
        let hunk = DiffHunk {
            old_start: 163,
            old_lines: 10,
            new_start: 182,
            new_lines: 10,
            header: "@@ -163,10 +182,10 @@".to_string(),
            lines: vec![
                DiffLine {
                    kind: LineKind::Context,
                    content: "fn toggle_resolved_current(state: &mut AppState) {".to_string(),
                    old_lineno: Some(163),
                    new_lineno: Some(182),
                },
                DiffLine {
                    kind: LineKind::Context,
                    content: "    let Some(comment) = current_comment(state).cloned() else {"
                        .to_string(),
                    old_lineno: Some(164),
                    new_lineno: Some(183),
                },
            ],
        };
        let path = "src/app/update/comments.rs";
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![FileEntry {
                change: FileChange {
                    path: path.to_string(),
                    old_path: None,
                    kind: ChangeKind::Modified,
                },
                status: ReviewStatus::Unreviewed,
                diff: DiffContent {
                    hunks: vec![hunk],
                    is_binary: false,
                    diff_hash: "hash-context".to_string(),
                    content_id: String::new(),
                },
            }],
        );
        app.state.render_variant = RenderVariant::SideBySide;
        app.state.head_content = Some(
            (1..=183)
                .map(|n| format!("head {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        app.state.base_content = None;
        app.state.pane_focus = PaneFocus::Diff;
        app.state.diff_line_cursor = 181;

        app.apply_core_effects(
            &EmptyViewport,
            vec![
                CoreEffect::VisualSelection(VisualSelectionEffect::StartLine),
                CoreEffect::VisualSelection(VisualSelectionEffect::Move(
                    DiffCursorEffect::LineDown,
                )),
                CoreEffect::VisualSelection(VisualSelectionEffect::Commit),
            ],
        );

        let capture = app
            .state
            .pending_comment_anchor
            .as_ref()
            .expect("side-by-side context should capture anchor data");
        let base = segment_for_side(capture, CommentAnchorSide::Base)
            .expect("context selection should capture a base segment from diff rows");
        let head = segment_for_side(capture, CommentAnchorSide::Head)
            .expect("context selection should capture a head segment");
        assert_eq!(base.line_start, 163);
        assert_eq!(base.line_end, 164);
        assert_eq!(head.line_start, 182);
        assert_eq!(head.line_end, 183);
    }

    #[test]
    fn inline_visual_selection_captures_offset_replacement_sides() {
        let path = "migrations/0005_comment_resolution_events.up.sql";
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_migration_offset_replacement_hunk(path)],
        );
        let mut base_lines: Vec<String> = (1..=34).map(|n| format!("base {n}")).collect();
        let mut head_lines: Vec<String> = (1..=36).map(|n| format!("head {n}")).collect();
        base_lines[28] = "a.context_before, a.context_after, a.status".to_string();
        head_lines[30] = "a.context_before, a.context_after, a.status, ''".to_string();
        app.state.base_content = Some(format!("{}\n", base_lines.join("\n")));
        app.state.head_content = Some(format!("{}\n", head_lines.join("\n")));
        app.state.pane_focus = PaneFocus::Diff;

        app.apply_core_effects(
            &EmptyViewport,
            vec![
                CoreEffect::VisualSelection(VisualSelectionEffect::StartText {
                    anchor: TextAnchor {
                        line: 30,
                        column: 0,
                    },
                }),
                CoreEffect::VisualSelection(VisualSelectionEffect::ExtendTo {
                    anchor: TextAnchor {
                        line: 31,
                        column: 0,
                    },
                }),
                CoreEffect::VisualSelection(VisualSelectionEffect::Commit),
            ],
        );

        let capture = app
            .state
            .pending_comment_anchor
            .as_ref()
            .expect("offset replacement selection should capture anchor data");
        assert_eq!(capture.file_path, path);
        assert_eq!(capture.segments.len(), 2);
        let base = segment_for_side(capture, CommentAnchorSide::Base)
            .expect("replacement selection should capture a base segment");
        let head = segment_for_side(capture, CommentAnchorSide::Head)
            .expect("replacement selection should capture a head segment");
        assert_eq!(base.line_start, 29);
        assert_eq!(base.line_end, 29);
        assert_eq!(head.line_start, 31);
        assert_eq!(head.line_end, 31);
        assert_eq!(capture.line_start, 29);
        assert_eq!(capture.line_end, 31);
    }

    #[test]
    fn inline_visual_selection_uses_render_rows_for_long_offset_hunk() {
        let path = "src/app/update/comments.rs";
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_long_offset_replacement_hunk(path)],
        );
        let mut base_lines: Vec<String> = (1..=60).map(|n| format!("base {n}")).collect();
        let mut head_lines: Vec<String> = (1..=62).map(|n| format!("head {n}")).collect();
        base_lines[17] = "let line = current_head_line(state)?;".to_string();
        head_lines[19] = "let line = current_visible_line(state)?;".to_string();
        app.state.base_content = Some(format!("{}\n", base_lines.join("\n")));
        app.state.head_content = Some(format!("{}\n", head_lines.join("\n")));
        app.state.pane_focus = PaneFocus::Diff;

        app.apply_core_effects(
            &EmptyViewport,
            vec![
                CoreEffect::VisualSelection(VisualSelectionEffect::StartText {
                    anchor: TextAnchor {
                        line: 19,
                        column: 0,
                    },
                }),
                CoreEffect::VisualSelection(VisualSelectionEffect::ExtendTo {
                    anchor: TextAnchor {
                        line: 20,
                        column: 0,
                    },
                }),
                CoreEffect::VisualSelection(VisualSelectionEffect::Commit),
            ],
        );

        let capture = app
            .state
            .pending_comment_anchor
            .as_ref()
            .expect("offset replacement selection should capture anchor data");
        assert_eq!(capture.file_path, path);
        assert_eq!(capture.segments.len(), 2);
        let base = segment_for_side(capture, CommentAnchorSide::Base)
            .expect("replacement selection should capture a base segment");
        let head = segment_for_side(capture, CommentAnchorSide::Head)
            .expect("replacement selection should capture a head segment");
        assert_eq!(base.line_start, 18);
        assert_eq!(base.line_end, 18);
        assert_eq!(base.anchor_text, "let line = current_head_line(state)?;");
        assert_eq!(head.line_start, 20);
        assert_eq!(head.line_end, 20);
        assert_eq!(head.anchor_text, "let line = current_visible_line(state)?;");
    }

    #[test]
    fn inline_visual_selection_preserves_context_to_later_head_line() {
        let path = "src/app/update/comments.rs";
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_deletion_to_later_head_context_hunk(path)],
        );
        app.state.base_content = Some(
            (1..=167)
                .map(|n| format!("base {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        app.state.head_content = Some(
            (1..=188)
                .map(|n| format!("head {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        app.state.content_mode = ContentMode::Diff;
        app.state.render_variant = RenderVariant::Inline;
        app.state.pane_focus = PaneFocus::Diff;
        app.state.rebuild_active_document();
        let active_document = app.state.active_document.as_ref().expect("active document");
        let start_row = active_document
            .document()
            .diff
            .row_for_source_line(CommentAnchorSide::Base, 154)
            .expect("base 154 should render")
            .0;
        let end_row = active_document
            .document()
            .diff
            .row_for_source_line(CommentAnchorSide::Head, 188)
            .expect("head 188 should render")
            .0;

        app.apply_core_effects(
            &EmptyViewport,
            vec![
                CoreEffect::VisualSelection(VisualSelectionEffect::StartText {
                    anchor: TextAnchor {
                        line: start_row,
                        column: 0,
                    },
                }),
                CoreEffect::VisualSelection(VisualSelectionEffect::ExtendTo {
                    anchor: TextAnchor {
                        line: end_row,
                        column: 0,
                    },
                }),
                CoreEffect::VisualSelection(VisualSelectionEffect::Commit),
            ],
        );

        let capture = app
            .state
            .pending_comment_anchor
            .as_ref()
            .expect("mixed inline selection should capture anchor data");
        assert_eq!(capture.file_path, path);
        assert_eq!(capture.segments.len(), 2);
        let base = segment_for_side(capture, CommentAnchorSide::Base)
            .expect("selection should capture a base segment");
        let head = segment_for_side(capture, CommentAnchorSide::Head)
            .expect("selection should capture a head segment");
        assert_eq!(base.line_start, 154);
        assert_eq!(base.line_end, 167);
        assert_eq!(head.line_start, 172);
        assert_eq!(head.line_end, 188);
        assert_eq!(capture.line_start, 154);
        assert_eq!(capture.line_end, 188);
    }

    #[test]
    fn visual_selection_across_hunks_captures_all_side_segments() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_two_hunks("src/main.rs")],
        );
        app.state.base_content = Some(
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\n\
             old first\nline11\nline12\nline13\nline14\nline15\nline16\nline17\n\
             line18\nline19\nold second\n"
                .to_string(),
        );
        app.state.head_content = Some(
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\n\
             new first\nline11\nline12\nline13\nline14\nline15\nline16\nline17\n\
             line18\nline19\nnew second\n"
                .to_string(),
        );
        app.state.pane_focus = PaneFocus::Diff;

        app.apply_core_effects(
            &EmptyViewport,
            vec![
                CoreEffect::VisualSelection(VisualSelectionEffect::StartText {
                    anchor: TextAnchor { line: 9, column: 0 },
                }),
                CoreEffect::VisualSelection(VisualSelectionEffect::ExtendTo {
                    anchor: TextAnchor {
                        line: 21,
                        column: 0,
                    },
                }),
                CoreEffect::VisualSelection(VisualSelectionEffect::Commit),
            ],
        );

        let capture = app
            .state
            .pending_comment_anchor
            .as_ref()
            .expect("cross-hunk selection should capture anchor data");
        assert_eq!(capture.segments.len(), 2);
        let base_segments: Vec<&CommentAnchorSegment> = capture
            .segments
            .iter()
            .filter(|segment| segment.side == CommentAnchorSide::Base)
            .collect();
        let head_segments: Vec<&CommentAnchorSegment> = capture
            .segments
            .iter()
            .filter(|segment| segment.side == CommentAnchorSide::Head)
            .collect();
        assert_eq!(base_segments.len(), 1);
        assert_eq!(head_segments.len(), 1);
        assert_eq!(base_segments[0].line_start, 10);
        assert_eq!(base_segments[0].line_end, 20);
        assert_eq!(
            base_segments[0].anchor_text,
            "old first\nline11\nline12\nline13\nline14\nline15\nline16\nline17\nline18\nline19\nold second"
        );
        assert_eq!(head_segments[0].line_start, 10);
        assert_eq!(head_segments[0].line_end, 20);
        assert_eq!(
            head_segments[0].anchor_text,
            "new first\nline11\nline12\nline13\nline14\nline15\nline16\nline17\nline18\nline19\nnew second"
        );
        assert_eq!(capture.file_path, "src/main.rs");
        assert_eq!(capture.line_start, 10);
        assert_eq!(capture.line_end, 20);
    }

    #[test]
    fn full_file_base_visual_selection_captures_base_segment() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("src/main.rs")],
        );
        app.state.content_mode = ContentMode::FullFile;
        app.state.render_variant = RenderVariant::BaseVersion;
        app.state.base_content = Some("base one\nbase two\nbase three\n".to_string());
        app.state.head_content = Some("head one\nhead two\nhead three\n".to_string());
        app.state.pane_focus = PaneFocus::Diff;
        app.state.diff_line_cursor = 1;

        app.apply_core_effects(
            &EmptyViewport,
            vec![
                CoreEffect::VisualSelection(VisualSelectionEffect::StartLine),
                CoreEffect::VisualSelection(VisualSelectionEffect::Commit),
            ],
        );

        let capture = app
            .state
            .pending_comment_anchor
            .as_ref()
            .expect("full-file base selection should capture anchor data");
        assert_eq!(capture.segments.len(), 1);
        assert!(segment_for_side(capture, CommentAnchorSide::Head).is_none());
        let base = segment_for_side(capture, CommentAnchorSide::Base)
            .expect("full-file base selection should capture a base segment");
        assert_eq!(base.line_start, 2);
        assert_eq!(base.line_end, 2);
        assert_eq!(base.anchor_text, "base two");
        assert_eq!(capture.anchor_text, "base two");
    }

    #[test]
    fn full_file_head_visual_selection_captures_head_segment() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("src/main.rs")],
        );
        app.state.content_mode = ContentMode::FullFile;
        app.state.render_variant = RenderVariant::HeadVersion;
        app.state.base_content = Some("base one\nbase two\nbase three\n".to_string());
        app.state.head_content = Some("head one\nhead two\nhead three\n".to_string());
        app.state.pane_focus = PaneFocus::Diff;
        app.state.diff_line_cursor = 1;

        app.apply_core_effects(
            &EmptyViewport,
            vec![
                CoreEffect::VisualSelection(VisualSelectionEffect::StartLine),
                CoreEffect::VisualSelection(VisualSelectionEffect::Commit),
            ],
        );

        let capture = app
            .state
            .pending_comment_anchor
            .as_ref()
            .expect("full-file head selection should capture anchor data");
        assert_eq!(capture.segments.len(), 1);
        assert!(segment_for_side(capture, CommentAnchorSide::Base).is_none());
        let head = segment_for_side(capture, CommentAnchorSide::Head)
            .expect("full-file head selection should capture a head segment");
        assert_eq!(head.line_start, 2);
        assert_eq!(head.line_end, 2);
        assert_eq!(head.anchor_text, "head two");
        assert_eq!(capture.anchor_text, "head two");
    }

    #[test]
    fn hidden_comment_navigation_keeps_render_mode_and_reports_status() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("src/main.rs")],
        );
        let mut comment = stored_comment(8, "src/main.rs");
        move_comment_head_range(&mut comment, 2, 2);
        app.state.comments = vec![comment];
        app.state.content_mode = ContentMode::FullFile;
        app.state.render_variant = RenderVariant::BaseVersion;
        app.state.base_content = Some("base one\nbase two\n".to_string());
        app.state.head_content = Some("head one\nhead two\n".to_string());

        let output = app.apply_core_effects(
            &RenderedViewport::new(vec!["base one", "base two"]),
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Next)],
        );

        assert_eq!(app.state.content_mode, ContentMode::FullFile);
        assert_eq!(app.state.render_variant, RenderVariant::BaseVersion);
        assert_eq!(app.state.selected_comment_id, Some(8));
        assert_eq!(
            output.status,
            Some(StatusUpdate::Set(
                "Comment is not visible in this view".to_string()
            ))
        );
    }

    #[test]
    fn full_file_base_navigation_uses_visible_base_segment() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("src/main.rs")],
        );
        let mut comment = stored_comment(8, "src/main.rs");
        comment
            .replace_anchor(crate::review_types::CommentAnchor {
                segments: vec![
                    CommentAnchorSegment {
                        side: CommentAnchorSide::Base,
                        file_path: "src/main.rs".to_string(),
                        line_start: 2,
                        line_end: 2,
                        char_start: None,
                        char_end: None,
                        anchor_text: "base two".to_string(),
                        context_before: "base one".to_string(),
                        context_after: "base three".to_string(),
                        placement_status: AnchorPlacementStatus::Anchored,
                        match_method: AnchorMatchMethod::ExactAtLine,
                    },
                    CommentAnchorSegment {
                        side: CommentAnchorSide::Head,
                        file_path: "src/main.rs".to_string(),
                        line_start: 10,
                        line_end: 10,
                        char_start: None,
                        char_end: None,
                        anchor_text: "head ten".to_string(),
                        context_before: "head nine".to_string(),
                        context_after: "head eleven".to_string(),
                        placement_status: AnchorPlacementStatus::Anchored,
                        match_method: AnchorMatchMethod::ExactAtLine,
                    },
                ],
                aggregate_status: crate::review_types::AnchorAggregateStatus::Anchored,
            })
            .expect("test comment anchor should be valid");
        app.state.comments = vec![comment];
        app.state.content_mode = ContentMode::FullFile;
        app.state.render_variant = RenderVariant::BaseVersion;
        app.state.base_content = Some("base one\nbase two\nbase three\n".to_string());
        app.state.head_content = Some(
            (1..=12)
                .map(|n| format!("head {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );

        app.apply_core_effects(
            &RenderedViewport::new(vec!["base one", "base two", "base three"]),
            vec![CoreEffect::CommentsPanel(
                CommentsPanelEffect::NavigateNextComment,
            )],
        );

        assert_eq!(app.state.selected_comment_id, Some(8));
        assert_eq!(app.state.diff_line_cursor, 1);
    }

    #[test]
    fn full_file_head_navigation_uses_visible_head_segment() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("src/main.rs")],
        );
        let mut comment = stored_comment(8, "src/main.rs");
        comment
            .replace_anchor(crate::review_types::CommentAnchor {
                segments: vec![
                    CommentAnchorSegment {
                        side: CommentAnchorSide::Base,
                        file_path: "src/main.rs".to_string(),
                        line_start: 2,
                        line_end: 2,
                        char_start: None,
                        char_end: None,
                        anchor_text: "base two".to_string(),
                        context_before: "base one".to_string(),
                        context_after: "base three".to_string(),
                        placement_status: AnchorPlacementStatus::Anchored,
                        match_method: AnchorMatchMethod::ExactAtLine,
                    },
                    CommentAnchorSegment {
                        side: CommentAnchorSide::Head,
                        file_path: "src/main.rs".to_string(),
                        line_start: 10,
                        line_end: 10,
                        char_start: None,
                        char_end: None,
                        anchor_text: "head ten".to_string(),
                        context_before: "head nine".to_string(),
                        context_after: "head eleven".to_string(),
                        placement_status: AnchorPlacementStatus::Anchored,
                        match_method: AnchorMatchMethod::ExactAtLine,
                    },
                ],
                aggregate_status: crate::review_types::AnchorAggregateStatus::Anchored,
            })
            .expect("test comment anchor should be valid");
        app.state.comments = vec![comment];
        app.state.content_mode = ContentMode::FullFile;
        app.state.render_variant = RenderVariant::HeadVersion;
        app.state.base_content = Some("base one\nbase two\nbase three\n".to_string());
        app.state.head_content = Some(
            (1..=12)
                .map(|n| format!("head {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );

        app.apply_core_effects(
            &RenderedViewport::new(
                (1..=12)
                    .map(|n| format!("head {n}"))
                    .collect::<Vec<_>>()
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            ),
            vec![CoreEffect::CommentsPanel(
                CommentsPanelEffect::NavigateNextComment,
            )],
        );

        assert_eq!(app.state.selected_comment_id, Some(8));
        assert_eq!(app.state.diff_line_cursor, 9);
    }

    #[test]
    fn visual_selection_cancel_clears_without_capture() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_hunk("src/main.rs")],
        );

        app.apply_core_effects(
            &EmptyViewport,
            vec![
                CoreEffect::VisualSelection(VisualSelectionEffect::StartLine),
                CoreEffect::VisualSelection(VisualSelectionEffect::Cancel),
            ],
        );

        assert!(app.state.visual_selection.is_none());
        assert!(app.state.pending_comment_anchor.is_none());
    }

    #[test]
    fn comment_body_submit_queues_create_work() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("src/main.rs")],
        );
        let anchor = CommentAnchorCapture {
            segments: test_anchor("src/main.rs", 1, 1, "fn main() {}").segments,
            file_path: "src/main.rs".to_string(),
            line_start: 1,
            line_end: 1,
            char_start: None,
            char_end: None,
            anchor_text: "fn main() {}".to_string(),
            context_before: String::new(),
            context_after: String::new(),
        };
        app.state.pending_comment_anchor = Some(anchor.clone());

        let output = app.apply_core_effects(
            &EmptyViewport,
            vec![CoreEffect::Comment(CommentEffect::SubmitBody {
                body: "  please fix  ".to_string(),
            })],
        );

        assert_eq!(
            output.status,
            Some(StatusUpdate::Set("Creating comment...".to_string()))
        );
        assert_eq!(
            app.pending_work.pop_front(),
            Some(AppWork::CreateComment {
                anchor,
                body: "  please fix".to_string(),
            })
        );
    }

    #[test]
    fn comment_body_submit_strips_trailing_whitespace_only() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("src/main.rs")],
        );
        let anchor = CommentAnchorCapture {
            segments: test_anchor("src/main.rs", 1, 1, "fn main() {}").segments,
            file_path: "src/main.rs".to_string(),
            line_start: 1,
            line_end: 1,
            char_start: None,
            char_end: None,
            anchor_text: "fn main() {}".to_string(),
            context_before: String::new(),
            context_after: String::new(),
        };
        app.state.pending_comment_anchor = Some(anchor.clone());

        app.apply_core_effects(
            &EmptyViewport,
            vec![CoreEffect::Comment(CommentEffect::SubmitBody {
                body: "  please fix  \n\t ".to_string(),
            })],
        );

        assert_eq!(
            app.pending_work.pop_front(),
            Some(AppWork::CreateComment {
                anchor,
                body: "  please fix".to_string(),
            })
        );
    }

    #[test]
    fn comment_create_params_preserve_compound_capture_segments() {
        let anchor = CommentAnchorCapture {
            segments: vec![
                CommentAnchorSegment {
                    side: CommentAnchorSide::Base,
                    file_path: "src/main.rs".to_string(),
                    line_start: 8,
                    line_end: 8,
                    char_start: None,
                    char_end: None,
                    anchor_text: "old".to_string(),
                    context_before: "before".to_string(),
                    context_after: "after".to_string(),
                    placement_status: AnchorPlacementStatus::Anchored,
                    match_method: AnchorMatchMethod::ExactAtLine,
                },
                CommentAnchorSegment {
                    side: CommentAnchorSide::Head,
                    file_path: "src/main.rs".to_string(),
                    line_start: 8,
                    line_end: 9,
                    char_start: None,
                    char_end: None,
                    anchor_text: "new\nlines".to_string(),
                    context_before: "before".to_string(),
                    context_after: "after".to_string(),
                    placement_status: AnchorPlacementStatus::Anchored,
                    match_method: AnchorMatchMethod::ExactAtLine,
                },
            ],
            file_path: "src/main.rs".to_string(),
            line_start: 8,
            line_end: 9,
            char_start: None,
            char_end: None,
            anchor_text: "old\nnew\nlines".to_string(),
            context_before: "before".to_string(),
            context_after: "after".to_string(),
        };

        let params = create_comment_params_from_capture(anchor, "body".to_string())
            .expect("compound capture should convert to create params");

        assert_eq!(params.body, "body");
        assert_eq!(
            params.anchor.aggregate_status,
            crate::review_types::AnchorAggregateStatus::Anchored
        );
        assert_eq!(params.anchor.segments.len(), 2);
        assert_eq!(params.anchor.segments[0].side, CommentAnchorSide::Base);
        assert_eq!(params.anchor.segments[0].anchor_text, "old");
        assert_eq!(params.anchor.segments[1].side, CommentAnchorSide::Head);
        assert_eq!(params.anchor.segments[1].anchor_text, "new\nlines");
    }

    #[test]
    fn comment_edit_submit_queues_update_work() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("src/main.rs")],
        );

        let output = app.apply_core_effects(
            &EmptyViewport,
            vec![CoreEffect::Comment(CommentEffect::SubmitEditBody {
                id: 42,
                body: "updated  \n  ".to_string(),
            })],
        );

        assert_eq!(
            output.status,
            Some(StatusUpdate::Set("Updating comment #42...".to_string()))
        );
        assert_eq!(
            app.pending_work.pop_front(),
            Some(AppWork::UpdateComment {
                id: 42,
                body: "updated".to_string(),
            })
        );
    }

    #[test]
    fn comments_panel_toggle_does_not_select_first_file_comment() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("src/main.rs")],
        );
        app.state.comments = vec![stored_comment(7, "src/main.rs")];

        app.apply_core_effects(
            &EmptyViewport,
            vec![CoreEffect::CommentsPanel(CommentsPanelEffect::Toggle)],
        );

        assert!(app.state.show_comments_panel);
        assert_eq!(app.state.selected_comment_id, None);
    }

    #[test]
    fn comments_panel_model_shows_only_cursor_comment() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("src/main.rs")],
        );
        let mut first = stored_comment(7, "src/main.rs");
        move_comment_head_range(&mut first, 2, 2);
        let mut second = stored_comment(8, "src/main.rs");
        move_comment_head_range(&mut second, 3, 3);
        app.state.comments = vec![first, second];
        app.state.head_content = Some("one\ntwo\nthree\n".to_string());
        app.state.show_comments_panel = true;
        app.state.diff_line_cursor = 1;
        app.state.ensure_active_document();

        let model = app.model();

        assert_eq!(model.comments_panel.comments.len(), 1);
        assert_eq!(model.comments_panel.comments[0].id, 7);
        assert_eq!(model.comments_panel.selected_index, Some(1));
        assert_eq!(model.comments_panel.total, 2);

        app.state.diff_line_cursor = 0;
        app.state.ensure_active_document();
        let model = app.model();

        assert!(model.comments_panel.comments.is_empty());
        assert_eq!(model.comments_panel.selected_index, None);
        assert_eq!(model.comments_panel.total, 2);
    }

    #[test]
    fn comments_panel_current_lookup_uses_active_document_overlay() {
        let path = "src/main.rs";
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_deletion_to_later_head_context_hunk(path)],
        );
        app.state.content_mode = ContentMode::Diff;
        app.state.render_variant = RenderVariant::Inline;
        app.state.head_content = Some(
            (1..=188)
                .map(|n| format!("head {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let mut broad = stored_comment(33, path);
        move_comment_to_base_head_ranges(&mut broad, 154, 167, 172, 188);
        let mut nested = stored_comment(34, path);
        move_comment_head_range(&mut nested, 172, 175);
        app.state.comments = vec![broad, nested];
        app.state.ensure_active_document();
        let head_172_row = app
            .state
            .active_document
            .as_ref()
            .and_then(|active| match &active.document().diff {
                document::DiffDocument::Unified(document) => document
                    .rows()
                    .iter()
                    .position(|row| row_has_head_line(row, 172)),
                _ => None,
            })
            .expect("head 172 should render");
        app.state.diff_line_cursor = head_172_row;
        app.state.selected_comment_id = Some(33);
        app.state.mark_model_changed();

        let model = app.model();

        assert_eq!(model.comments_panel.comments.len(), 1);
        assert_eq!(model.comments_panel.comments[0].id, 33);
        assert!(model.comments_panel.comments[0].current);
    }

    #[test]
    fn comment_navigation_is_relative_to_cursor_line() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("src/main.rs")],
        );
        let mut first = stored_comment(7, "src/main.rs");
        move_comment_head_range(&mut first, 2, 2);
        let mut second = stored_comment(8, "src/main.rs");
        move_comment_head_range(&mut second, 4, 4);
        app.state.comments = vec![first, second];
        app.state.head_content = Some("one\ntwo\nthree\nfour\nfive\n".to_string());
        app.state.diff_line_cursor = 2;
        assert!(!app.state.show_comments_panel);
        let view = RenderedViewport::new(vec!["one", "two", "three", "four", "five"]);

        app.apply_core_effects(
            &view,
            vec![CoreEffect::CommentsPanel(
                CommentsPanelEffect::NavigateNextComment,
            )],
        );

        assert_eq!(app.state.selected_comment_id, Some(8));
        assert_eq!(app.state.diff_line_cursor, 3);
        assert!(app.state.show_comments_panel);

        app.state.show_comments_panel = false;
        app.state.diff_line_cursor = 2;
        app.apply_core_effects(
            &view,
            vec![CoreEffect::CommentsPanel(
                CommentsPanelEffect::NavigatePreviousComment,
            )],
        );

        assert_eq!(app.state.selected_comment_id, Some(7));
        assert_eq!(app.state.diff_line_cursor, 1);
        assert!(app.state.show_comments_panel);
    }

    #[test]
    fn comment_navigation_places_fitting_comment_end_above_bottom() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("src/main.rs")],
        );
        let mut comment = stored_comment(7, "src/main.rs");
        move_comment_head_range(&mut comment, 18, 20);
        app.state.comments = vec![comment];
        app.state.head_content = Some(
            (1..=30)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let view = RenderedViewport;

        app.apply_core_effects(
            &view,
            vec![CoreEffect::CommentsPanel(
                CommentsPanelEffect::NavigateNextComment,
            )],
        );

        assert_eq!(app.state.selected_comment_id, Some(7));
        assert_eq!(app.state.diff_line_cursor, 17);
        assert_eq!(app.state.diff_scroll, 11);
    }

    #[test]
    fn comment_navigation_keeps_view_full_near_file_end() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("src/main.rs")],
        );
        let mut comment = stored_comment(7, "src/main.rs");
        move_comment_head_range(&mut comment, 27, 29);
        app.state.comments = vec![comment];
        app.state.head_content = Some(
            (1..=30)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let view = RenderedViewport;

        app.apply_core_effects(
            &view,
            vec![CoreEffect::CommentsPanel(
                CommentsPanelEffect::NavigateNextComment,
            )],
        );

        assert_eq!(app.state.selected_comment_id, Some(7));
        assert_eq!(app.state.diff_line_cursor, 26);
        assert_eq!(app.state.diff_scroll, 20);
    }

    #[test]
    fn comment_navigation_keeps_visible_comment_on_screen_without_scrolling() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("src/main.rs")],
        );
        let mut comment = stored_comment(7, "src/main.rs");
        move_comment_head_range(&mut comment, 8, 9);
        app.state.comments = vec![comment];
        app.state.content_mode = ContentMode::FullFile;
        app.state.render_variant = RenderVariant::HeadVersion;
        app.state.head_content = Some(
            (1..=30)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        app.state.diff_scroll = 5;
        app.state.diff_line_cursor = 5;
        let view = RenderedViewport;

        app.apply_core_effects(
            &view,
            vec![CoreEffect::CommentsPanel(
                CommentsPanelEffect::NavigateNextComment,
            )],
        );

        assert_eq!(app.state.selected_comment_id, Some(7));
        assert_eq!(app.state.diff_line_cursor, 7);
        assert_eq!(app.state.diff_scroll, 5);
    }

    #[test]
    fn comment_navigation_top_aligns_large_comment_that_cannot_fit() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("src/main.rs")],
        );
        let mut comment = stored_comment(7, "src/main.rs");
        move_comment_head_range(&mut comment, 12, 30);
        app.state.comments = vec![comment];
        app.state.head_content = Some(
            (1..=40)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let view = RenderedViewport;

        app.apply_core_effects(
            &view,
            vec![CoreEffect::CommentsPanel(
                CommentsPanelEffect::NavigateNextComment,
            )],
        );

        assert_eq!(app.state.selected_comment_id, Some(7));
        assert_eq!(app.state.diff_line_cursor, 11);
        assert_eq!(app.state.diff_scroll, 11);
    }

    #[test]
    fn comment_navigation_works_from_inline_deletion_row() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_leading_deletion_hunk("src/main.rs")],
        );
        let mut comment = stored_comment(7, "src/main.rs");
        move_comment_head_range(&mut comment, 16, 16);
        app.state.comments = vec![comment];
        app.state.head_content = Some(
            (1..=20)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        app.state.diff_line_cursor = 0;
        let view = RenderedViewport;

        app.apply_core_effects(
            &view,
            vec![CoreEffect::CommentsPanel(
                CommentsPanelEffect::NavigateNextComment,
            )],
        );

        assert_eq!(app.state.selected_comment_id, Some(7));
        assert_eq!(app.state.diff_line_cursor, 16);
    }

    #[test]
    fn unresolved_navigation_to_inline_replacement_uses_start_marker_row() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_offset_multiline_replacement_hunk(
                "src/main.rs",
            )],
        );
        let mut comment = stored_comment(7, "src/main.rs");
        comment
            .replace_anchor(crate::review_types::CommentAnchor {
                segments: vec![
                    CommentAnchorSegment {
                        side: CommentAnchorSide::Base,
                        file_path: "src/main.rs".to_string(),
                        line_start: 26,
                        line_end: 26,
                        char_start: None,
                        char_end: None,
                        anchor_text: "old".to_string(),
                        context_before: String::new(),
                        context_after: String::new(),
                        placement_status: AnchorPlacementStatus::Anchored,
                        match_method: AnchorMatchMethod::ExactAtLine,
                    },
                    CommentAnchorSegment {
                        side: CommentAnchorSide::Head,
                        file_path: "src/main.rs".to_string(),
                        line_start: 27,
                        line_end: 28,
                        char_start: None,
                        char_end: None,
                        anchor_text: "new".to_string(),
                        context_before: String::new(),
                        context_after: String::new(),
                        placement_status: AnchorPlacementStatus::Anchored,
                        match_method: AnchorMatchMethod::ExactAtLine,
                    },
                ],
                aggregate_status: crate::review_types::AnchorAggregateStatus::Anchored,
            })
            .expect("test comment anchor should be valid");
        app.state.comments = vec![comment];
        app.state.head_content = Some(
            (1..=34)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        app.state.file_list_section_focus = FileListSectionFocus::UnresolvedComments;
        app.state.diff_line_cursor = 24;
        let view = RenderedViewport;

        app.apply_core_effects(
            &view,
            vec![CoreEffect::NavigateUnresolvedComment(Direction::Next)],
        );

        assert_eq!(app.state.selected_comment_id, Some(7));
        assert_eq!(app.state.diff_line_cursor, 26);
    }

    #[test]
    fn side_by_side_comment_navigation_uses_paired_rows() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_replacement_hunk("src/main.rs")],
        );
        let mut comment = stored_comment(7, "src/main.rs");
        move_comment_head_range(&mut comment, 16, 16);
        app.state.comments = vec![comment];
        app.state.head_content = Some(
            (1..=20)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        app.state.render_variant = RenderVariant::SideBySide;
        app.state.diff_line_cursor = 0;
        let view = RenderedViewport;

        app.apply_core_effects(
            &view,
            vec![CoreEffect::CommentsPanel(
                CommentsPanelEffect::NavigateNextComment,
            )],
        );

        assert_eq!(app.state.selected_comment_id, Some(7));
        assert_eq!(app.state.diff_line_cursor, 15);
    }

    #[test]
    fn previous_comment_uses_nearest_start_before_cursor() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("src/main.rs")],
        );
        let mut early = stored_comment(7, "src/main.rs");
        move_comment_head_range(&mut early, 16, 20);
        let mut outer = stored_comment(8, "src/main.rs");
        move_comment_head_range(&mut outer, 31, 50);
        let mut nested = stored_comment(9, "src/main.rs");
        move_comment_head_range(&mut nested, 36, 36);
        app.state.comments = vec![early, outer, nested];
        app.state.head_content = Some(
            (1..=60)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        app.state.diff_line_cursor = 37;
        let view = RenderedViewport;

        app.apply_core_effects(
            &view,
            vec![CoreEffect::CommentsPanel(
                CommentsPanelEffect::NavigatePreviousComment,
            )],
        );

        assert_eq!(app.state.selected_comment_id, Some(9));
        assert_eq!(app.state.diff_line_cursor, 35);

        app.apply_core_effects(
            &view,
            vec![CoreEffect::CommentsPanel(
                CommentsPanelEffect::NavigatePreviousComment,
            )],
        );

        assert_eq!(app.state.selected_comment_id, Some(8));
        assert_eq!(app.state.diff_line_cursor, 30);
    }

    #[test]
    fn current_comment_uses_innermost_comment_on_cursor_line() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_hunk("src/main.rs")],
        );
        app.state.head_content = Some("1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n".to_string());
        let mut outer = stored_comment(1, "src/main.rs");
        move_comment_head_range(&mut outer, 10, 14);
        let mut inner = stored_comment(2, "src/main.rs");
        move_comment_head_range(&mut inner, 11, 11);
        app.state.comments = vec![outer, inner];
        app.state.diff_line_cursor = 10;
        app.state.ensure_active_document();

        let context = app.interaction_context();

        assert_eq!(context.current_comment.map(|comment| comment.id), Some(2));
    }

    #[test]
    fn selected_overlapping_comment_wins_current_comment_lookup() {
        let path = "src/app/update/comments.rs";
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_deletion_to_later_head_context_hunk(path)],
        );
        app.state.content_mode = ContentMode::Diff;
        app.state.render_variant = RenderVariant::Inline;
        app.state.head_content = Some(
            (1..=188)
                .map(|n| format!("head {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let mut broad = stored_comment(33, path);
        move_comment_to_base_head_ranges(&mut broad, 154, 167, 172, 188);
        let mut nested = stored_comment(34, path);
        move_comment_head_range(&mut nested, 172, 175);
        app.state.comments = vec![broad, nested];
        app.state.rebuild_active_document();
        app.state.diff_line_cursor = app
            .state
            .active_document
            .as_ref()
            .and_then(|document| {
                document
                    .document()
                    .diff
                    .row_for_source_line(CommentAnchorSide::Head, 172)
            })
            .expect("head 172 should render")
            .0;

        app.state.selected_comment_id = None;
        app.state.ensure_active_document();
        let default_context = app.interaction_context();
        assert_eq!(
            default_context.current_comment.map(|comment| comment.id),
            Some(34)
        );

        app.state.selected_comment_id = Some(33);
        app.state.ensure_active_document();
        let selected_context = app.interaction_context();
        assert_eq!(
            selected_context.current_comment.map(|comment| comment.id),
            Some(33)
        );

        let model = app.model();
        assert_eq!(model.comments_panel.comments.len(), 1);
        assert_eq!(model.comments_panel.comments[0].id, 33);
    }

    #[test]
    fn resolving_current_comment_queues_lifecycle_work() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("src/main.rs")],
        );
        app.state.comments = vec![stored_comment(7, "src/main.rs")];
        app.state.head_content = Some("one\ntwo\n".to_string());
        app.state.diff_line_cursor = 1;

        app.apply_core_effects(
            &EmptyViewport,
            vec![CoreEffect::CommentsPanel(
                CommentsPanelEffect::ToggleResolvedCurrent,
            )],
        );

        assert_eq!(
            app.pending_work.pop_front(),
            Some(AppWork::ResolveComment { id: 7 })
        );
    }

    #[test]
    fn comment_body_cancel_clears_pending_anchor() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("src/main.rs")],
        );
        app.state.pending_comment_anchor = Some(CommentAnchorCapture {
            segments: test_anchor("src/main.rs", 1, 1, "fn main() {}").segments,
            file_path: "src/main.rs".to_string(),
            line_start: 1,
            line_end: 1,
            char_start: None,
            char_end: None,
            anchor_text: "fn main() {}".to_string(),
            context_before: String::new(),
            context_after: String::new(),
        });

        let output = app.apply_core_effects(
            &EmptyViewport,
            vec![CoreEffect::Comment(CommentEffect::Cancel)],
        );

        assert!(app.state.pending_comment_anchor.is_none());
        assert_eq!(
            output.status,
            Some(StatusUpdate::Set("Comment canceled".to_string()))
        );
    }

    #[test]
    fn app_toggles_blame_and_whitespace_from_core_effects() {
        let mut app = App::new(Config::default(), test_context(), vec![test_file("a.rs")]);
        app.state.diff_search_query = Some("Command".to_string());
        app.state.diff_search_matches = vec![(4, 1, 8)];

        app.apply_core_effects(&EmptyViewport, vec![CoreEffect::ToggleBlame]);

        assert!(app.state.show_blame);

        app.apply_core_effects(&EmptyViewport, vec![CoreEffect::ToggleBlame]);

        assert!(!app.state.show_blame);

        app.apply_core_effects(&EmptyViewport, vec![CoreEffect::ToggleWhitespaceIgnored]);

        assert!(app.state.ignore_whitespace);
        assert_eq!(app.state.diff_search_query.as_deref(), Some("Command"));
        assert!(app.state.diff_search_matches.is_empty());

        app.apply_core_effects(&EmptyViewport, vec![CoreEffect::ToggleWhitespaceIgnored]);

        assert!(!app.state.ignore_whitespace);
    }

    #[test]
    fn app_owns_search_overlay_selection() {
        let mut app = App::new(Config::default(), test_context(), vec![test_file("a.rs")]);
        app.state.search_results = Some(SearchResults {
            query: "needle".to_string(),
            diff_only: false,
            matches: vec![
                SearchMatch {
                    file_path: "a.rs".to_string(),
                    line_number: 1,
                    line_content: "first".to_string(),
                },
                SearchMatch {
                    file_path: "b.rs".to_string(),
                    line_number: 2,
                    line_content: "second".to_string(),
                },
            ],
            selected: 0,
        });

        app.apply_core_effects(
            &EmptyViewport,
            vec![CoreEffect::SearchResults(SearchResultsEffect::SelectNext)],
        );

        assert_eq!(app.state.search_results.as_ref().unwrap().selected, 1);

        app.apply_core_effects(
            &EmptyViewport,
            vec![CoreEffect::SearchResults(SearchResultsEffect::Close)],
        );

        assert!(app.state.search_results.is_none());
    }

    #[test]
    fn app_owns_definition_overlay_selection() {
        let mut app = App::new(Config::default(), test_context(), vec![test_file("a.rs")]);
        app.state.definition_results = Some(DefinitionResults {
            symbol: "needle".to_string(),
            definitions: vec![
                DefinitionLocation {
                    file_path: "a.rs".to_string(),
                    line_number: 1,
                    line_content: "first".to_string(),
                },
                DefinitionLocation {
                    file_path: "b.rs".to_string(),
                    line_number: 2,
                    line_content: "second".to_string(),
                },
            ],
            selected: 0,
        });

        app.apply_core_effects(
            &EmptyViewport,
            vec![CoreEffect::DefinitionResults(
                DefinitionResultsEffect::SelectNext,
            )],
        );

        assert_eq!(app.state.definition_results.as_ref().unwrap().selected, 1);

        app.apply_core_effects(
            &EmptyViewport,
            vec![CoreEffect::DefinitionResults(
                DefinitionResultsEffect::Close,
            )],
        );

        assert!(app.state.definition_results.is_none());
    }

    #[test]
    fn app_owns_pane_visibility_and_layout_width() {
        let mut app = App::new(Config::default(), test_context(), vec![test_file("a.rs")]);

        app.apply_core_effects(
            &EmptyViewport,
            vec![CoreEffect::TogglePaneVisibility(PaneId::FileList)],
        );

        assert!(!app.state.show_file_list);
        assert!(app.state.show_diff_pane);
        assert_eq!(app.state.pane_focus, PaneFocus::Diff);

        app.set_file_list_width(72);

        assert_eq!(app.state.file_list_width, 72);
        assert_eq!(app.config.layout.file_list_width, 72);
        assert_eq!(app.model().layout.file_list_width, 72);
    }

    #[test]
    fn review_cleared_notifications_require_snapshot_reload() {
        assert!(!notification_requires_snapshot_reload(&[notification(
            NotificationKind::ReviewChanged {
                file_path: "a.rs".to_string(),
                status: Some(reviewed()),
            },
        )]));
        assert!(notification_requires_snapshot_reload(&[notification(
            NotificationKind::ReviewsCleared,
        )]));
        assert!(notification_requires_snapshot_reload(&[notification(
            NotificationKind::ReviewsMigrated { count: 3 },
        )]));
        assert!(!notification_requires_snapshot_reload(&[notification(
            NotificationKind::CommentChanged { comment_id: 7 },
        )]));
    }

    #[test]
    fn comment_notifications_require_comment_reload() {
        assert!(notification_requires_comment_reload(&[notification(
            NotificationKind::CommentChanged { comment_id: 7 },
        )]));
        assert!(!notification_requires_comment_reload(&[notification(
            NotificationKind::ReviewChanged {
                file_path: "a.rs".to_string(),
                status: Some(reviewed()),
            },
        )]));
    }

    #[test]
    fn review_changed_notification_without_status_needs_status_refresh() {
        assert!(notification_requires_status_refresh(&[notification(
            NotificationKind::ReviewChanged {
                file_path: "a.rs".to_string(),
                status: None,
            },
        )]));
        assert!(!notification_requires_status_refresh(&[notification(
            NotificationKind::ReviewChanged {
                file_path: "a.rs".to_string(),
                status: Some(reviewed()),
            },
        )]));
    }

    #[test]
    fn apply_review_change_notifications_patches_status_without_full_reload() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("a.rs"), test_file("b.rs")],
        );
        app.apply_review_change_notifications(&[notification(NotificationKind::ReviewChanged {
            file_path: "a.rs".to_string(),
            status: Some(reviewed()),
        })]);

        assert!(matches!(
            app.state
                .files
                .iter()
                .find(|entry| entry.change.path == "a.rs")
                .map(|entry| &entry.status),
            Some(ReviewStatus::Reviewed { .. })
        ));
        // Selection is preserved (no auto-advance on peer/status patch).
        assert_eq!(app.state.files[app.state.selected_file].change.path, "a.rs");
    }

    #[test]
    fn refresh_current_file_diff_reuses_loaded_hunks() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_hunk("a.rs")],
        );
        let original_header = app.state.files[0].diff.hunks[0].header.clone();
        app.state.refresh_current_file_diff_raw();
        assert_eq!(app.state.files[0].diff.hunks[0].header, original_header);
    }
}
