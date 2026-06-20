//! Review data types shared across modules.
//!
//! These types form the shared vocabulary between the git module, database
//! module, server, client, and UI. Each module can produce or consume these
//! types without importing another module's internals.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Git-originated types
// ---------------------------------------------------------------------------

/// How a file changed between the base and head commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Deleted,
    Modified,
    Renamed,
}

/// A file that changed between base and HEAD.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    /// Path in the new (HEAD) tree. For deletions this is the old path.
    pub path: String,
    /// For renames, the path in the old (base) tree.
    pub old_path: Option<String>,
    pub kind: ChangeKind,
}

/// The type of a single line within a diff hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    Context,
    Addition,
    Deletion,
}

/// A single line within a diff hunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: LineKind,
    pub content: String,
    /// Line number in the old file (None for additions).
    pub old_lineno: Option<u32>,
    /// Line number in the new file (None for deletions).
    pub new_lineno: Option<u32>,
}

/// A single hunk within a diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub header: String,
    pub lines: Vec<DiffLine>,
}

/// The complete diff for a single file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffContent {
    pub hunks: Vec<DiffHunk>,
    pub is_binary: bool,
    /// Stable SHA-256 hash of the diff content.
    pub diff_hash: String,
}

/// Which version of a file to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileVersion {
    /// The version at the base commit.
    Base,
    /// The version at HEAD.
    Head,
}

// ---------------------------------------------------------------------------
// Review types
// ---------------------------------------------------------------------------

/// A file's review state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ReviewStatus {
    /// Never reviewed.
    Unreviewed,
    /// Reviewed and diff unchanged since.
    Reviewed {
        /// ISO 8601 timestamp of when the file was reviewed.
        at: String,
        /// The HEAD commit OID at the time the file was reviewed.
        #[serde(default)]
        reviewed_commit: Option<String>,
    },
    /// Was reviewed, but the diff has changed since the review.
    Changed {
        /// ISO 8601 timestamp of the original review (now stale).
        at: String,
        /// The HEAD commit OID at the time the file was reviewed.
        #[serde(default)]
        reviewed_commit: Option<String>,
    },
}

/// The complete state of a single file in the review. Combines git change
/// information, review status, and diff content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub change: FileChange,
    pub status: ReviewStatus,
    pub diff: DiffContent,
}

// ---------------------------------------------------------------------------
// Comment types
// ---------------------------------------------------------------------------

/// How well a comment's anchor was resolved after a rebase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorStatus {
    /// Exact match at the stored line number.
    Anchored,
    /// Exact match, but at a different line number (code moved).
    Shifted,
    /// Context-assisted match (anchor gone but surrounding code matches).
    Approximate,
    /// No match found anywhere in the diff.
    Orphaned,
}

/// A review comment attached to a specific location in a diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comment {
    pub id: i64,
    pub merge_base: String,
    pub head_ref: String,
    pub file_path: String,
    pub line_start: i64,
    pub line_end: i64,
    /// Character start within the line (None for full-line selections).
    pub char_start: Option<i64>,
    /// Character end within the line (None for full-line selections).
    pub char_end: Option<i64>,
    /// The exact lines the comment is attached to.
    pub anchor_text: String,
    /// Lines preceding the anchor in the diff.
    pub context_before: String,
    /// Lines following the anchor in the diff.
    pub context_after: String,
    pub body: String,
    pub resolved: bool,
    pub created_at: String,
    pub updated_at: String,
    /// How well the anchor was resolved (computed at runtime).
    pub anchor_status: AnchorStatus,
}

// ---------------------------------------------------------------------------
// UI state types
// ---------------------------------------------------------------------------

/// Which pane currently has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneFocus {
    FileList,
    Diff,
}

/// What content is shown in the diff pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentMode {
    /// Show the diff between base and HEAD.
    Diff,
    /// Show the full file content.
    FullFile,
}

/// How the diff pane content is rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderVariant {
    /// Unified (inline) diff.
    Inline,
    /// Side-by-side diff.
    SideBySide,
    /// Full file at HEAD.
    HeadVersion,
    /// Full file at the base commit.
    BaseVersion,
}

// ---------------------------------------------------------------------------
// Connection types
// ---------------------------------------------------------------------------

