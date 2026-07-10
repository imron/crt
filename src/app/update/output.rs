use crate::app::CommentAnchorCapture;
use crate::core::ConnectionState;
use crate::core::command::Command;

#[derive(Debug, Default)]
pub struct AppOutput {
    pub handled: bool,
    pub status: Option<StatusUpdate>,
    pending_review_toggle: bool,
    pending_undo: bool,
    pub should_suspend: bool,
    pub should_quit: bool,
    pending_command: Option<Command>,
    pending_comment_create: Option<(CommentAnchorCapture, String)>,
    pending_comment_update: Option<(i64, String)>,
    pending_comment_resolve: Option<i64>,
    pending_comment_unresolve: Option<i64>,
    pending_comment_delete: Option<i64>,
    pub save_layout: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusUpdate {
    Set(String),
    Clear,
    ConnectionState(ConnectionState),
}

impl AppOutput {
    pub fn handled() -> Self {
        Self {
            handled: true,
            ..Self::default()
        }
    }

    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status = Some(StatusUpdate::Set(message.into()));
    }

    pub fn clear_status(&mut self) {
        self.status = Some(StatusUpdate::Clear);
    }

    pub fn request_review_toggle(&mut self) {
        self.pending_review_toggle = true;
    }

    pub fn take_pending_review_toggle(&mut self) -> bool {
        let pending = self.pending_review_toggle;
        self.pending_review_toggle = false;
        pending
    }

    pub fn request_undo(&mut self) {
        self.pending_undo = true;
    }

    pub fn take_pending_undo(&mut self) -> bool {
        let pending = self.pending_undo;
        self.pending_undo = false;
        pending
    }

    pub fn request_suspend(&mut self) {
        self.should_suspend = true;
    }

    pub fn request_quit(&mut self) {
        self.should_quit = true;
    }

    pub fn request_command(&mut self, command: Command) {
        self.pending_command = Some(command);
    }

    pub fn take_pending_command(&mut self) -> Option<Command> {
        self.pending_command.take()
    }

    pub fn request_comment_create(&mut self, anchor: CommentAnchorCapture, body: String) {
        self.pending_comment_create = Some((anchor, body));
    }

    pub fn take_pending_comment_create(&mut self) -> Option<(CommentAnchorCapture, String)> {
        self.pending_comment_create.take()
    }

    pub fn request_comment_update(&mut self, id: i64, body: String) {
        self.pending_comment_update = Some((id, body));
    }

    pub fn take_pending_comment_update(&mut self) -> Option<(i64, String)> {
        self.pending_comment_update.take()
    }

    pub fn request_comment_resolve(&mut self, id: i64) {
        self.pending_comment_resolve = Some(id);
    }

    pub fn take_pending_comment_resolve(&mut self) -> Option<i64> {
        self.pending_comment_resolve.take()
    }

    pub fn request_comment_unresolve(&mut self, id: i64) {
        self.pending_comment_unresolve = Some(id);
    }

    pub fn take_pending_comment_unresolve(&mut self) -> Option<i64> {
        self.pending_comment_unresolve.take()
    }

    pub fn request_comment_delete(&mut self, id: i64) {
        self.pending_comment_delete = Some(id);
    }

    pub fn take_pending_comment_delete(&mut self) -> Option<i64> {
        self.pending_comment_delete.take()
    }

    pub fn request_layout_save(&mut self) {
        self.save_layout = true;
    }
}
