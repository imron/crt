//! Review data types shared across modules.
//!
//! These types form the shared vocabulary between the git module, database
//! module, server, client, and UI. Each module can produce or consume these
//! types without importing another module's internals.

use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use std::fmt;

pub const ROOT_REVIEW_BASE_REF: &str = "--root";

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
    /// Stable SHA-256 hash of the rendered patch text (algorithm-dependent).
    /// Used for display/cache identity of hunk bodies, not review status.
    pub diff_hash: String,
    /// Algorithm-independent review identity: `v1:{base_blob}:{workdir_blob}`.
    /// See [`encode_content_id`].
    #[serde(default)]
    pub content_id: String,
}

/// Null git blob OID used when a file is missing on one side of a review pair.
pub const NULL_BLOB_OID: &str = "0000000000000000000000000000000000000000";

/// Encode a review content identity from base and workdir (or end-state) blob
/// OIDs. Both sides are git blob object IDs; missing files use
/// [`NULL_BLOB_OID`].
pub fn encode_content_id(base_oid: &str, workdir_oid: &str) -> String {
    format!("v1:{base_oid}:{workdir_oid}")
}

/// True when `value` is a v1 content id (`v1:{base}:{workdir}`).
pub fn is_content_id(value: &str) -> bool {
    parse_content_id(value).is_some()
}

/// Parse a v1 content id into `(base_oid, workdir_oid)`.
pub fn parse_content_id(value: &str) -> Option<(&str, &str)> {
    let rest = value.strip_prefix("v1:")?;
    let (base, workdir) = rest.split_once(':')?;
    if base.is_empty() || workdir.is_empty() || workdir.contains(':') {
        return None;
    }
    Some((base, workdir))
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

/// Compact stats for a file diff without hunk line content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffSummary {
    pub hunks: usize,
    pub additions: usize,
    pub deletions: usize,
    pub is_binary: bool,
    /// Stable SHA-256 hash of the rendered patch text (algorithm-dependent).
    pub diff_hash: String,
    /// Algorithm-independent review identity: `v1:{base_blob}:{workdir_blob}`.
    #[serde(default)]
    pub content_id: String,
}

/// Review state for a changed file without the full diff payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStatusEntry {
    pub change: FileChange,
    pub status: ReviewStatus,
    pub diff: DiffSummary,
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

/// Which side of a review range a comment anchor segment refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentAnchorSide {
    /// Old-side file content at the review merge base.
    Base,
    /// New-side file content at the review head.
    Head,
}

/// Whether a single anchor segment has a usable current placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorPlacementStatus {
    /// The segment is placed at a current file range.
    Anchored,
    /// The segment could not be placed in the target content.
    Orphaned,
}

/// How the latest segment placement was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorMatchMethod {
    /// Exact text match at the stored line number.
    ExactAtLine,
    /// Exact text match at a different line number.
    ExactElsewhere,
    /// Context-assisted match.
    Context,
    /// No match found.
    NotFound,
}

/// Aggregate placement status for all populated segments in one comment
/// anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorAggregateStatus {
    /// All populated segments are anchored.
    Anchored,
    /// At least one segment is anchored and at least one is orphaned.
    Partial,
    /// All populated segments are orphaned.
    Orphaned,
}

/// One side-specific anchor segment for a review comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentAnchorSegment {
    pub side: CommentAnchorSide,
    pub file_path: String,
    pub line_start: i64,
    pub line_end: i64,
    pub char_start: Option<i64>,
    pub char_end: Option<i64>,
    pub anchor_text: String,
    pub context_before: String,
    pub context_after: String,
    pub placement_status: AnchorPlacementStatus,
    pub match_method: AnchorMatchMethod,
}

/// Compound anchor for one logical review comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentAnchor {
    pub segments: Vec<CommentAnchorSegment>,
    pub aggregate_status: AnchorAggregateStatus,
}

/// Invalid compound-anchor shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentAnchorValidationError {
    Empty,
    DuplicateSide(CommentAnchorSide),
    AggregateMismatch {
        expected: AnchorAggregateStatus,
        actual: AnchorAggregateStatus,
    },
}

impl fmt::Display for CommentAnchorValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "comment anchor must have at least one segment"),
            Self::DuplicateSide(side) => {
                write!(f, "comment anchor has duplicate {side:?} segment")
            }
            Self::AggregateMismatch { expected, actual } => write!(
                f,
                "comment anchor aggregate status {actual:?} does not match derived {expected:?}"
            ),
        }
    }
}