/// The resolved state from the `init` handshake. This is what clients
/// receive after connecting and is used for display and scoping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionContext {
    /// The main repository root (not the worktree).
    pub repo_root: String,
    /// The worktree path where the command was run.
    pub worktree: String,
    /// The original base ref string (for display only, not used as a key).
    pub base_ref: String,
    /// The branch name at HEAD.
    pub head_ref: String,
    /// The merge-base commit hash (stable scope key).
    pub merge_base: String,
}

// ---------------------------------------------------------------------------
// Server message types — request params
// ---------------------------------------------------------------------------

/// Parameters for the `init` method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitParams {
    pub worktree: String,
    pub base_ref: String,
}

/// Parameters for `get_file_diff`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetFileDiffParams {
    pub file_path: String,
}

/// Parameters for `get_file_content`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetFileContentParams {
    pub file_path: String,
    pub version: FileVersion,
}

/// Parameters for `mark_reviewed`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkReviewedParams {
    pub file_path: String,
}

/// Parameters for `unmark_reviewed`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnmarkReviewedParams {
    pub file_path: String,
}

/// Parameters for `create_comment`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCommentParams {
    pub file_path: String,
    pub line_start: i64,
    pub line_end: i64,
    pub char_start: Option<i64>,
    pub char_end: Option<i64>,
    pub anchor_text: String,
    pub context_before: String,
    pub context_after: String,
    pub body: String,
}

/// Parameters for `list_comments`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListCommentsParams {
    pub file_path: Option<String>,
    #[serde(default)]
    pub include_resolved: bool,
}

/// Parameters for `get_comment`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetCommentParams {
    pub id: i64,
}

/// Parameters for `update_comment`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCommentParams {
    pub id: i64,
    pub body: String,
}

/// Parameters for `resolve_comment`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveCommentParams {
    pub id: i64,
}

/// Parameters for `unresolve_comment`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnresolveCommentParams {
    pub id: i64,
}

/// Parameters for `delete_comment`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteCommentParams {
    pub id: i64,
}

/// Parameters for `search_codebase`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchCodebaseParams {
    pub pattern: String,
    /// Scope: "all" or "diff".
    pub scope: Option<String>,
}

/// Parameters for `find_definition`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindDefinitionParams {
    pub symbol: String,
    pub context_file: Option<String>,
}

/// Parameters for `track_repo`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackRepoParams {
    pub path: String,
}

// ---------------------------------------------------------------------------
// Server message types — response results
// ---------------------------------------------------------------------------

/// Result of `list_changed_files`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListChangedFilesResult {
    pub files: Vec<FileEntry>,
}

/// Result of `get_file_diff`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetFileDiffResult {
    pub diff: DiffContent,
}

/// Result of `get_file_content`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetFileContentResult {
    /// None if the file doesn't exist at this version, or is binary.
    pub content: Option<String>,
}

/// Result of `mark_reviewed` / `unmark_reviewed`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewActionResult {
    pub file_path: String,
    pub status: ReviewStatus,
}

/// Result of `reset_reviews`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResetReviewsResult {
    pub cleared: u64,
}

/// Result of `create_comment`, `get_comment`, `update_comment`,
/// `resolve_comment`, `unresolve_comment`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentResult {
    pub comment: Comment,
}

/// Result of `list_comments`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListCommentsResult {
    pub comments: Vec<Comment>,
}

/// Result of `delete_comment`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteCommentResult {
    pub deleted: bool,
}

/// A single search match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMatch {
    pub file_path: String,
    pub line_number: u32,
    pub line_content: String,
}

/// Result of `search_codebase`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchCodebaseResult {
    pub matches: Vec<SearchMatch>,
}

/// A definition location.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefinitionLocation {
    pub file_path: String,
    pub line_number: u32,
    pub line_content: String,
}

/// Result of `find_definition`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindDefinitionResult {
    pub definitions: Vec<DefinitionLocation>,
}

/// Result of `apply_comments` / `clear_comments`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkerActionResult {
    pub files_modified: u32,
    pub markers_written: u32,
}

/// Result of `track_repo`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackRepoResult {
    pub path: String,
}

/// Result of `list_repos`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListReposResult {
    pub repos: Vec<String>,
}

// ---------------------------------------------------------------------------
// Conversions from database types
// ---------------------------------------------------------------------------

