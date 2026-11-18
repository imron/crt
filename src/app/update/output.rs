use crate::core::command::Command;

#[derive(Debug, Default)]
pub struct AppOutput {
    pub handled: bool,
    pub status: Option<StatusUpdate>,
    pending_review_toggle: bool,
    pub should_suspend: bool,
    pub should_quit: bool,
    pending_command: Option<Command>,
    pub save_layout: bool,
    pub show_comments: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusUpdate {
    Set(String),
    Clear,
}

impl AppOutput {
    pub(super) fn handled() -> Self {
        Self {
            handled: true,
            ..Self::default()
        }
    }

    pub(super) fn set_status(&mut self, message: impl Into<String>) {
        self.status = Some(StatusUpdate::Set(message.into()));
    }

    pub(super) fn clear_status(&mut self) {
        self.status = Some(StatusUpdate::Clear);
    }

    pub(super) fn request_review_toggle(&mut self) {
        self.pending_review_toggle = true;
    }

    pub(in crate::app) fn take_pending_review_toggle(&mut self) -> bool {
        let pending = self.pending_review_toggle;
        self.pending_review_toggle = false;
        pending
    }

    pub(super) fn request_suspend(&mut self) {
        self.should_suspend = true;
    }

    pub(super) fn request_quit(&mut self) {
        self.should_quit = true;
    }

    pub(super) fn request_command(&mut self, command: Command) {
        self.pending_command = Some(command);
    }

    pub(in crate::app) fn take_pending_command(&mut self) -> Option<Command> {
        self.pending_command.take()
    }

    pub(super) fn request_layout_save(&mut self) {
        self.save_layout = true;
    }

    pub(super) fn set_comments_visible(&mut self, show: bool) {
        self.show_comments = Some(show);
    }
}
