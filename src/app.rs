//! Application state and input dispatch.

pub mod model;
mod update;

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;

use self::model::AppModel;
use crate::client::{Client, ClientEvent, Notification};
use crate::config::Config;
use crate::core::TextAnchor;
use crate::core::command::Command;
use crate::core::diff;
use crate::core::interaction::{CoreEffect, CoreInteractionEngine, InteractionContext};
use crate::core::review;
use crate::core::search as core_search;
use crate::core::{ConnectionState, InputEvent};
use crate::protocol::NotificationKind;
use crate::review_types::{
    Comment, ConnectionContext, ContentMode, CreateCommentParams, DefinitionLocation, FileEntry,
    ListCommentsResult, PaneFocus, RenderVariant, ReviewActionResult, ReviewStatus, SearchMatch,
};
use anyhow::{Context, Result};

pub use update::{AppOutput, AppViewport, StatusUpdate};

#[derive(Debug, Clone, PartialEq, Eq)]
enum AppWork {
    ToggleSelectedReview,
    UndoLastAction,
    ReloadFileSnapshot,
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

async fn list_current_and_previous_unresolved_comments(
    client: &Client,
) -> Result<ListCommentsResult> {
    let mut result = client.list_comments(None, true, false).await?;
    let previous_unresolved = client.list_comments(None, false, true).await?;
    append_unique_comments(&mut result.comments, previous_unresolved.comments);
    Ok(result)
}

fn append_unique_comments(comments: &mut Vec<Comment>, new_comments: Vec<Comment>) {
    let mut seen: BTreeSet<i64> = comments.iter().map(|comment| comment.id).collect();
    for comment in new_comments {
        if seen.insert(comment.id) {
            comments.push(comment);
        }
    }
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
    pub start: TextAnchor,
    pub end: TextAnchor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentAnchorCapture {
    pub file_path: String,
    pub line_start: i64,
    pub line_end: i64,
    pub char_start: Option<i64>,
    pub char_end: Option<i64>,
    pub anchor_text: String,
    pub context_before: String,
    pub context_after: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileListSectionFocus {
    Unreviewed,
    Reviewed,
    UnresolvedComments,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedFilePosition {
    pub diff_scroll: usize,
    pub diff_line_cursor: usize,
    pub diff_col_cursor: usize,
    pub content_mode: ContentMode,
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

    pub async fn start_review(base: &str, reset: bool, standalone: bool) -> Result<ReviewStartup> {
        let socket_path = default_socket_path()?;
        let cwd = std::env::current_dir().context("Failed to determine current directory")?;
        let client = Client::connect_or_start(&socket_path, standalone).await?;
        let init = client.init(&cwd.to_string_lossy(), base).await?;

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

        Ok(ReviewStartup::Review(Self::load(client, init).await?))
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
        let comments = match list_current_and_previous_unresolved_comments(&client).await {
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
        list_current_and_previous_unresolved_comments(self.client()?).await
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
                AppWork::ReloadFileSnapshot => self.reload_file_snapshot().await,
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
        if notification_requires_comment_reload(&notifications) {
            return self.reload_comments().await;
        }

        None
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
        if matches!(event, InputEvent::FocusGained) {
            self.pending_work.push_back(AppWork::ReloadFileSnapshot);
            return Vec::new();
        }
        self.state.core_interaction.handle_input(event, context)
    }

    pub fn apply_core_effects(
        &mut self,
        viewport: &impl AppViewport,
        effects: Vec<CoreEffect>,
    ) -> AppOutput {
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

    pub fn refresh_active_diff_search(&mut self, viewport: &impl AppViewport) -> bool {
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
        match self.list_changed_files().await {
            Ok(result) => {
                self.replace_file_snapshot(result.files);
                if let Some(status) = self.reload_comments().await {
                    return Some(status);
                }
                None
            }
            Err(e) => Some(StatusUpdate::Set(format!("Failed to reload files: {e}"))),
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

        self.state.refresh_current_file_diff();
        self.state.load_head_content();
        self.state.load_blame();

        if restored_same_file {
            self.state.content_mode = saved_content_mode;
            self.state.render_variant = saved_render_variant;
            self.state.diff_line_cursor = saved_cursor;
            self.state.diff_col_cursor = saved_col;
            self.state.diff_scroll = saved_scroll;
        } else {
            self.state.on_file_changed();
        }
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
            Command::ViewFile { path, line_number } => Some(StatusUpdate::Set(format!(
                "File not in diff: {path} {line_number}"
            ))),
            Command::Quit
            | Command::SetBlame(_)
            | Command::SetComments(_)
            | Command::SetWhitespaceIgnored(_)
            | Command::Unknown { .. } => {
                Some(StatusUpdate::Set("Unsupported pending command".to_string()))
            }
        }
    }

    async fn create_comment_from_anchor(
        &mut self,
        anchor: CommentAnchorCapture,
        body: String,
    ) -> Option<StatusUpdate> {
        let params = CreateCommentParams {
            file_path: anchor.file_path,
            line_start: anchor.line_start,
            line_end: anchor.line_end,
            char_start: anchor.char_start,
            char_end: anchor.char_end,
            anchor_text: anchor.anchor_text,
            context_before: anchor.context_before,
            context_after: anchor.context_after,
            body,
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
            NotificationKind::ReviewChanged { .. }
                | NotificationKind::ReviewsCleared
                | NotificationKind::ReviewsMigrated { .. }
        )
    })
}

fn notification_requires_comment_reload(notifications: &[Notification]) -> bool {
    notifications
        .iter()
        .any(|notification| matches!(notification.kind, NotificationKind::CommentChanged { .. }))
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
    /// Core interaction entrypoint used by the TUI adapter for migrated input.
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
        self.model_revision = self.model_revision.wrapping_add(1);
    }

    pub fn set_file_list_width(&mut self, width: u16) {
        if self.file_list_width != width {
            self.file_list_width = width;
            self.mark_model_changed();
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

    pub fn focus_for_file(&self, file_index: usize) -> FileListSectionFocus {
        if file_index < self.unreviewed_count() {
            FileListSectionFocus::Unreviewed
        } else {
            FileListSectionFocus::Reviewed
        }
    }

    pub fn save_selected_file_position(&mut self) {
        let Some(path) = self
            .selected_file_entry()
            .map(|entry| entry.change.path.clone())
        else {
            return;
        };
        self.saved_file_positions.insert(
            path,
            SavedFilePosition {
                diff_scroll: self.diff_scroll,
                diff_line_cursor: self.diff_line_cursor,
                diff_col_cursor: self.diff_col_cursor,
                content_mode: self.content_mode,
                render_variant: self.render_variant,
            },
        );
    }

    pub fn restore_selected_file_position(&mut self) {
        let Some(position) = self
            .selected_file_entry()
            .and_then(|entry| self.saved_file_positions.get(&entry.change.path))
            .copied()
        else {
            return;
        };

        self.content_mode = position.content_mode;
        self.render_variant = position.render_variant;
        if self.content_mode == ContentMode::FullFile
            && self.render_variant == RenderVariant::BaseVersion
        {
            self.load_base_content_raw();
        }
        self.diff_scroll = position.diff_scroll;
        self.diff_line_cursor = position.diff_line_cursor;
        self.diff_col_cursor = position.diff_col_cursor;
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
            self.base_content =
                diff::file_content(&self.context.worktree, &self.context.merge_base, &path);
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
        self.refresh_current_file_diff_raw();
        self.mark_model_changed();
    }

    fn refresh_current_file_diff_raw(&mut self) {
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
        self.reload_current_diff_raw();
        self.mark_model_changed();
    }

    fn reload_current_diff_raw(&mut self) {
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
        self.refresh_current_file_diff_raw();
        self.load_head_content_raw();
        self.load_blame_raw();

        // Reload base content if we're currently in base view.
        if self.content_mode == ContentMode::FullFile
            && self.render_variant == RenderVariant::BaseVersion
        {
            self.load_base_content_raw();
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
        self.visual_selection = None;
        self.pending_comment_anchor = None;
        self.selected_comment_id = None;
        self.pending_delete_comment_id = None;
        self.invalidate_diff_search_matches();
        self.mark_model_changed();
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::navigation::Direction;
    use crate::core::{
        CommentEffect, CommentsPanelEffect, CoreEffect, DefinitionResultsEffect, DiffCursorEffect,
        PaneEffect, PaneId, SearchResultsEffect, VisualSelectionEffect,
    };
    use crate::protocol::NotificationKind;
    use crate::review_types::{
        ChangeKind, DiffContent, DiffHunk, DiffLine, FileChange, FileEntry, LineKind, ReviewStatus,
    };

    struct EmptyViewport;

    impl AppViewport for EmptyViewport {
        fn hunk_start_rows(&self) -> &[usize] {
            &[]
        }

        fn hunk_end_rows(&self) -> &[usize] {
            &[]
        }

        fn hunk_first_change_rows(&self) -> &[usize] {
            &[]
        }

        fn diff_gutter_cols(&self) -> usize {
            0
        }

        fn diff_content_height(&self) -> usize {
            0
        }

        fn diff_view_height(&self) -> usize {
            0
        }

        fn diff_rendered_text(&self) -> &[String] {
            &[]
        }
    }

    struct RenderedViewport {
        lines: Vec<String>,
    }

    impl RenderedViewport {
        fn new(lines: Vec<&str>) -> Self {
            Self {
                lines: lines.into_iter().map(str::to_string).collect(),
            }
        }
    }

    impl AppViewport for RenderedViewport {
        fn hunk_start_rows(&self) -> &[usize] {
            &[]
        }

        fn hunk_end_rows(&self) -> &[usize] {
            &[]
        }

        fn hunk_first_change_rows(&self) -> &[usize] {
            &[]
        }

        fn diff_gutter_cols(&self) -> usize {
            0
        }

        fn diff_content_height(&self) -> usize {
            self.lines.len()
        }

        fn diff_view_height(&self) -> usize {
            10
        }

        fn diff_rendered_text(&self) -> &[String] {
            &self.lines
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

    fn test_file(path: &str) -> FileEntry {
        test_file_with_status(path, ReviewStatus::Unreviewed)
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
        crate::review_types::Comment {
            id,
            merge_base: "abc123".to_string(),
            head_ref: "feature".to_string(),
            file_path: file_path.to_string(),
            line_start: 2,
            line_end: 2,
            char_start: None,
            char_end: None,
            anchor_text: "anchor".to_string(),
            context_before: String::new(),
            context_after: String::new(),
            body: "comment".to_string(),
            resolved: false,
            created_at: "2026-06-28T00:00:00+10:00".to_string(),
            updated_at: "2026-06-28T00:00:00+10:00".to_string(),
            anchor_status: crate::review_types::AnchorStatus::Anchored,
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

    #[test]
    fn focus_gained_queues_snapshot_reload_in_app() {
        let mut app = App::new(Config::default(), test_context(), vec![test_file("a.rs")]);

        let effects = app.handle_input(InputEvent::FocusGained, &InteractionContext::default());

        assert!(effects.is_empty());
        assert_eq!(
            app.pending_work.pop_front(),
            Some(AppWork::ReloadFileSnapshot)
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
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("a.rs"), test_file("b.rs")],
        );
        let mut first = stored_comment(3, "a.rs");
        first.line_start = 2;
        first.line_end = 2;
        let mut second = stored_comment(5, "b.rs");
        second.line_start = 6;
        second.line_end = 6;
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
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("a.rs"), test_file("b.rs")],
        );
        let mut comment = stored_comment(9, "b.rs");
        comment.line_start = 4;
        comment.line_end = 4;
        app.state.comments = vec![comment];
        app.state.selected_file = 1;
        app.state.pane_focus = PaneFocus::FileList;
        app.state.file_list_section_focus = FileListSectionFocus::UnresolvedComments;
        app.state.selected_comment_id = Some(9);
        app.state.content_mode = ContentMode::FullFile;
        app.state.render_variant = RenderVariant::HeadVersion;

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
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("a.rs"), test_file("b.rs")],
        );
        let mut first = stored_comment(3, "a.rs");
        first.line_start = 4;
        first.line_end = 4;
        let mut second = stored_comment(5, "b.rs");
        second.line_start = 2;
        second.line_end = 2;
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
    fn unresolved_comment_shortcut_visits_same_start_line_comments() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("a.rs"), test_file("b.rs")],
        );
        let mut outer = stored_comment(9, "a.rs");
        outer.line_start = 18;
        outer.line_end = 29;
        let mut middle = stored_comment(14, "a.rs");
        middle.line_start = 18;
        middle.line_end = 21;
        let mut single = stored_comment(15, "a.rs");
        single.line_start = 18;
        single.line_end = 18;
        let mut next_file = stored_comment(20, "b.rs");
        next_file.line_start = 4;
        next_file.line_end = 4;
        app.state.comments = vec![outer, next_file, single, middle];
        app.state.file_list_section_focus = FileListSectionFocus::UnresolvedComments;
        app.state.content_mode = ContentMode::FullFile;
        app.state.render_variant = RenderVariant::HeadVersion;
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
    fn unresolved_comment_navigation_selects_out_of_range_comment() {
        let mut app = App::new(Config::default(), test_context(), vec![test_file("a.rs")]);
        let mut comment = stored_comment(8, "mcp.rs");
        comment.line_start = 12;
        comment.line_end = 12;
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
    fn app_model_projects_current_file_comments() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("a.rs"), test_file("b.rs")],
        );
        app.state.comments = vec![stored_comment(1, "a.rs"), stored_comment(2, "b.rs")];
        app.state.selected_file = 1;

        let model = app.model();

        assert_eq!(model.diff.comments.len(), 1);
        assert_eq!(model.diff.comments[0].id, 2);
        assert_eq!(model.diff.comments[0].line_start, 2);
    }

    #[test]
    fn active_diff_search_recomputes_after_file_change() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("a.rs"), test_file("b.rs")],
        );
        app.state.diff_search_query = Some("Command".to_string());
        app.state.diff_search_matches = vec![(8, 1, 8)];
        app.state.diff_search_current = 0;

        app.state.selected_file = 1;
        app.state.on_file_changed();

        assert_eq!(app.state.diff_search_query.as_deref(), Some("Command"));
        assert!(app.state.diff_search_matches.is_empty());

        let view = RenderedViewport::new(vec!["let value = 1;", "    Command::Quit"]);
        assert!(app.refresh_active_diff_search(&view));

        assert_eq!(app.state.diff_search_matches, vec![(1, 4, 11)]);
        assert_eq!(app.state.diff_search_current, 0);
        assert_eq!(app.state.diff_line_cursor, 1);

        app.state.diff_line_cursor = 0;

        assert!(!app.refresh_active_diff_search(&view));
        assert_eq!(app.state.diff_line_cursor, 0);
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
        assert!(app.state.visual_selection.is_some());
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
        first.line_start = 2;
        first.line_end = 2;
        let mut second = stored_comment(8, "src/main.rs");
        second.line_start = 3;
        second.line_end = 3;
        app.state.comments = vec![first, second];
        app.state.head_content = Some("one\ntwo\nthree\n".to_string());
        app.state.show_comments_panel = true;
        app.state.diff_line_cursor = 1;

        let model = app.model();

        assert_eq!(model.comments_panel.comments.len(), 1);
        assert_eq!(model.comments_panel.comments[0].id, 7);
        assert_eq!(model.comments_panel.selected_index, Some(1));
        assert_eq!(model.comments_panel.total, 2);

        app.state.diff_line_cursor = 0;
        let model = app.model();

        assert!(model.comments_panel.comments.is_empty());
        assert_eq!(model.comments_panel.selected_index, None);
        assert_eq!(model.comments_panel.total, 2);
    }

    #[test]
    fn comment_navigation_is_relative_to_cursor_line() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("src/main.rs")],
        );
        let mut first = stored_comment(7, "src/main.rs");
        first.line_start = 2;
        first.line_end = 2;
        let mut second = stored_comment(8, "src/main.rs");
        second.line_start = 4;
        second.line_end = 4;
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
        comment.line_start = 18;
        comment.line_end = 20;
        app.state.comments = vec![comment];
        app.state.head_content = Some(
            (1..=30)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let lines: Vec<String> = (1..=30).map(|n| n.to_string()).collect();
        let view = RenderedViewport { lines };

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
        comment.line_start = 27;
        comment.line_end = 29;
        app.state.comments = vec![comment];
        app.state.head_content = Some(
            (1..=30)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let lines: Vec<String> = (1..=30).map(|n| n.to_string()).collect();
        let view = RenderedViewport { lines };

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
    fn comment_navigation_starts_large_comment_after_bounded_context() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file("src/main.rs")],
        );
        let mut comment = stored_comment(7, "src/main.rs");
        comment.line_start = 12;
        comment.line_end = 30;
        app.state.comments = vec![comment];
        app.state.head_content = Some(
            (1..=40)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let lines: Vec<String> = (1..=40).map(|n| n.to_string()).collect();
        let view = RenderedViewport { lines };

        app.apply_core_effects(
            &view,
            vec![CoreEffect::CommentsPanel(
                CommentsPanelEffect::NavigateNextComment,
            )],
        );

        assert_eq!(app.state.selected_comment_id, Some(7));
        assert_eq!(app.state.diff_line_cursor, 11);
        assert_eq!(app.state.diff_scroll, 6);
    }

    #[test]
    fn comment_navigation_works_from_inline_deletion_row() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_leading_deletion_hunk("src/main.rs")],
        );
        let mut comment = stored_comment(7, "src/main.rs");
        comment.line_start = 16;
        comment.line_end = 16;
        app.state.comments = vec![comment];
        app.state.head_content = Some(
            (1..=20)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        app.state.diff_line_cursor = 0;
        let lines: Vec<String> = (1..=20).map(|n| n.to_string()).collect();
        let view = RenderedViewport { lines };

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
    fn side_by_side_comment_navigation_uses_paired_rows() {
        let mut app = App::new(
            Config::default(),
            test_context(),
            vec![test_file_with_replacement_hunk("src/main.rs")],
        );
        let mut comment = stored_comment(7, "src/main.rs");
        comment.line_start = 16;
        comment.line_end = 16;
        app.state.comments = vec![comment];
        app.state.head_content = Some(
            (1..=20)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        app.state.render_variant = RenderVariant::SideBySide;
        app.state.diff_line_cursor = 0;
        let lines: Vec<String> = (1..=20).map(|n| n.to_string()).collect();
        let view = RenderedViewport { lines };

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
        early.line_start = 16;
        early.line_end = 20;
        let mut outer = stored_comment(8, "src/main.rs");
        outer.line_start = 31;
        outer.line_end = 50;
        let mut nested = stored_comment(9, "src/main.rs");
        nested.line_start = 36;
        nested.line_end = 36;
        app.state.comments = vec![early, outer, nested];
        app.state.head_content = Some(
            (1..=60)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        app.state.diff_line_cursor = 37;
        let lines: Vec<String> = (1..=60).map(|n| n.to_string()).collect();
        let view = RenderedViewport { lines };

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
        outer.line_start = 10;
        outer.line_end = 14;
        let mut inner = stored_comment(2, "src/main.rs");
        inner.line_start = 11;
        inner.line_end = 11;
        app.state.comments = vec![outer, inner];
        app.state.diff_line_cursor = 10;

        let context = app.interaction_context();

        assert_eq!(context.current_comment.map(|comment| comment.id), Some(2));
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
    fn review_notifications_require_snapshot_reload() {
        assert!(notification_requires_snapshot_reload(&[notification(
            NotificationKind::ReviewChanged {
                file_path: "a.rs".to_string(),
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
            },
        )]));
    }
}