impl Comment {
    /// Create a `Comment` from a `db::StoredComment` with a given anchor status.
    ///
    /// This is the bridge between the database layer and the model layer.
    /// The anchor status is determined at runtime by the re-anchoring logic.
    pub fn from_stored(stored: &crate::db::StoredComment, anchor_status: AnchorStatus) -> Self {
        Self {
            id: stored.id,
            merge_base: stored.merge_base.clone(),
            head_ref: stored.head_ref.clone(),
            file_path: stored.file_path.clone(),
            line_start: stored.line_start,
            line_end: stored.line_end,
            char_start: stored.char_start,
            char_end: stored.char_end,
            anchor_text: stored.anchor_text.clone(),
            context_before: stored.context_before.clone(),
            context_after: stored.context_after.clone(),
            body: stored.body.clone(),
            resolved: stored.resolved,
            created_at: stored.created_at.clone(),
            updated_at: stored.updated_at.clone(),
            anchor_status,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_review_status_serialization() {
        let unreviewed = ReviewStatus::Unreviewed;
        let json = serde_json::to_string(&unreviewed).unwrap();
        assert!(json.contains("\"status\":\"unreviewed\""));

        let reviewed = ReviewStatus::Reviewed {
            at: "2026-03-29T14:30:00+10:00".to_string(),
            reviewed_commit: Some("abc123".to_string()),
        };
        let json = serde_json::to_string(&reviewed).unwrap();
        assert!(json.contains("\"status\":\"reviewed\""));
        assert!(json.contains("2026-03-29"));

        let changed = ReviewStatus::Changed {
            at: "2026-03-29T14:30:00+10:00".to_string(),
            reviewed_commit: Some("abc123".to_string()),
        };
        let json = serde_json::to_string(&changed).unwrap();
        assert!(json.contains("\"status\":\"changed\""));
    }

    #[test]
    fn test_review_status_roundtrip() {
        let statuses = vec![
            ReviewStatus::Unreviewed,
            ReviewStatus::Reviewed {
                at: "2026-03-29T14:30:00+10:00".to_string(),
                reviewed_commit: Some("abc123".to_string()),
            },
            ReviewStatus::Changed {
                at: "2026-03-29T14:30:00+10:00".to_string(),
                reviewed_commit: Some("abc123".to_string()),
            },
        ];

        for status in statuses {
            let json = serde_json::to_string(&status).unwrap();
            let deserialized: ReviewStatus = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&deserialized).unwrap();
            assert_eq!(json, json2);
        }
    }

    #[test]
    fn test_change_kind_serialization() {
        assert_eq!(
            serde_json::to_string(&ChangeKind::Added).unwrap(),
            "\"added\""
        );
        assert_eq!(
            serde_json::to_string(&ChangeKind::Renamed).unwrap(),
            "\"renamed\""
        );

        let roundtrip: ChangeKind = serde_json::from_str("\"modified\"").unwrap();
        assert_eq!(roundtrip, ChangeKind::Modified);
    }

    #[test]
    fn test_line_kind_serialization() {
        assert_eq!(
            serde_json::to_string(&LineKind::Addition).unwrap(),
            "\"addition\""
        );
        assert_eq!(
            serde_json::to_string(&LineKind::Deletion).unwrap(),
            "\"deletion\""
        );
        assert_eq!(
            serde_json::to_string(&LineKind::Context).unwrap(),
            "\"context\""
        );
    }

    #[test]
    fn test_file_version_serialization() {
        assert_eq!(
            serde_json::to_string(&FileVersion::Base).unwrap(),
            "\"base\""
        );
        assert_eq!(
            serde_json::to_string(&FileVersion::Head).unwrap(),
            "\"head\""
        );
    }

    #[test]
    fn test_anchor_status_serialization() {
        let statuses = [
            (AnchorStatus::Anchored, "\"anchored\""),
            (AnchorStatus::Shifted, "\"shifted\""),
            (AnchorStatus::Approximate, "\"approximate\""),
            (AnchorStatus::Orphaned, "\"orphaned\""),
        ];

        for (status, expected) in statuses {
            assert_eq!(serde_json::to_string(&status).unwrap(), expected);
            let roundtrip: AnchorStatus = serde_json::from_str(expected).unwrap();
            assert_eq!(roundtrip, status);
        }
    }

    #[test]
    fn test_file_entry_serialization() {
        let entry = FileEntry {
            change: FileChange {
                path: "src/main.rs".to_string(),
                old_path: None,
                kind: ChangeKind::Modified,
            },
            status: ReviewStatus::Unreviewed,
            diff: DiffContent {
                hunks: vec![DiffHunk {
                    old_start: 1,
                    old_lines: 3,
                    new_start: 1,
                    new_lines: 5,
                    header: "@@ -1,3 +1,5 @@".to_string(),
                    lines: vec![
                        DiffLine {
                            kind: LineKind::Context,
                            content: "fn main() {\n".to_string(),
                            old_lineno: Some(1),
                            new_lineno: Some(1),
                        },
                        DiffLine {
                            kind: LineKind::Deletion,
                            content: "    println!(\"hello\");\n".to_string(),
                            old_lineno: Some(2),
                            new_lineno: None,
                        },
                        DiffLine {
                            kind: LineKind::Addition,
                            content: "    let app = App::new();\n".to_string(),
                            old_lineno: None,
                            new_lineno: Some(2),
                        },
                    ],
                }],
                is_binary: false,
                diff_hash: "abc123".to_string(),
            },
        };

        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: FileEntry = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.change.path, "src/main.rs");
        assert_eq!(deserialized.change.kind, ChangeKind::Modified);
        assert_eq!(deserialized.diff.hunks.len(), 1);
        assert_eq!(deserialized.diff.hunks[0].lines.len(), 3);
    }