impl std::error::Error for CommentAnchorValidationError {}

impl AnchorAggregateStatus {
    pub fn from_segment_placements(
        segments: &[CommentAnchorSegment],
    ) -> Option<AnchorAggregateStatus> {
        if segments.is_empty() {
            return None;
        }

        let anchored = segments
            .iter()
            .filter(|segment| segment.placement_status == AnchorPlacementStatus::Anchored)
            .count();
        if anchored == segments.len() {
            Some(AnchorAggregateStatus::Anchored)
        } else if anchored == 0 {
            Some(AnchorAggregateStatus::Orphaned)
        } else {
            Some(AnchorAggregateStatus::Partial)
        }
    }
}

impl CommentAnchor {
    pub fn try_new(
        segments: Vec<CommentAnchorSegment>,
    ) -> Result<Self, CommentAnchorValidationError> {
        let aggregate_status = Self::validate_segments_and_derive_status(&segments)?
            .unwrap_or(AnchorAggregateStatus::Orphaned);
        Ok(Self {
            segments,
            aggregate_status,
        })
    }

    pub fn try_with_aggregate_status(
        segments: Vec<CommentAnchorSegment>,
        aggregate_status: AnchorAggregateStatus,
    ) -> Result<Self, CommentAnchorValidationError> {
        let expected = Self::validate_segments_and_derive_status(&segments)?
            .unwrap_or(AnchorAggregateStatus::Orphaned);
        if aggregate_status != expected {
            return Err(CommentAnchorValidationError::AggregateMismatch {
                expected,
                actual: aggregate_status,
            });
        }
        Ok(Self {
            segments,
            aggregate_status,
        })
    }

    fn validate_segments_and_derive_status(
        segments: &[CommentAnchorSegment],
    ) -> Result<Option<AnchorAggregateStatus>, CommentAnchorValidationError> {
        if segments.is_empty() {
            return Err(CommentAnchorValidationError::Empty);
        }

        let mut has_base = false;
        let mut has_head = false;
        for segment in segments {
            match segment.side {
                CommentAnchorSide::Base if has_base => {
                    return Err(CommentAnchorValidationError::DuplicateSide(
                        CommentAnchorSide::Base,
                    ));
                }
                CommentAnchorSide::Base => has_base = true,
                CommentAnchorSide::Head if has_head => {
                    return Err(CommentAnchorValidationError::DuplicateSide(
                        CommentAnchorSide::Head,
                    ));
                }
                CommentAnchorSide::Head => has_head = true,
            }
        }

        Ok(AnchorAggregateStatus::from_segment_placements(segments))
    }
}

/// A review comment attached to a specific location in a diff.
#[derive(Debug, Clone, Deserialize)]
pub struct Comment {
    pub id: i64,
    pub merge_base: String,
    pub head_ref: String,
    pub created_head_commit: String,
    anchor: CommentAnchor,
    pub body: String,
    pub resolved: bool,
    pub created_at: String,
    pub updated_at: String,
    /// How well the anchor was resolved (computed at runtime).
    anchor_status: AnchorStatus,
}

impl Serialize for Comment {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("Comment", 18)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("merge_base", &self.merge_base)?;
        state.serialize_field("head_ref", &self.head_ref)?;
        state.serialize_field("created_head_commit", &self.created_head_commit)?;
        state.serialize_field("file_path", self.file_path())?;
        state.serialize_field("line_start", &self.line_start())?;
        state.serialize_field("line_end", &self.line_end())?;
        state.serialize_field("char_start", &self.char_start())?;
        state.serialize_field("char_end", &self.char_end())?;
        state.serialize_field("anchor_text", self.anchor_text())?;
        state.serialize_field("context_before", self.context_before())?;
        state.serialize_field("context_after", self.context_after())?;
        state.serialize_field("anchor", &self.anchor)?;
        state.serialize_field("body", &self.body)?;
        state.serialize_field("resolved", &self.resolved)?;
        state.serialize_field("created_at", &self.created_at)?;
        state.serialize_field("updated_at", &self.updated_at)?;
        state.serialize_field("anchor_status", &self.anchor_status)?;
        state.end()
    }
}

