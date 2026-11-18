//! Push notifications to connected clients.
//!
//! When state changes on one connection (e.g. a comment is resolved),
//! the server broadcasts a notification to all other connections in the
//! same scope.

use serde::{Deserialize, Serialize};

/// A server-to-client notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    /// The scope this notification applies to.
    pub base_ref: String,
    pub head_ref: String,
    /// What changed.
    pub kind: NotificationKind,
}

/// Types of state changes that trigger notifications.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NotificationKind {
    /// A file's review status changed.
    ReviewChanged { file_path: String },
    /// A comment was created, updated, resolved, or deleted.
    CommentChanged { comment_id: i64 },
    /// Reviews were cleared/reset.
    ReviewsCleared,
}