    #[test]
    fn test_connection_context_serialization() {
        let ctx = ConnectionContext {
            repo_root: "/path/to/repo".to_string(),
            worktree: "/path/to/repo".to_string(),
            base_ref: "main".to_string(),
            head_ref: "feature-a".to_string(),
            merge_base: "abc123def456".to_string(),
        };

        let json = serde_json::to_string(&ctx).unwrap();
        let deserialized: ConnectionContext = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.repo_root, "/path/to/repo");
        assert_eq!(deserialized.merge_base, "abc123def456");
    }

    #[test]
    fn test_comment_from_stored() {
        let stored = crate::db::StoredComment {
            id: 42,
            merge_base: "abc".to_string(),
            head_ref: "feat".to_string(),
            file_path: "src/lib.rs".to_string(),
            line_start: 10,
            line_end: 12,
            char_start: None,
            char_end: None,
            anchor_text: "fn foo() {}".to_string(),
            context_before: "// before".to_string(),
            context_after: "// after".to_string(),
            body: "Fix this".to_string(),
            resolved: false,
            created_at: "2026-03-29T14:00:00+10:00".to_string(),
            updated_at: "2026-03-29T14:00:00+10:00".to_string(),
        };

        let comment = Comment::from_stored(&stored, AnchorStatus::Anchored);

        assert_eq!(comment.id, 42);
        assert_eq!(comment.file_path, "src/lib.rs");
        assert_eq!(comment.body, "Fix this");
        assert_eq!(comment.anchor_status, AnchorStatus::Anchored);
        assert!(!comment.resolved);
    }

    #[test]
    fn test_server_message_roundtrip() {
        // Test a few representative message types
        let params = InitParams {
            worktree: "/path/to/repo".to_string(),
            base_ref: "main".to_string(),
        };
        let json = serde_json::to_string(&params).unwrap();
        let deserialized: InitParams = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.base_ref, "main");

        let params = ListCommentsParams {
            file_path: Some("src/lib.rs".to_string()),
            include_resolved: true,
        };
        let json = serde_json::to_string(&params).unwrap();
        let deserialized: ListCommentsParams = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.file_path.as_deref(), Some("src/lib.rs"));
        assert!(deserialized.include_resolved);

        let params = GetFileContentParams {
            file_path: "src/main.rs".to_string(),
            version: FileVersion::Head,
        };
        let json = serde_json::to_string(&params).unwrap();
        let deserialized: GetFileContentParams = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.version, FileVersion::Head);
    }

    #[test]
    fn test_list_comments_params_defaults() {
        // include_resolved should default to false when missing from JSON
        let json = r#"{"file_path": null}"#;
        let params: ListCommentsParams = serde_json::from_str(json).unwrap();
        assert!(!params.include_resolved);
        assert!(params.file_path.is_none());
    }
}