/// Construction data for a review comment.
#[derive(Debug, Clone)]
pub struct CommentInit {
    pub id: i64,
    pub merge_base: String,
    pub head_ref: String,
    pub created_head_commit: String,
    pub anchor: CommentAnchor,
    pub body: String,
    pub resolved: bool,
    pub created_at: String,
    pub updated_at: String,
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
    Comments,
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
    #[serde(default)]
    pub root: bool,
    /// Preferred diff algorithm for server-side review hashing and file diffs.
    /// Older clients omit this and the server falls back to patience.
    #[serde(default)]
    pub diff_algorithm: Option<String>,
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
    pub anchor: CommentAnchor,
    pub body: String,
}

/// Parameters for `create_line_comment`.
///
/// A convenience form of `create_comment` for clients that know a file and a
/// line range but cannot build a full anchor themselves. The server reads the
/// file content for the requested side and derives the anchor text and
/// surrounding context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateLineCommentParams {
    pub file_path: String,
    /// First line of the range, 1-based and inclusive.
    pub line_start: i64,
    /// Last line of the range, 1-based and inclusive. Defaults to
    /// `line_start`.
    pub line_end: Option<i64>,
    /// Which side of the review the line numbers refer to. Defaults to
    /// `Head`.
    pub side: Option<CommentAnchorSide>,
    pub body: String,
}

/// Parameters for `list_comments`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListCommentsParams {
    pub file_path: Option<String>,
    #[serde(default)]
    pub include_resolved: bool,
    #[serde(default)]
    pub include_previous_bases: bool,
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

/// Result of `list_file_statuses`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListFileStatusesResult {
    pub total_files: usize,
    pub reviewed_files: usize,
    pub unreviewed_files: usize,
    pub changed_files: usize,
    pub files: Vec<FileStatusEntry>,
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

/// Parameters for changing the current session merge-base.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetMergeBaseParams {
    pub refspec: String,
}

/// Result of `set_merge_base`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetMergeBaseResult {
    pub context: ConnectionContext,
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
    #[serde(default)]
    pub sessions: Vec<ActiveReviewSession>,
}

/// An initialized review session currently known by the server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveReviewSession {
    pub repo_root: String,
    pub worktree: String,
    pub base_ref: String,
    pub head_ref: String,
    pub merge_base: String,
    pub client_count: usize,
}

// ---------------------------------------------------------------------------
// Conversions from database types
// ---------------------------------------------------------------------------

impl Comment {
    /// Create a comment by projecting its display location from the anchor.
    ///
    /// The head segment is used when present because the head side is the
    /// default list/panel projection. Base-only comments project from their
    /// base segment.
    pub fn new(init: CommentInit) -> Result<Self, CommentAnchorValidationError> {
        CommentAnchor::try_with_aggregate_status(
            init.anchor.segments.clone(),
            init.anchor.aggregate_status,
        )?;
        if projected_comment_segment(&init.anchor).is_none() {
            return Err(CommentAnchorValidationError::Empty);
        }
        Ok(Self {
            id: init.id,
            merge_base: init.merge_base,
            head_ref: init.head_ref,
            created_head_commit: init.created_head_commit,
            anchor: init.anchor,
            body: init.body,
            resolved: init.resolved,
            created_at: init.created_at,
            updated_at: init.updated_at,
            anchor_status: init.anchor_status,
        })
    }

    pub fn anchor(&self) -> &CommentAnchor {
        &self.anchor
    }

    pub fn file_path(&self) -> &str {
        self.projected_segment()
            .map(|segment| segment.file_path.as_str())
            .unwrap_or("")
    }

    pub fn line_start(&self) -> i64 {
        self.projected_segment()
            .map(|segment| segment.line_start)
            .unwrap_or(0)
    }

    pub fn line_end(&self) -> i64 {
        self.projected_segment()
            .map(|segment| segment.line_end)
            .unwrap_or(0)
    }

    pub fn char_start(&self) -> Option<i64> {
        self.projected_segment()
            .and_then(|segment| segment.char_start)
    }

    pub fn char_end(&self) -> Option<i64> {
        self.projected_segment()
            .and_then(|segment| segment.char_end)
    }

    pub fn anchor_text(&self) -> &str {
        self.projected_segment()
            .map(|segment| segment.anchor_text.as_str())
            .unwrap_or("")
    }

