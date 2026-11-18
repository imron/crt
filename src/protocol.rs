//! Shared transport protocol types for the JSON-RPC client and server.
//!
//! This module owns wire-level request, response, error, and notification
//! shapes. Client and server code may depend on this module; client code
//! should not depend on `crate::server` internals for shared protocol models.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct JsonRpcCall<P: Serialize> {
    pub jsonrpc: &'static str,
    pub method: RpcMethod,
    pub params: P,
    pub id: u64,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcNotification<P: Serialize> {
    pub jsonrpc: &'static str,
    pub method: RpcMethod,
    pub params: P,
}

macro_rules! rpc_methods {
    ($($variant:ident => $wire_name:literal),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum RpcMethod {
            $($variant),+
        }

        impl RpcMethod {
            pub fn from_str(value: &str) -> Option<Self> {
                match value {
                    $($wire_name => Some(Self::$variant),)+
                    _ => None,
                }
            }

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire_name,)+
                }
            }
        }

        impl std::fmt::Display for RpcMethod {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

rpc_methods! {
    Init => "init",
    ListChangedFiles => "list_changed_files",
    ListFileStatuses => "list_file_statuses",
    GetFileDiff => "get_file_diff",
    GetFileContent => "get_file_content",
    MarkReviewed => "mark_reviewed",
    UnmarkReviewed => "unmark_reviewed",
    ResetReviews => "reset_reviews",
    CreateComment => "create_comment",
    ListComments => "list_comments",
    GetComment => "get_comment",
    UpdateComment => "update_comment",
    ResolveComment => "resolve_comment",
    UnresolveComment => "unresolve_comment",
    DeleteComment => "delete_comment",
    ApplyComments => "apply_comments",
    ClearComments => "clear_comments",
    SearchCodebase => "search_codebase",
    FindDefinition => "find_definition",
    TrackRepo => "track_repo",
    ListRepos => "list_repos",
    Notification => "notification",
}

impl Serialize for RpcMethod {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    #[serde(default)]
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
    pub id: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Clone)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    pub id: serde_json::Value,
}

#[derive(Debug, Serialize, Clone)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JsonRpcResponse {
    pub fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: Some(result),
            error: None,
            id,
        }
    }

    pub fn error(id: serde_json::Value, code: i64, message: String) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data: None,
            }),
            id,
        }
    }
}

/// JSON-RPC standard error codes.
pub const ERR_PARSE: i64 = -32700;
pub const ERR_METHOD_NOT_FOUND: i64 = -32601;
pub const ERR_INVALID_PARAMS: i64 = -32602;
pub const ERR_INTERNAL: i64 = -32603;

/// Application error codes.
pub const ERR_NOT_INITIALIZED: i64 = -32000;
pub const ERR_NOT_IMPLEMENTED: i64 = -32001;

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
    /// Reviews were migrated from an old scope after a rebase.
    ReviewsMigrated { count: usize },
}