    pub fn context_before(&self) -> &str {
        self.projected_segment()
            .map(|segment| segment.context_before.as_str())
            .unwrap_or("")
    }

    pub fn context_after(&self) -> &str {
        self.projected_segment()
            .map(|segment| segment.context_after.as_str())
            .unwrap_or("")
    }

    pub fn anchor_status(&self) -> AnchorStatus {
        self.anchor_status
    }

    pub fn set_anchor_status(&mut self, anchor_status: AnchorStatus) {
        self.anchor_status = anchor_status;
    }

    pub fn replace_anchor(
        &mut self,
        anchor: CommentAnchor,
    ) -> Result<(), CommentAnchorValidationError> {
        CommentAnchor::try_with_aggregate_status(anchor.segments.clone(), anchor.aggregate_status)?;
        if projected_comment_segment(&anchor).is_none() {
            return Err(CommentAnchorValidationError::Empty);
        }
        self.anchor = anchor;
        Ok(())
    }

    /// Create a `Comment` from a `db::StoredComment`.
    ///
    /// This is the bridge between the database layer and the model layer.
    pub fn from_stored(stored: &crate::db::StoredComment) -> Self {
        Self {
            id: stored.id,
            merge_base: stored.merge_base.clone(),
            head_ref: stored.head_ref.clone(),
            created_head_commit: stored.created_head_commit.clone(),
            anchor: stored.anchor.clone(),
            body: stored.body.clone(),
            resolved: stored.resolved,
            created_at: stored.created_at.clone(),
            updated_at: stored.updated_at.clone(),
            anchor_status: stored.anchor_status,
        }
    }

    fn projected_segment(&self) -> Option<&CommentAnchorSegment> {
        projected_comment_segment(&self.anchor)
    }
}

fn projected_comment_segment(anchor: &CommentAnchor) -> Option<&CommentAnchorSegment> {
    anchor
        .segments
        .iter()
        .find(|segment| segment.side == CommentAnchorSide::Head)
        .or_else(|| anchor.segments.first())
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

    fn anchor_segment(
        side: CommentAnchorSide,
        placement_status: AnchorPlacementStatus,
    ) -> CommentAnchorSegment {
        CommentAnchorSegment {
            side,
            file_path: "src/lib.rs".to_string(),
            line_start: 10,
            line_end: 12,
            char_start: None,
            char_end: None,
            anchor_text: "fn foo() {}".to_string(),
            context_before: "// before".to_string(),
            context_after: "// after".to_string(),
            placement_status,
            match_method: match placement_status {
                AnchorPlacementStatus::Anchored => AnchorMatchMethod::ExactAtLine,
                AnchorPlacementStatus::Orphaned => AnchorMatchMethod::NotFound,
            },
        }
    }

    #[test]
    fn test_compound_anchor_enum_serialization() {
        assert_eq!(
            serde_json::to_string(&CommentAnchorSide::Base).unwrap(),
            "\"base\""
        );
        assert_eq!(
            serde_json::to_string(&CommentAnchorSide::Head).unwrap(),
            "\"head\""
        );
        assert_eq!(
            serde_json::to_string(&AnchorPlacementStatus::Anchored).unwrap(),
            "\"anchored\""
        );
        assert_eq!(
            serde_json::to_string(&AnchorPlacementStatus::Orphaned).unwrap(),
            "\"orphaned\""
        );
        assert_eq!(
            serde_json::to_string(&AnchorMatchMethod::ExactAtLine).unwrap(),
            "\"exact_at_line\""
        );
        assert_eq!(
            serde_json::to_string(&AnchorMatchMethod::ExactElsewhere).unwrap(),
            "\"exact_elsewhere\""
        );
        assert_eq!(
            serde_json::to_string(&AnchorMatchMethod::Context).unwrap(),
            "\"context\""
        );
        assert_eq!(
            serde_json::to_string(&AnchorMatchMethod::NotFound).unwrap(),
            "\"not_found\""
        );
        assert_eq!(
            serde_json::to_string(&AnchorAggregateStatus::Partial).unwrap(),
            "\"partial\""
        );
    }

    #[test]
    fn test_compound_anchor_shape_roundtrips() {
        let base_only = CommentAnchor::try_new(vec![anchor_segment(
            CommentAnchorSide::Base,
            AnchorPlacementStatus::Anchored,
        )])
        .unwrap();
        assert_eq!(base_only.aggregate_status, AnchorAggregateStatus::Anchored);

        let head_only = CommentAnchor::try_new(vec![anchor_segment(
            CommentAnchorSide::Head,
            AnchorPlacementStatus::Orphaned,
        )])
        .unwrap();
        assert_eq!(head_only.aggregate_status, AnchorAggregateStatus::Orphaned);

        let paired = CommentAnchor::try_new(vec![
            anchor_segment(CommentAnchorSide::Base, AnchorPlacementStatus::Anchored),
            anchor_segment(CommentAnchorSide::Head, AnchorPlacementStatus::Orphaned),
        ])
        .unwrap();
        assert_eq!(paired.aggregate_status, AnchorAggregateStatus::Partial);

        let json = serde_json::to_string(&paired).unwrap();
        let roundtrip: CommentAnchor = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip, paired);
    }

    #[test]
    fn test_compound_anchor_rejects_impossible_shapes() {
        let empty = CommentAnchor::try_new(Vec::new()).unwrap_err();
        assert_eq!(empty, CommentAnchorValidationError::Empty);

        let duplicate = CommentAnchor::try_new(vec![
            anchor_segment(CommentAnchorSide::Head, AnchorPlacementStatus::Anchored),
            anchor_segment(CommentAnchorSide::Head, AnchorPlacementStatus::Anchored),
        ])
        .unwrap_err();
        assert_eq!(
            duplicate,
            CommentAnchorValidationError::DuplicateSide(CommentAnchorSide::Head)
        );

        let mismatch = CommentAnchor::try_with_aggregate_status(
            vec![anchor_segment(
                CommentAnchorSide::Base,
                AnchorPlacementStatus::Anchored,
            )],
            AnchorAggregateStatus::Orphaned,
        )
        .unwrap_err();
        assert_eq!(
            mismatch,
            CommentAnchorValidationError::AggregateMismatch {
                expected: AnchorAggregateStatus::Anchored,
                actual: AnchorAggregateStatus::Orphaned,
            }
        );
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
                content_id: String::new(),
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
            created_head_commit: "head-commit".to_string(),
            anchor: CommentAnchor {
                segments: vec![CommentAnchorSegment {
                    side: CommentAnchorSide::Head,
                    file_path: "src/lib.rs".to_string(),
                    line_start: 10,
                    line_end: 12,
                    char_start: None,
                    char_end: None,
                    anchor_text: "fn foo() {}".to_string(),
                    context_before: "// before".to_string(),
                    context_after: "// after".to_string(),
                    placement_status: AnchorPlacementStatus::Anchored,
                    match_method: AnchorMatchMethod::ExactAtLine,
                }],
                aggregate_status: AnchorAggregateStatus::Anchored,
            },
            file_path: "stale.rs".to_string(),
            line_start: 1,
            line_end: 1,
            char_start: Some(99),
            char_end: Some(100),
            anchor_text: "stale".to_string(),
            context_before: "stale before".to_string(),
            context_after: "stale after".to_string(),
            body: "Fix this".to_string(),
            resolved: false,
            created_at: "2026-03-29T14:00:00+10:00".to_string(),
            updated_at: "2026-03-29T14:00:00+10:00".to_string(),
            file_blob_sha: "blob-1".to_string(),
            anchor_status: AnchorStatus::Anchored,
        };

        let comment = Comment::from_stored(&stored);

        assert_eq!(comment.id, 42);
        assert_eq!(comment.file_path(), "src/lib.rs");
        assert_eq!(comment.line_start(), 10);
        assert_eq!(comment.line_end(), 12);
        assert_eq!(comment.char_start(), None);
        assert_eq!(comment.char_end(), None);
        assert_eq!(comment.anchor_text(), "fn foo() {}");
        assert_eq!(comment.context_before(), "// before");
        assert_eq!(comment.context_after(), "// after");
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
            root: false,
            diff_algorithm: None,
        };
        let json = serde_json::to_string(&params).unwrap();
        let deserialized: InitParams = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.base_ref, "main");

        let params = ListCommentsParams {
            file_path: Some("src/lib.rs".to_string()),
            include_resolved: true,
            include_previous_bases: false,
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
        assert!(!params.include_previous_bases);
        assert!(params.file_path.is_none());
    }
}
