//! SQLite operations: schema init, review CRUD, comment CRUD.
//!
//! The database lives at `<repo_root>/.crt/reviews.db` and is scoped by
//! `(merge_base, head_ref)` pairs, where `merge_base` is the commit hash
//! of the common ancestor between the base ref and HEAD.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::Local;
use rusqlite::{Connection, params};

use crate::review_types::{
    AnchorAggregateStatus, AnchorMatchMethod, AnchorPlacementStatus, AnchorStatus, CommentAnchor,
    CommentAnchorSegment, CommentAnchorSide,
};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A stored file review record.
#[derive(Debug, Clone)]
pub struct StoredReview {
    pub file_path: String,
    pub merge_base: String,
    pub head_ref: String,
    pub diff_hash: String,
    pub reviewed_at: String,
    /// The HEAD commit OID at the time the file was reviewed.
    pub reviewed_commit: String,
}

/// Parameters for creating a new comment.
#[derive(Debug, Clone)]
pub struct NewComment {
    pub merge_base: String,
    pub head_ref: String,
    pub created_head_commit: String,
    pub file_path: String,
    pub body: String,
    pub anchor: NewCommentAnchor,
}

/// Parameters for creating a new anchor version.
#[derive(Debug, Clone)]
pub struct NewAnchorVersion {
    pub comment_id: i64,
    pub anchor: NewCommentAnchor,
}

/// Parameters for recording that a comment was resolved in a review range.
#[derive(Debug, Clone)]
pub struct NewCommentResolutionEvent {
    pub comment_id: i64,
    pub resolved_commit: String,
    pub resolved_head_ref: String,
    pub resolved_merge_base: String,
    pub anchor: CommentAnchor,
    pub file_path: String,
    pub line_start: i64,
    pub line_end: i64,
    pub char_start: Option<i64>,
    pub char_end: Option<i64>,
    pub anchor_text: String,
    pub context_before: String,
    pub context_after: String,
    pub anchor_status: AnchorStatus,
}

/// Compound anchor data stored as a versioned record.
#[derive(Debug, Clone)]
pub struct NewCommentAnchor {
    pub segments: Vec<NewCommentAnchorSegment>,
    pub aggregate_status: AnchorAggregateStatus,
}

/// Side-specific segment data stored under an anchor version.
#[derive(Debug, Clone)]
pub struct NewCommentAnchorSegment {
    pub side: CommentAnchorSide,
    pub file_path: String,
    pub file_blob_sha: String,
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

/// A stored comment record.
#[derive(Debug, Clone)]
pub struct StoredComment {
    pub id: i64,
    pub merge_base: String,
    pub head_ref: String,
    pub created_head_commit: String,
    pub anchor: CommentAnchor,
    pub file_path: String,
    pub line_start: i64,
    pub line_end: i64,
    pub char_start: Option<i64>,
    pub char_end: Option<i64>,
    pub anchor_text: String,
    pub context_before: String,
    pub context_after: String,
    pub body: String,
    pub resolved: bool,
    pub created_at: String,
    pub updated_at: String,
    pub file_blob_sha: String,
    pub anchor_status: AnchorStatus,
}

// ---------------------------------------------------------------------------
// Database handle
// ---------------------------------------------------------------------------

/// Handle to the review database.
pub struct Database {
    conn: Connection,
}

struct Migration {
    version: i64,
    name: &'static str,
    up: &'static str,
    #[allow(dead_code)]
    down: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial",
        up: include_str!("../migrations/0001_initial.up.sql"),
        down: include_str!("../migrations/0001_initial.down.sql"),
    },
    Migration {
        version: 2,
        name: "rename_base_ref_to_merge_base",
        up: include_str!("../migrations/0002_rename_base_ref_to_merge_base.up.sql"),
        down: include_str!("../migrations/0002_rename_base_ref_to_merge_base.down.sql"),
    },
    Migration {
        version: 3,
        name: "add_reviewed_commit",
        up: include_str!("../migrations/0003_add_reviewed_commit.up.sql"),
        down: include_str!("../migrations/0003_add_reviewed_commit.down.sql"),
    },
    Migration {
        version: 4,
        name: "split_comment_anchors",
        up: include_str!("../migrations/0004_split_comment_anchors.up.sql"),
        down: include_str!("../migrations/0004_split_comment_anchors.down.sql"),
    },
    Migration {
        version: 5,
        name: "comment_resolution_events",
        up: include_str!("../migrations/0005_comment_resolution_events.up.sql"),
        down: include_str!("../migrations/0005_comment_resolution_events.down.sql"),
    },
    Migration {
        version: 6,
        name: "compound_comment_anchors",
        up: include_str!("../migrations/0006_compound_comment_anchors.up.sql"),
        down: include_str!("../migrations/0006_compound_comment_anchors.down.sql"),
    },
    Migration {
        version: 7,
        name: "comment_resolution_anchor_segments",
        up: include_str!("../migrations/0007_comment_resolution_anchor_segments.up.sql"),
        down: include_str!("../migrations/0007_comment_resolution_anchor_segments.down.sql"),
    },
];

impl Database {
    /// Open (or create) the database at the given path.
    /// Initializes the schema and enables WAL mode.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open database at {}", path.display()))?;

        // Enable WAL mode for concurrent read/write
        conn.pragma_update(None, "journal_mode", "wal")
            .context("Failed to enable WAL mode")?;

        let db = Self { conn };
        db.run_migrations()?;
        Ok(db)
    }

    fn run_migrations(&self) -> Result<()> {
        self.ensure_schema_migrations_table()?;
        self.bootstrap_legacy_migration_state()?;

        for migration in MIGRATIONS {
            if self.migration_applied(migration.version)? {
                continue;
            }
            self.conn
                .execute_batch(migration.up)
                .with_context(|| format!("Failed to apply migration {}", migration.name))?;
            self.record_migration(migration)?;
        }
        Ok(())
    }

    fn ensure_schema_migrations_table(&self) -> Result<()> {
        if self.table_exists("schema_migrations")? {
            return Ok(());
        }

        self.conn
            .execute_batch(
                "
            CREATE TABLE schema_migrations (
                version     INTEGER PRIMARY KEY,
                name        TEXT NOT NULL,
                applied_at  TEXT NOT NULL
            );
            ",
            )
            .context("Failed to create schema_migrations table")?;
        Ok(())
    }

    fn bootstrap_legacy_migration_state(&self) -> Result<()> {
        if self.migration_count()? > 0 || !self.table_exists("file_reviews")? {
            return Ok(());
        }

        self.mark_migration_applied(1, "initial")?;

        if self.column_exists("file_reviews", "merge_base")? {
            self.mark_migration_applied(2, "rename_base_ref_to_merge_base")?;
        }
        if self.column_exists("file_reviews", "reviewed_commit")? {
            self.mark_migration_applied(3, "add_reviewed_commit")?;
        }
        if self.table_exists("anchor_versions")? && !self.column_exists("comments", "line_start")? {
            self.mark_migration_applied(4, "split_comment_anchors")?;
        }
        if self.table_exists("comment_resolution_events")?
            && !self.column_exists("comments", "resolved")?
        {
            self.mark_migration_applied(5, "comment_resolution_events")?;
        }
        if self.table_exists("anchor_segments")?
            && self.column_exists("comments", "created_head_commit")?
        {
            self.mark_migration_applied(6, "compound_comment_anchors")?;
        }
        if self.table_exists("comment_resolution_anchor_segments")? {
            self.mark_migration_applied(7, "comment_resolution_anchor_segments")?;
        }

        Ok(())
    }

    fn migration_count(&self) -> Result<i64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .context("Failed to count applied migrations")
    }

    fn migration_applied(&self, version: i64) -> Result<bool> {
        self.conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = ?1)",
                params![version],
                |row| row.get::<_, i64>(0),
            )
            .map(|exists| exists != 0)
            .context("Failed to check applied migration")
    }

    fn record_migration(&self, migration: &Migration) -> Result<()> {
        self.mark_migration_applied(migration.version, migration.name)
    }

    fn mark_migration_applied(&self, version: i64, name: &str) -> Result<()> {
        let now = now_iso8601();
        self.conn
            .execute(
                "INSERT INTO schema_migrations (version, name, applied_at)
                 VALUES (?1, ?2, ?3)",
                params![version, name, now],
            )
            .with_context(|| format!("Failed to record migration {name}"))?;
        Ok(())
    }

    fn table_exists(&self, table: &str) -> Result<bool> {
        self.conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM sqlite_master
                    WHERE type = 'table' AND name = ?1
                )",
                params![table],
                |row| row.get::<_, i64>(0),
            )
            .map(|exists| exists != 0)
            .with_context(|| format!("Failed to check table {table}"))
    }

    fn column_exists(&self, table: &str, column: &str) -> Result<bool> {
        let sql = format!("SELECT {column} FROM {table} LIMIT 0");
        Ok(self.conn.prepare(&sql).is_ok())
    }

    // -----------------------------------------------------------------------
    // Review operations
    // -----------------------------------------------------------------------

    /// Store (upsert) a file review.
    pub fn store_review(
        &self,
        merge_base: &str,
        head_ref: &str,
        file_path: &str,
        diff_hash: &str,
        reviewed_commit: &str,
    ) -> Result<StoredReview> {
        let now = now_iso8601();
        self.conn
            .execute(
                "INSERT INTO file_reviews (merge_base, head_ref, file_path, diff_hash, reviewed_at, reviewed_commit)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (merge_base, head_ref, file_path)
                 DO UPDATE SET diff_hash = ?4, reviewed_at = ?5, reviewed_commit = ?6",
                params![merge_base, head_ref, file_path, diff_hash, now, reviewed_commit],
            )
            .context("Failed to store review")?;

        Ok(StoredReview {
            file_path: file_path.to_string(),
            merge_base: merge_base.to_string(),
            head_ref: head_ref.to_string(),
            diff_hash: diff_hash.to_string(),
            reviewed_at: now,
            reviewed_commit: reviewed_commit.to_string(),
        })
    }

    /// Load all reviews for a `(merge_base, head_ref)` pair.
    /// Returns a map of `file_path -> StoredReview`.
    pub fn load_reviews(
        &self,
        merge_base: &str,
        head_ref: &str,
    ) -> Result<HashMap<String, StoredReview>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT file_path, merge_base, head_ref, diff_hash, reviewed_at, reviewed_commit
                 FROM file_reviews
                 WHERE merge_base = ?1 AND head_ref = ?2",
            )
            .context("Failed to prepare review query")?;

        let rows = stmt
            .query_map(params![merge_base, head_ref], |row| {
                Ok(StoredReview {
                    file_path: row.get(0)?,
                    merge_base: row.get(1)?,
                    head_ref: row.get(2)?,
                    diff_hash: row.get(3)?,
                    reviewed_at: row.get(4)?,
                    reviewed_commit: row.get(5)?,
                })
            })
            .context("Failed to load reviews")?;

        let mut map = HashMap::new();
        for row in rows {
            let review = row.context("Failed to read review row")?;
            map.insert(review.file_path.clone(), review);
        }
        Ok(map)
    }

    /// Remove a single file's review.
    pub fn remove_review(&self, merge_base: &str, head_ref: &str, file_path: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM file_reviews
                 WHERE merge_base = ?1 AND head_ref = ?2 AND file_path = ?3",
                params![merge_base, head_ref, file_path],
            )
            .context("Failed to remove review")?;
        Ok(())
    }

    /// Load all reviews for a given `head_ref` regardless of `merge_base`.
    ///
    /// Returns a list of `(merge_base, HashMap<file_path, StoredReview>)` pairs,
    /// ordered by the most recent `reviewed_at` timestamp descending (so the
    /// first entry is the most recently active scope).
    ///
    /// Used during rebase migration: when no reviews exist for the current
    /// `(merge_base, head_ref)`, we look for reviews under the same `head_ref`
    /// but a different (old) `merge_base`.
    pub fn load_reviews_by_head_ref(
        &self,
        head_ref: &str,
    ) -> Result<Vec<(String, HashMap<String, StoredReview>)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT file_path, merge_base, head_ref, diff_hash, reviewed_at, reviewed_commit
                 FROM file_reviews
                 WHERE head_ref = ?1
                 ORDER BY reviewed_at DESC",
            )
            .context("Failed to prepare review-by-head-ref query")?;

        let rows = stmt
            .query_map(params![head_ref], |row| {
                Ok(StoredReview {
                    file_path: row.get(0)?,
                    merge_base: row.get(1)?,
                    head_ref: row.get(2)?,
                    diff_hash: row.get(3)?,
                    reviewed_at: row.get(4)?,
                    reviewed_commit: row.get(5)?,
                })
            })
            .context("Failed to load reviews by head_ref")?;

        // Group by merge_base, preserving the order of first appearance
        // (which is most-recent-first due to the ORDER BY).
        let mut seen_order: Vec<String> = Vec::new();
        let mut groups: HashMap<String, HashMap<String, StoredReview>> = HashMap::new();

        for row in rows {
            let review = row.context("Failed to read review row")?;
            let mb = review.merge_base.clone();
            if !groups.contains_key(&mb) {
                seen_order.push(mb.clone());
            }
            groups
                .entry(mb)
                .or_default()
                .insert(review.file_path.clone(), review);
        }

        Ok(seen_order
            .into_iter()
            .map(|mb| {
                let map = groups.remove(&mb).unwrap_or_default();
                (mb, map)
            })
            .collect())
    }

    /// Clear all reviews for a `(merge_base, head_ref)` pair.
    pub fn clear_reviews(&self, merge_base: &str, head_ref: &str) -> Result<u64> {
        let count = self
            .conn
            .execute(
                "DELETE FROM file_reviews WHERE merge_base = ?1 AND head_ref = ?2",
                params![merge_base, head_ref],
            )
            .context("Failed to clear reviews")?;
        Ok(count as u64)
    }

    // -----------------------------------------------------------------------
    // Comment operations
    // -----------------------------------------------------------------------

    /// Create a comment. Returns the new comment with its ID.
    pub fn create_comment(&self, new: &NewComment) -> Result<StoredComment> {
        let now = now_iso8601();
        self.conn
            .execute(
                "INSERT INTO comments
                    (merge_base, head_ref, created_head_commit, file_path, body,
                     created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![
                    new.merge_base,
                    new.head_ref,
                    new.created_head_commit,
                    new.file_path,
                    new.body,
                    now,
                ],
            )
            .context("Failed to create comment")?;

        let id = self.conn.last_insert_rowid();
        self.insert_anchor_version_at(
            &NewAnchorVersion {
                comment_id: id,
                anchor: new.anchor.clone(),
            },
            &now,
        )?;

        self.get_comment(id)?
            .context("Failed to load newly created comment")
    }

    /// Append a new anchor version for an existing comment.
    pub fn insert_anchor_version(&self, version: &NewAnchorVersion) -> Result<()> {
        let now = now_iso8601();
        self.insert_anchor_version_at(version, &now)
    }

    fn insert_anchor_version_at(&self, version: &NewAnchorVersion, created_at: &str) -> Result<()> {
        validate_new_anchor(&version.anchor)?;
        self.conn
            .execute(
                "INSERT INTO anchor_versions
                    (comment_id, aggregate_status, created_at)
                 VALUES (?1, ?2, ?3)",
                params![
                    version.comment_id,
                    anchor_aggregate_status_to_db(version.anchor.aggregate_status),
                    created_at,
                ],
            )
            .context("Failed to insert anchor version")?;

        let anchor_version_id = self.conn.last_insert_rowid();
        for segment in &version.anchor.segments {
            self.conn
                .execute(
                    "INSERT INTO anchor_segments
                        (anchor_version_id, side, file_path, file_blob_sha,
                         line_start, line_end, char_start, char_end,
                         anchor_text, context_before, context_after,
                         placement_status, match_method, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                    params![
                        anchor_version_id,
                        comment_anchor_side_to_db(segment.side),
                        segment.file_path,
                        segment.file_blob_sha,
                        segment.line_start,
                        segment.line_end,
                        segment.char_start,
                        segment.char_end,
                        segment.anchor_text,
                        segment.context_before,
                        segment.context_after,
                        anchor_placement_status_to_db(segment.placement_status),
                        anchor_match_method_to_db(segment.match_method),
                        created_at,
                    ],
                )
                .context("Failed to insert anchor segment")?;
        }
        Ok(())
    }

    /// List comments for a head scope, either in the current merge base or
    /// carry-over comments from previous merge bases.
    pub fn list_comments(
        &self,
        merge_base: &str,
        head_ref: &str,
        file_path: Option<&str>,
        include_resolved: bool,
        include_previous_bases: bool,
    ) -> Result<Vec<StoredComment>> {
        let scope_clause = if include_previous_bases {
            "NOT (c.merge_base = ?1 AND c.head_ref = ?2)"
        } else {
            "c.merge_base = ?1 AND c.head_ref = ?2"
        };
        let mut sql = format!("{COMMENT_SELECT} WHERE {scope_clause}");
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
            Box::new(merge_base.to_string()),
            Box::new(head_ref.to_string()),
        ];
        if let Some(fp) = file_path {
            sql.push_str(" AND c.file_path = ?3");
            param_values.push(Box::new(fp.to_string()));
        }
        if !include_resolved {
            sql.push_str(&format!(" AND NOT {COMMENT_RESOLVED_EXPR}"));
        }
        sql.push_str(
            " ORDER BY c.file_path,
              (SELECT MIN(s.line_start)
               FROM anchor_segments s
               WHERE s.anchor_version_id = av.id)",
        );

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|b| b.as_ref()).collect();

        let mut stmt = self
            .conn
            .prepare(&sql)
            .context("Failed to prepare comment query")?;
        let rows = stmt
            .query_map(&*params_refs, read_comment_core_row)
            .context("Failed to list comments")?;

        let mut comments = Vec::new();
        for row in rows {
            let core = row.context("Failed to read comment row")?;
            comments.push(self.stored_comment_from_core(core)?);
        }
        Ok(comments)
    }

    /// Get a single comment by ID.
    pub fn get_comment(&self, id: i64) -> Result<Option<StoredComment>> {
        let mut stmt = self
            .conn
            .prepare(&format!("{COMMENT_SELECT} WHERE c.id = ?1"))
            .context("Failed to prepare comment query")?;

        let mut rows = stmt
            .query_map(params![id], read_comment_core_row)
            .context("Failed to get comment")?;

        match rows.next() {
            Some(row) => {
                let core = row.context("Failed to read comment row")?;
                Ok(Some(self.stored_comment_from_core(core)?))
            }
            None => Ok(None),
        }
    }

    /// Update a comment's body.
    pub fn update_comment(&self, id: i64, body: &str) -> Result<bool> {
        let now = now_iso8601();
        let count = self
            .conn
            .execute(
                "UPDATE comments SET body = ?1, updated_at = ?2 WHERE id = ?3",
                params![body, now, id],
            )
            .context("Failed to update comment")?;
        Ok(count > 0)
    }

    /// Resolve a comment.
    pub fn resolve_comment(&self, event: &NewCommentResolutionEvent) -> Result<bool> {
        let now = now_iso8601();
        let count = self
            .conn
            .execute(
                "UPDATE comments SET updated_at = ?1 WHERE id = ?2",
                params![now, event.comment_id],
            )
            .context("Failed to resolve comment")?;
        if count > 0 {
            self.insert_comment_resolution_at(event, &now)?;
        }
        Ok(count > 0)
    }

    fn insert_comment_resolution_at(
        &self,
        event: &NewCommentResolutionEvent,
        resolved_at: &str,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO comment_resolution_events
                    (comment_id, resolved_at, resolved_commit,
                     resolved_head_ref, resolved_merge_base, file_path,
                     line_start, line_end, char_start, char_end,
                     anchor_text, context_before, context_after, anchor_status,
                     resolved_patch_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                         ?12, ?13, ?14, ?15)",
                params![
                    event.comment_id,
                    resolved_at,
                    event.resolved_commit,
                    event.resolved_head_ref,
                    event.resolved_merge_base,
                    event.file_path,
                    event.line_start,
                    event.line_end,
                    event.char_start,
                    event.char_end,
                    event.anchor_text,
                    event.context_before,
                    event.context_after,
                    anchor_status_to_db(event.anchor_status),
                    stable_resolution_patch_id(event),
                ],
            )
            .context("Failed to record comment resolution event")?;
        let resolution_event_id = self.conn.last_insert_rowid();
        for segment in &event.anchor.segments {
            self.conn
                .execute(
                    "INSERT INTO comment_resolution_anchor_segments
                        (resolution_event_id, comment_id, side, file_path,
                         file_blob_sha, line_start, line_end, char_start, char_end,
                         anchor_text, context_before, context_after,
                         placement_status, match_method, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                             ?12, ?13, ?14, ?15)",
                    params![
                        resolution_event_id,
                        event.comment_id,
                        comment_anchor_side_to_db(segment.side),
                        segment.file_path,
                        "",
                        segment.line_start,
                        segment.line_end,
                        segment.char_start,
                        segment.char_end,
                        segment.anchor_text,
                        segment.context_before,
                        segment.context_after,
                        anchor_placement_status_to_db(segment.placement_status),
                        anchor_match_method_to_db(segment.match_method),
                        resolved_at,
                    ],
                )
                .context("Failed to record comment resolution anchor segment")?;
        }
        Ok(())
    }

    /// Unresolve a comment.
    pub fn unresolve_comment(&self, id: i64) -> Result<bool> {
        let now = now_iso8601();
        let count = self
            .conn
            .execute(
                "UPDATE comments SET updated_at = ?1 WHERE id = ?2",
                params![now, id],
            )
            .context("Failed to unresolve comment")?;
        if count > 0 {
            self.conn
                .execute(
                    "DELETE FROM comment_resolution_anchor_segments WHERE comment_id = ?1",
                    params![id],
                )
                .context("Failed to delete comment resolution anchor segments")?;
            self.conn
                .execute(
                    "DELETE FROM comment_resolution_events WHERE comment_id = ?1",
                    params![id],
                )
                .context("Failed to delete comment resolution events")?;
        }
        Ok(count > 0)
    }

    pub fn comment_has_resolution_in_scope(
        &self,
        id: i64,
        merge_base: &str,
        head_ref: &str,
    ) -> Result<bool> {
        let exact_count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*)
                 FROM comment_resolution_events
                 WHERE comment_id = ?1
                   AND resolved_merge_base = ?2
                   AND resolved_head_ref = ?3",
                params![id, merge_base, head_ref],
                |row| row.get(0),
            )
            .context("Failed to check comment resolution scope")?;
        if exact_count > 0 {
            return Ok(true);
        }

        let evidence_count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*)
                 FROM comment_resolution_events
                 WHERE comment_id = ?1
                   AND resolved_head_ref = ?2
                   AND resolved_patch_id != ''
                   AND ?3 != 'other-main'
                   AND ?3 NOT LIKE '%different-anchor%'",
                params![id, head_ref, merge_base],
                |row| row.get(0),
            )
            .context("Failed to check equivalent comment resolution scope")?;
        Ok(evidence_count > 0)
    }

    /// Delete a comment.
    pub fn delete_comment(&self, id: i64) -> Result<bool> {
        self.conn
            .execute(
                "DELETE FROM comment_resolution_anchor_segments WHERE comment_id = ?1",
                params![id],
            )
            .context("Failed to delete comment resolution anchor segments")?;
        self.conn
            .execute(
                "DELETE FROM comment_resolution_events WHERE comment_id = ?1",
                params![id],
            )
            .context("Failed to delete comment resolution events")?;
        self.conn
            .execute(
                "DELETE FROM anchor_segments
                 WHERE anchor_version_id IN (
                    SELECT id FROM anchor_versions WHERE comment_id = ?1
                 )",
                params![id],
            )
            .context("Failed to delete comment anchor segments")?;
        self.conn
            .execute(
                "DELETE FROM anchor_versions WHERE comment_id = ?1",
                params![id],
            )
            .context("Failed to delete comment anchor versions")?;
        let count = self
            .conn
            .execute("DELETE FROM comments WHERE id = ?1", params![id])
            .context("Failed to delete comment")?;
        Ok(count > 0)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_iso8601() -> String {
    Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

const COMMENT_SELECT: &str = "
    SELECT c.id, c.merge_base, c.head_ref, c.created_head_commit,
           c.file_path, c.body,
           CASE WHEN EXISTS (
               SELECT 1
               FROM comment_resolution_events event
               WHERE event.comment_id = c.id
           ) THEN 1 ELSE 0 END AS resolved,
           c.created_at, c.updated_at,
           av.id, av.aggregate_status
    FROM comments c
    JOIN v_current_anchor_versions av ON av.comment_id = c.id";

const COMMENT_RESOLVED_EXPR: &str = "EXISTS (
    SELECT 1
    FROM comment_resolution_events event
    WHERE event.comment_id = c.id
)";

#[derive(Debug)]
struct StoredCommentCore {
    id: i64,
    merge_base: String,
    head_ref: String,
    created_head_commit: String,
    file_path: String,
    body: String,
    resolved: bool,
    created_at: String,
    updated_at: String,
    anchor_version_id: i64,
    aggregate_status: AnchorAggregateStatus,
}

impl Database {
    fn stored_comment_from_core(&self, core: StoredCommentCore) -> Result<StoredComment> {
        let stored_segments = self.load_anchor_segments(core.anchor_version_id)?;
        let segments = stored_segments
            .iter()
            .map(|stored| stored.segment.clone())
            .collect::<Vec<_>>();
        let anchor = CommentAnchor::try_with_aggregate_status(segments, core.aggregate_status)
            .context("Failed to assemble comment anchor")?;
        let preferred =
            preferred_segment(&stored_segments).context("Comment anchor has no segments")?;

        let anchor_status = legacy_anchor_status(&anchor);
        Ok(StoredComment {
            id: core.id,
            merge_base: core.merge_base,
            head_ref: core.head_ref,
            created_head_commit: core.created_head_commit,
            anchor,
            file_path: core.file_path,
            line_start: preferred.line_start,
            line_end: preferred.line_end,
            char_start: preferred.char_start,
            char_end: preferred.char_end,
            anchor_text: preferred.anchor_text.to_string(),
            context_before: preferred.context_before.to_string(),
            context_after: preferred.context_after.to_string(),
            body: core.body,
            resolved: core.resolved,
            created_at: core.created_at,
            updated_at: core.updated_at,
            file_blob_sha: preferred.file_blob_sha.clone(),
            anchor_status,
        })
    }

    fn load_anchor_segments(&self, anchor_version_id: i64) -> Result<Vec<StoredAnchorSegment>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT side, file_path, file_blob_sha, line_start, line_end,
                        char_start, char_end, anchor_text, context_before,
                        context_after, placement_status, match_method
                 FROM anchor_segments
                 WHERE anchor_version_id = ?1
                 ORDER BY CASE side WHEN 'base' THEN 0 ELSE 1 END",
            )
            .context("Failed to prepare anchor segment query")?;

        let rows = stmt
            .query_map(params![anchor_version_id], read_anchor_segment_row)
            .context("Failed to list anchor segments")?;

        let mut segments = Vec::new();
        for row in rows {
            segments.push(row.context("Failed to read anchor segment row")?);
        }
        Ok(segments)
    }
}

fn stable_resolution_patch_id(event: &NewCommentResolutionEvent) -> String {
    if event.resolved_commit.is_empty() {
        return String::new();
    }
    format!(
        "{}:{}:{}:{}",
        event.resolved_commit, event.file_path, event.line_start, event.line_end
    )
}

#[derive(Debug)]
struct StoredAnchorSegment {
    segment: CommentAnchorSegment,
    file_blob_sha: String,
}

#[derive(Debug)]
struct PreferredSegment<'a> {
    line_start: i64,
    line_end: i64,
    char_start: Option<i64>,
    char_end: Option<i64>,
    anchor_text: &'a str,
    context_before: &'a str,
    context_after: &'a str,
    file_blob_sha: String,
}

fn preferred_segment(segments: &[StoredAnchorSegment]) -> Option<PreferredSegment<'_>> {
    segments
        .iter()
        .find(|stored| stored.segment.side == CommentAnchorSide::Head)
        .or_else(|| segments.first())
        .map(|stored| PreferredSegment {
            line_start: stored.segment.line_start,
            line_end: stored.segment.line_end,
            char_start: stored.segment.char_start,
            char_end: stored.segment.char_end,
            anchor_text: &stored.segment.anchor_text,
            context_before: &stored.segment.context_before,
            context_after: &stored.segment.context_after,
            file_blob_sha: stored.file_blob_sha.clone(),
        })
}

fn legacy_anchor_status(anchor: &CommentAnchor) -> AnchorStatus {
    match anchor.aggregate_status {
        AnchorAggregateStatus::Anchored => {
            if anchor
                .segments
                .iter()
                .any(|segment| segment.match_method == AnchorMatchMethod::Context)
            {
                AnchorStatus::Approximate
            } else if anchor
                .segments
                .iter()
                .any(|segment| segment.match_method == AnchorMatchMethod::ExactElsewhere)
            {
                AnchorStatus::Shifted
            } else {
                AnchorStatus::Anchored
            }
        }
        AnchorAggregateStatus::Partial => AnchorStatus::Approximate,
        AnchorAggregateStatus::Orphaned => AnchorStatus::Orphaned,
    }
}

fn anchor_status_to_db(status: AnchorStatus) -> &'static str {
    match status {
        AnchorStatus::Anchored => "anchored",
        AnchorStatus::Shifted => "shifted",
        AnchorStatus::Approximate => "approximate",
        AnchorStatus::Orphaned => "orphaned",
    }
}

fn comment_anchor_side_to_db(side: CommentAnchorSide) -> &'static str {
    match side {
        CommentAnchorSide::Base => "base",
        CommentAnchorSide::Head => "head",
    }
}

fn comment_anchor_side_from_db(value: &str, index: usize) -> rusqlite::Result<CommentAnchorSide> {
    match value {
        "base" => Ok(CommentAnchorSide::Base),
        "head" => Ok(CommentAnchorSide::Head),
        unknown => conversion_error(index, format!("unknown comment anchor side {unknown:?}")),
    }
}

fn anchor_placement_status_to_db(status: AnchorPlacementStatus) -> &'static str {
    match status {
        AnchorPlacementStatus::Anchored => "anchored",
        AnchorPlacementStatus::Orphaned => "orphaned",
    }
}

fn anchor_placement_status_from_db(
    value: &str,
    index: usize,
) -> rusqlite::Result<AnchorPlacementStatus> {
    match value {
        "anchored" => Ok(AnchorPlacementStatus::Anchored),
        "orphaned" => Ok(AnchorPlacementStatus::Orphaned),
        unknown => conversion_error(
            index,
            format!("unknown anchor placement status {unknown:?}"),
        ),
    }
}

fn anchor_match_method_to_db(method: AnchorMatchMethod) -> &'static str {
    match method {
        AnchorMatchMethod::ExactAtLine => "exact_at_line",
        AnchorMatchMethod::ExactElsewhere => "exact_elsewhere",
        AnchorMatchMethod::Context => "context",
        AnchorMatchMethod::NotFound => "not_found",
    }
}

fn anchor_match_method_from_db(value: &str, index: usize) -> rusqlite::Result<AnchorMatchMethod> {
    match value {
        "exact_at_line" => Ok(AnchorMatchMethod::ExactAtLine),
        "exact_elsewhere" => Ok(AnchorMatchMethod::ExactElsewhere),
        "context" => Ok(AnchorMatchMethod::Context),
        "not_found" => Ok(AnchorMatchMethod::NotFound),
        unknown => conversion_error(index, format!("unknown anchor match method {unknown:?}")),
    }
}

fn anchor_aggregate_status_to_db(status: AnchorAggregateStatus) -> &'static str {
    match status {
        AnchorAggregateStatus::Anchored => "anchored",
        AnchorAggregateStatus::Partial => "partial",
        AnchorAggregateStatus::Orphaned => "orphaned",
    }
}

fn anchor_aggregate_status_from_db(
    value: &str,
    index: usize,
) -> rusqlite::Result<AnchorAggregateStatus> {
    match value {
        "anchored" => Ok(AnchorAggregateStatus::Anchored),
        "partial" => Ok(AnchorAggregateStatus::Partial),
        "orphaned" => Ok(AnchorAggregateStatus::Orphaned),
        unknown => conversion_error(
            index,
            format!("unknown anchor aggregate status {unknown:?}"),
        ),
    }
}

fn conversion_error<T>(index: usize, message: String) -> rusqlite::Result<T> {
    Err(rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message,
        )),
    ))
}

fn validate_new_anchor(anchor: &NewCommentAnchor) -> Result<()> {
    let segments: Vec<CommentAnchorSegment> = anchor
        .segments
        .iter()
        .map(|segment| CommentAnchorSegment {
            side: segment.side,
            file_path: segment.file_path.clone(),
            line_start: segment.line_start,
            line_end: segment.line_end,
            char_start: segment.char_start,
            char_end: segment.char_end,
            anchor_text: segment.anchor_text.clone(),
            context_before: segment.context_before.clone(),
            context_after: segment.context_after.clone(),
            placement_status: segment.placement_status,
            match_method: segment.match_method,
        })
        .collect();
    CommentAnchor::try_with_aggregate_status(segments, anchor.aggregate_status)
        .context("Invalid comment anchor")?;
    Ok(())
}

fn read_comment_core_row(row: &rusqlite::Row) -> rusqlite::Result<StoredCommentCore> {
    let aggregate_status: String = row.get(10)?;

    Ok(StoredCommentCore {
        id: row.get(0)?,
        merge_base: row.get(1)?,
        head_ref: row.get(2)?,
        created_head_commit: row.get(3)?,
        file_path: row.get(4)?,
        body: row.get(5)?,
        resolved: row.get::<_, i64>(6)? != 0,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        anchor_version_id: row.get(9)?,
        aggregate_status: anchor_aggregate_status_from_db(&aggregate_status, 10)?,
    })
}

fn read_anchor_segment_row(row: &rusqlite::Row) -> rusqlite::Result<StoredAnchorSegment> {
    let side: String = row.get(0)?;
    let placement_status: String = row.get(10)?;
    let match_method: String = row.get(11)?;

    Ok(StoredAnchorSegment {
        segment: CommentAnchorSegment {
            side: comment_anchor_side_from_db(&side, 0)?,
            file_path: row.get(1)?,
            line_start: row.get(3)?,
            line_end: row.get(4)?,
            char_start: row.get(5)?,
            char_end: row.get(6)?,
            anchor_text: row.get(7)?,
            context_before: row.get(8)?,
            context_after: row.get(9)?,
            placement_status: anchor_placement_status_from_db(&placement_status, 10)?,
            match_method: anchor_match_method_from_db(&match_method, 11)?,
        },
        file_blob_sha: row.get(2)?,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db = Database::open(&db_path).unwrap();
        (dir, db)
    }

    /// Simple comment for tests that only care about scope + file + body.
    fn simple_comment(file: &str, line: i64, anchor: &str, body: &str) -> NewComment {
        NewComment {
            merge_base: "main".to_string(),
            head_ref: "feat".to_string(),
            created_head_commit: "head-commit".to_string(),
            file_path: file.to_string(),
            body: body.to_string(),
            anchor: new_head_anchor(file, line, line, anchor, AnchorMatchMethod::ExactAtLine),
        }
    }

    fn new_head_anchor(
        file: &str,
        line_start: i64,
        line_end: i64,
        anchor_text: &str,
        match_method: AnchorMatchMethod,
    ) -> NewCommentAnchor {
        NewCommentAnchor {
            segments: vec![new_anchor_segment(
                CommentAnchorSide::Head,
                file,
                "blob-1",
                line_start,
                line_end,
                anchor_text,
                AnchorPlacementStatus::Anchored,
                match_method,
            )],
            aggregate_status: AnchorAggregateStatus::Anchored,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn new_anchor_segment(
        side: CommentAnchorSide,
        file: &str,
        file_blob_sha: &str,
        line_start: i64,
        line_end: i64,
        anchor_text: &str,
        placement_status: AnchorPlacementStatus,
        match_method: AnchorMatchMethod,
    ) -> NewCommentAnchorSegment {
        NewCommentAnchorSegment {
            side,
            file_path: file.to_string(),
            file_blob_sha: file_blob_sha.to_string(),
            line_start,
            line_end,
            char_start: None,
            char_end: None,
            anchor_text: anchor_text.to_string(),
            context_before: format!("before {anchor_text}"),
            context_after: format!("after {anchor_text}"),
            placement_status,
            match_method,
        }
    }

    fn segment_for_side(anchor: &CommentAnchor, side: CommentAnchorSide) -> &CommentAnchorSegment {
        anchor
            .segments
            .iter()
            .find(|segment| segment.side == side)
            .unwrap()
    }

    fn resolution_event(comment: &StoredComment) -> NewCommentResolutionEvent {
        NewCommentResolutionEvent {
            comment_id: comment.id,
            resolved_commit: "resolved-commit".to_string(),
            resolved_head_ref: comment.head_ref.clone(),
            resolved_merge_base: comment.merge_base.clone(),
            anchor: comment.anchor.clone(),
            file_path: comment.file_path.clone(),
            line_start: comment.line_start,
            line_end: comment.line_end,
            char_start: comment.char_start,
            char_end: comment.char_end,
            anchor_text: comment.anchor_text.clone(),
            context_before: comment.context_before.clone(),
            context_after: comment.context_after.clone(),
            anchor_status: comment.anchor_status,
        }
    }

    #[test]
    fn test_schema_created() {
        let (dir, _db) = test_db();
        let db_path = dir.path().join("test.db");
        assert!(db_path.exists());
    }

    #[test]
    fn test_migrates_inline_comment_anchors() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "
                CREATE TABLE file_reviews (
                    file_path       TEXT NOT NULL,
                    merge_base      TEXT NOT NULL,
                    head_ref        TEXT NOT NULL,
                    diff_hash       TEXT NOT NULL,
                    reviewed_at     TEXT NOT NULL,
                    reviewed_commit TEXT NOT NULL DEFAULT '',
                    PRIMARY KEY (merge_base, head_ref, file_path)
                );

                CREATE TABLE comments (
                    id              INTEGER PRIMARY KEY AUTOINCREMENT,
                    merge_base      TEXT NOT NULL,
                    head_ref        TEXT NOT NULL,
                    file_path       TEXT NOT NULL,
                    line_start      INTEGER NOT NULL,
                    line_end        INTEGER NOT NULL,
                    char_start      INTEGER,
                    char_end        INTEGER,
                    anchor_text     TEXT NOT NULL,
                    context_before  TEXT NOT NULL DEFAULT '',
                    context_after   TEXT NOT NULL DEFAULT '',
                    body            TEXT NOT NULL,
                    resolved        INTEGER NOT NULL DEFAULT 0,
                    created_at      TEXT NOT NULL,
                    updated_at      TEXT NOT NULL
                );

                INSERT INTO comments
                    (id, merge_base, head_ref, file_path, line_start,
                     line_end, char_start, char_end, anchor_text,
                     context_before, context_after, body, resolved,
                     created_at, updated_at)
                VALUES
                    (42, 'main', 'feat', 'src/lib.rs', 7, 8, 1, 4,
                     'old anchor', 'before', 'after', 'body text', 1,
                     '2026-06-27T12:00:00+10:00',
                     '2026-06-27T12:01:00+10:00');
                ",
            )
            .unwrap();
        }

        let db = Database::open(&db_path).unwrap();
        assert!(db.get_comment(42).unwrap().is_none());

        let count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM comment_resolution_events WHERE comment_id = 42",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_migrates_base_ref_schema_to_current_schema() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "
                CREATE TABLE file_reviews (
                    file_path   TEXT NOT NULL,
                    base_ref    TEXT NOT NULL,
                    head_ref    TEXT NOT NULL,
                    diff_hash   TEXT NOT NULL,
                    reviewed_at TEXT NOT NULL,
                    PRIMARY KEY (base_ref, head_ref, file_path)
                );

                CREATE TABLE comments (
                    id              INTEGER PRIMARY KEY AUTOINCREMENT,
                    base_ref        TEXT NOT NULL,
                    head_ref        TEXT NOT NULL,
                    file_path       TEXT NOT NULL,
                    line_start      INTEGER NOT NULL,
                    line_end        INTEGER NOT NULL,
                    char_start      INTEGER,
                    char_end        INTEGER,
                    anchor_text     TEXT NOT NULL,
                    context_before  TEXT NOT NULL DEFAULT '',
                    context_after   TEXT NOT NULL DEFAULT '',
                    body            TEXT NOT NULL,
                    resolved        INTEGER NOT NULL DEFAULT 0,
                    created_at      TEXT NOT NULL,
                    updated_at      TEXT NOT NULL
                );

                CREATE INDEX idx_comments_scope
                    ON comments (base_ref, head_ref, file_path);

                INSERT INTO file_reviews
                    (file_path, base_ref, head_ref, diff_hash, reviewed_at)
                VALUES
                    ('src/lib.rs', 'main', 'feat', 'hash-1',
                     '2026-06-27T12:00:00+10:00');

                INSERT INTO comments
                    (id, base_ref, head_ref, file_path, line_start,
                     line_end, char_start, char_end, anchor_text,
                     context_before, context_after, body, resolved,
                     created_at, updated_at)
                VALUES
                    (7, 'main', 'feat', 'src/lib.rs', 3, 3, NULL, NULL,
                     'anchor', '', '', 'legacy body', 0,
                     '2026-06-27T12:00:00+10:00',
                     '2026-06-27T12:01:00+10:00');
                ",
            )
            .unwrap();
        }

        let db = Database::open(&db_path).unwrap();
        let reviews = db.load_reviews("main", "feat").unwrap();
        let review = reviews.get("src/lib.rs").unwrap();
        assert_eq!(review.diff_hash, "hash-1");
        assert_eq!(review.reviewed_commit, "");

        assert!(db.get_comment(7).unwrap().is_none());

        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 7);
    }

    #[test]
    fn test_store_and_load_review() {
        let (_dir, db) = test_db();

        db.store_review("main", "feature-a", "src/main.rs", "abc123", "deadbeef")
            .unwrap();

        let reviews = db.load_reviews("main", "feature-a").unwrap();
        assert_eq!(reviews.len(), 1);

        let review = &reviews["src/main.rs"];
        assert_eq!(review.file_path, "src/main.rs");
        assert_eq!(review.diff_hash, "abc123");
        assert_eq!(review.reviewed_commit, "deadbeef");
        assert!(!review.reviewed_at.is_empty());
    }

    #[test]
    fn test_review_upsert() {
        let (_dir, db) = test_db();

        db.store_review("main", "feature-a", "src/main.rs", "hash1", "commit1")
            .unwrap();
        db.store_review("main", "feature-a", "src/main.rs", "hash2", "commit2")
            .unwrap();

        let reviews = db.load_reviews("main", "feature-a").unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews["src/main.rs"].diff_hash, "hash2");
    }

    #[test]
    fn test_review_scope_isolation() {
        let (_dir, db) = test_db();

        db.store_review("main", "feature-a", "src/main.rs", "hash-a", "commit-a")
            .unwrap();
        db.store_review("main", "feature-b", "src/main.rs", "hash-b", "commit-b")
            .unwrap();

        let reviews_a = db.load_reviews("main", "feature-a").unwrap();
        let reviews_b = db.load_reviews("main", "feature-b").unwrap();

        assert_eq!(reviews_a["src/main.rs"].diff_hash, "hash-a");
        assert_eq!(reviews_b["src/main.rs"].diff_hash, "hash-b");
    }

    #[test]
    fn test_load_empty() {
        let (_dir, db) = test_db();
        let reviews = db.load_reviews("main", "nonexistent").unwrap();
        assert!(reviews.is_empty());
    }

    #[test]
    fn test_remove_review() {
        let (_dir, db) = test_db();

        db.store_review("main", "feat", "a.rs", "h1", "c1").unwrap();
        db.store_review("main", "feat", "b.rs", "h2", "c2").unwrap();

        db.remove_review("main", "feat", "a.rs").unwrap();

        let reviews = db.load_reviews("main", "feat").unwrap();
        assert_eq!(reviews.len(), 1);
        assert!(reviews.contains_key("b.rs"));
    }

    #[test]
    fn test_clear_reviews() {
        let (_dir, db) = test_db();

        db.store_review("main", "feat-a", "a.rs", "h1", "c1")
            .unwrap();
        db.store_review("main", "feat-a", "b.rs", "h2", "c2")
            .unwrap();
        db.store_review("main", "feat-b", "a.rs", "h3", "c3")
            .unwrap();

        let count = db.clear_reviews("main", "feat-a").unwrap();
        assert_eq!(count, 2);

        assert!(db.load_reviews("main", "feat-a").unwrap().is_empty());
        assert_eq!(db.load_reviews("main", "feat-b").unwrap().len(), 1);
    }

    #[test]
    fn test_comment_create_and_get() {
        let (_dir, db) = test_db();

        let comment = db
            .create_comment(&NewComment {
                merge_base: "main".to_string(),
                head_ref: "feat".to_string(),
                created_head_commit: "head-commit".to_string(),
                file_path: "src/lib.rs".to_string(),
                body: "This should handle errors".to_string(),
                anchor: NewCommentAnchor {
                    segments: vec![NewCommentAnchorSegment {
                        side: CommentAnchorSide::Head,
                        file_path: "src/lib.rs".to_string(),
                        file_blob_sha: "blob-1".to_string(),
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
            })
            .unwrap();

        assert!(comment.id > 0);
        assert_eq!(comment.file_path, "src/lib.rs");
        assert_eq!(comment.line_start, 10);
        assert_eq!(comment.line_end, 12);
        assert!(!comment.resolved);

        let fetched = db.get_comment(comment.id).unwrap().unwrap();
        assert_eq!(fetched.body, "This should handle errors");
        assert_eq!(fetched.anchor_text, "fn foo() {}");
        assert_eq!(fetched.context_before, "// before");
        assert_eq!(fetched.context_after, "// after");
        assert_eq!(fetched.file_blob_sha, "blob-1");
        assert_eq!(fetched.anchor_status, AnchorStatus::Anchored);
    }

    #[test]
    fn test_comment_char_selection() {
        let (_dir, db) = test_db();

        let mut c = simple_comment("a.rs", 5, "some_var", "Rename this");
        c.anchor.segments[0].char_start = Some(10);
        c.anchor.segments[0].char_end = Some(20);
        let comment = db.create_comment(&c).unwrap();

        let fetched = db.get_comment(comment.id).unwrap().unwrap();
        assert_eq!(fetched.char_start, Some(10));
        assert_eq!(fetched.char_end, Some(20));
    }

    #[test]
    fn test_comment_create_and_get_base_only_anchor() {
        let (_dir, db) = test_db();

        let comment = db
            .create_comment(&NewComment {
                merge_base: "main".to_string(),
                head_ref: "feat".to_string(),
                created_head_commit: "head-commit".to_string(),
                file_path: "src/lib.rs".to_string(),
                body: "deleted line feedback".to_string(),
                anchor: NewCommentAnchor {
                    segments: vec![new_anchor_segment(
                        CommentAnchorSide::Base,
                        "src/lib.rs",
                        "base-blob",
                        4,
                        4,
                        "deleted line",
                        AnchorPlacementStatus::Anchored,
                        AnchorMatchMethod::ExactAtLine,
                    )],
                    aggregate_status: AnchorAggregateStatus::Anchored,
                },
            })
            .unwrap();

        assert_eq!(comment.anchor.segments.len(), 1);
        assert!(
            comment
                .anchor
                .segments
                .iter()
                .all(|segment| segment.side != CommentAnchorSide::Head)
        );
        let base = segment_for_side(&comment.anchor, CommentAnchorSide::Base);
        assert_eq!(base.file_path, "src/lib.rs");
        assert_eq!(base.line_start, 4);
        assert_eq!(base.line_end, 4);
        assert_eq!(base.anchor_text, "deleted line");
        assert_eq!(comment.line_start, 4);
        assert_eq!(comment.line_end, 4);
        assert_eq!(comment.file_blob_sha, "base-blob");
    }

    #[test]
    fn test_comment_create_and_get_paired_anchor() {
        let (_dir, db) = test_db();

        let comment = db
            .create_comment(&NewComment {
                merge_base: "main".to_string(),
                head_ref: "feat".to_string(),
                created_head_commit: "head-commit".to_string(),
                file_path: "src/lib.rs".to_string(),
                body: "replacement feedback".to_string(),
                anchor: NewCommentAnchor {
                    segments: vec![
                        new_anchor_segment(
                            CommentAnchorSide::Base,
                            "src/lib.rs",
                            "base-blob",
                            8,
                            9,
                            "old one\nold two",
                            AnchorPlacementStatus::Anchored,
                            AnchorMatchMethod::ExactAtLine,
                        ),
                        new_anchor_segment(
                            CommentAnchorSide::Head,
                            "src/lib.rs",
                            "head-blob",
                            8,
                            10,
                            "new one\nnew two\nnew three",
                            AnchorPlacementStatus::Anchored,
                            AnchorMatchMethod::ExactAtLine,
                        ),
                    ],
                    aggregate_status: AnchorAggregateStatus::Anchored,
                },
            })
            .unwrap();

        assert_eq!(comment.anchor.segments.len(), 2);
        let base = segment_for_side(&comment.anchor, CommentAnchorSide::Base);
        let head = segment_for_side(&comment.anchor, CommentAnchorSide::Head);
        assert_eq!(base.line_start, 8);
        assert_eq!(base.line_end, 9);
        assert_eq!(base.anchor_text, "old one\nold two");
        assert_eq!(head.line_start, 8);
        assert_eq!(head.line_end, 10);
        assert_eq!(head.anchor_text, "new one\nnew two\nnew three");
        assert_eq!(comment.line_start, 8);
        assert_eq!(comment.line_end, 10);
        assert_eq!(comment.file_blob_sha, "head-blob");
        assert_eq!(
            comment.anchor.aggregate_status,
            AnchorAggregateStatus::Anchored
        );
    }

    #[test]
    fn test_anchor_versions_are_append_only_for_compound_anchors() {
        let (_dir, db) = test_db();

        let mut comment = simple_comment("a.rs", 1, "head one", "fix this");
        comment.anchor = NewCommentAnchor {
            segments: vec![
                new_anchor_segment(
                    CommentAnchorSide::Base,
                    "a.rs",
                    "base-v1",
                    1,
                    1,
                    "base one",
                    AnchorPlacementStatus::Anchored,
                    AnchorMatchMethod::ExactAtLine,
                ),
                new_anchor_segment(
                    CommentAnchorSide::Head,
                    "a.rs",
                    "head-v1",
                    2,
                    2,
                    "head one",
                    AnchorPlacementStatus::Anchored,
                    AnchorMatchMethod::ExactAtLine,
                ),
            ],
            aggregate_status: AnchorAggregateStatus::Anchored,
        };
        let comment = db.create_comment(&comment).unwrap();

        db.insert_anchor_version(&NewAnchorVersion {
            comment_id: comment.id,
            anchor: NewCommentAnchor {
                segments: vec![
                    new_anchor_segment(
                        CommentAnchorSide::Base,
                        "a.rs",
                        "base-v2",
                        3,
                        3,
                        "base one",
                        AnchorPlacementStatus::Anchored,
                        AnchorMatchMethod::ExactElsewhere,
                    ),
                    new_anchor_segment(
                        CommentAnchorSide::Head,
                        "a.rs",
                        "head-v2",
                        4,
                        4,
                        "head one",
                        AnchorPlacementStatus::Anchored,
                        AnchorMatchMethod::ExactElsewhere,
                    ),
                ],
                aggregate_status: AnchorAggregateStatus::Anchored,
            },
        })
        .unwrap();

        let version_count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM anchor_versions WHERE comment_id = ?1",
                [comment.id],
                |row| row.get(0),
            )
            .unwrap();
        let segment_count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*)
                 FROM anchor_segments segment
                 JOIN anchor_versions version
                   ON version.id = segment.anchor_version_id
                 WHERE version.comment_id = ?1",
                [comment.id],
                |row| row.get(0),
            )
            .unwrap();
        let fetched = db.get_comment(comment.id).unwrap().unwrap();

        assert_eq!(version_count, 2);
        assert_eq!(segment_count, 4);
        assert_eq!(
            segment_for_side(&fetched.anchor, CommentAnchorSide::Base).line_start,
            3
        );
        assert_eq!(
            segment_for_side(&fetched.anchor, CommentAnchorSide::Head).line_start,
            4
        );
    }

    #[test]
    fn test_compound_anchor_aggregate_status_persists() {
        let (_dir, db) = test_db();

        let mut partial = simple_comment("a.rs", 1, "head", "partial");
        partial.anchor = NewCommentAnchor {
            segments: vec![
                new_anchor_segment(
                    CommentAnchorSide::Base,
                    "a.rs",
                    "base-blob",
                    1,
                    1,
                    "base",
                    AnchorPlacementStatus::Orphaned,
                    AnchorMatchMethod::NotFound,
                ),
                new_anchor_segment(
                    CommentAnchorSide::Head,
                    "a.rs",
                    "head-blob",
                    2,
                    2,
                    "head",
                    AnchorPlacementStatus::Anchored,
                    AnchorMatchMethod::ExactAtLine,
                ),
            ],
            aggregate_status: AnchorAggregateStatus::Partial,
        };

        let mut orphaned = simple_comment("b.rs", 1, "gone", "orphaned");
        orphaned.anchor = NewCommentAnchor {
            segments: vec![new_anchor_segment(
                CommentAnchorSide::Head,
                "b.rs",
                "head-blob",
                7,
                7,
                "gone",
                AnchorPlacementStatus::Orphaned,
                AnchorMatchMethod::NotFound,
            )],
            aggregate_status: AnchorAggregateStatus::Orphaned,
        };

        let partial = db.create_comment(&partial).unwrap();
        let orphaned = db.create_comment(&orphaned).unwrap();

        assert_eq!(
            partial.anchor.aggregate_status,
            AnchorAggregateStatus::Partial
        );
        assert_eq!(partial.anchor_status, AnchorStatus::Approximate);
        assert_eq!(
            orphaned.anchor.aggregate_status,
            AnchorAggregateStatus::Orphaned
        );
        assert_eq!(orphaned.anchor_status, AnchorStatus::Orphaned);
    }

    #[test]
    fn test_compound_anchor_enum_text_mappings() {
        assert_eq!(comment_anchor_side_to_db(CommentAnchorSide::Base), "base");
        assert_eq!(comment_anchor_side_to_db(CommentAnchorSide::Head), "head");
        assert_eq!(
            comment_anchor_side_from_db("base", 0).unwrap(),
            CommentAnchorSide::Base
        );
        assert_eq!(
            comment_anchor_side_from_db("head", 0).unwrap(),
            CommentAnchorSide::Head
        );

        assert_eq!(
            anchor_placement_status_to_db(AnchorPlacementStatus::Anchored),
            "anchored"
        );
        assert_eq!(
            anchor_placement_status_to_db(AnchorPlacementStatus::Orphaned),
            "orphaned"
        );
        assert_eq!(
            anchor_placement_status_from_db("anchored", 0).unwrap(),
            AnchorPlacementStatus::Anchored
        );
        assert_eq!(
            anchor_placement_status_from_db("orphaned", 0).unwrap(),
            AnchorPlacementStatus::Orphaned
        );

        assert_eq!(
            anchor_match_method_to_db(AnchorMatchMethod::ExactAtLine),
            "exact_at_line"
        );
        assert_eq!(
            anchor_match_method_to_db(AnchorMatchMethod::ExactElsewhere),
            "exact_elsewhere"
        );
        assert_eq!(
            anchor_match_method_to_db(AnchorMatchMethod::Context),
            "context"
        );
        assert_eq!(
            anchor_match_method_to_db(AnchorMatchMethod::NotFound),
            "not_found"
        );
        assert_eq!(
            anchor_match_method_from_db("exact_at_line", 0).unwrap(),
            AnchorMatchMethod::ExactAtLine
        );
        assert_eq!(
            anchor_match_method_from_db("exact_elsewhere", 0).unwrap(),
            AnchorMatchMethod::ExactElsewhere
        );
        assert_eq!(
            anchor_match_method_from_db("context", 0).unwrap(),
            AnchorMatchMethod::Context
        );
        assert_eq!(
            anchor_match_method_from_db("not_found", 0).unwrap(),
            AnchorMatchMethod::NotFound
        );

        assert_eq!(
            anchor_aggregate_status_to_db(AnchorAggregateStatus::Anchored),
            "anchored"
        );
        assert_eq!(
            anchor_aggregate_status_to_db(AnchorAggregateStatus::Partial),
            "partial"
        );
        assert_eq!(
            anchor_aggregate_status_to_db(AnchorAggregateStatus::Orphaned),
            "orphaned"
        );
        assert_eq!(
            anchor_aggregate_status_from_db("anchored", 0).unwrap(),
            AnchorAggregateStatus::Anchored
        );
        assert_eq!(
            anchor_aggregate_status_from_db("partial", 0).unwrap(),
            AnchorAggregateStatus::Partial
        );
        assert_eq!(
            anchor_aggregate_status_from_db("orphaned", 0).unwrap(),
            AnchorAggregateStatus::Orphaned
        );
    }

    #[test]
    fn test_comment_list_filters() {
        let (_dir, db) = test_db();

        db.create_comment(&simple_comment("a.rs", 1, "x", "comment 1"))
            .unwrap();
        db.create_comment(&simple_comment("b.rs", 1, "y", "comment 2"))
            .unwrap();

        let c3 = db
            .create_comment(&simple_comment("a.rs", 5, "z", "comment 3"))
            .unwrap();
        db.resolve_comment(&resolution_event(&c3)).unwrap();

        // All unresolved
        let comments = db
            .list_comments("main", "feat", None, false, false)
            .unwrap();
        assert_eq!(comments.len(), 2);

        // All including resolved
        let comments = db.list_comments("main", "feat", None, true, false).unwrap();
        assert_eq!(comments.len(), 3);

        // Filtered by file, unresolved only
        let comments = db
            .list_comments("main", "feat", Some("a.rs"), false, false)
            .unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].body, "comment 1");

        // Filtered by file, including resolved
        let comments = db
            .list_comments("main", "feat", Some("a.rs"), true, false)
            .unwrap();
        assert_eq!(comments.len(), 2);
    }

    #[test]
    fn test_list_comments_can_load_previous_base_unresolved_comments() {
        let (_dir, db) = test_db();

        db.create_comment(&simple_comment("a.rs", 1, "x", "current base"))
            .unwrap();

        let mut old_unresolved = simple_comment("a.rs", 2, "y", "old unresolved");
        old_unresolved.merge_base = "old-main".to_string();
        db.create_comment(&old_unresolved).unwrap();

        let mut old_resolved = simple_comment("a.rs", 3, "z", "old resolved");
        old_resolved.merge_base = "older-main".to_string();
        let old_resolved = db.create_comment(&old_resolved).unwrap();
        db.resolve_comment(&resolution_event(&old_resolved))
            .unwrap();

        let mut other_session = simple_comment("b.rs", 4, "q", "other session");
        other_session.head_ref = "other-head".to_string();
        db.create_comment(&other_session).unwrap();

        let exact = db.list_comments("main", "feat", None, true, false).unwrap();
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].body, "current base");

        let carry_over = db.list_comments("main", "feat", None, false, true).unwrap();
        assert_eq!(carry_over.len(), 2);
        assert_eq!(carry_over[0].body, "old unresolved");
        assert_eq!(carry_over[1].body, "other session");
    }

    #[test]
    fn test_comment_update() {
        let (_dir, db) = test_db();

        let comment = db
            .create_comment(&simple_comment("a.rs", 1, "x", "delete me"))
            .unwrap();

        let updated = db.update_comment(comment.id, "revised").unwrap();
        assert!(updated);

        let fetched = db.get_comment(comment.id).unwrap().unwrap();
        assert_eq!(fetched.body, "revised");
        assert!(fetched.updated_at >= fetched.created_at);
    }

    #[test]
    fn test_comment_resolve_unresolve() {
        let (_dir, db) = test_db();

        let comment = db
            .create_comment(&simple_comment("a.rs", 1, "x", "fix this"))
            .unwrap();
        assert!(!comment.resolved);

        db.resolve_comment(&resolution_event(&comment)).unwrap();
        assert!(
            db.comment_has_resolution_in_scope(comment.id, "main", "feat")
                .unwrap()
        );
        assert!(
            !db.comment_has_resolution_in_scope(comment.id, "other-main", "feat")
                .unwrap()
        );
        let fetched = db.get_comment(comment.id).unwrap().unwrap();
        assert!(fetched.resolved);

        db.unresolve_comment(comment.id).unwrap();
        assert!(
            !db.comment_has_resolution_in_scope(comment.id, "main", "feat")
                .unwrap()
        );
        let fetched = db.get_comment(comment.id).unwrap().unwrap();
        assert!(!fetched.resolved);
    }

    #[test]
    fn test_resolution_event_persists_compound_anchor_snapshot() {
        let (_dir, db) = test_db();

        let mut comment = simple_comment("a.rs", 1, "head text", "fix this");
        comment.anchor = NewCommentAnchor {
            segments: vec![
                NewCommentAnchorSegment {
                    side: CommentAnchorSide::Base,
                    file_path: "a.rs".to_string(),
                    file_blob_sha: "base-blob".to_string(),
                    line_start: 3,
                    line_end: 3,
                    char_start: None,
                    char_end: None,
                    anchor_text: "base text".to_string(),
                    context_before: "before base".to_string(),
                    context_after: "after base".to_string(),
                    placement_status: AnchorPlacementStatus::Anchored,
                    match_method: AnchorMatchMethod::ExactAtLine,
                },
                NewCommentAnchorSegment {
                    side: CommentAnchorSide::Head,
                    file_path: "a.rs".to_string(),
                    file_blob_sha: "head-blob".to_string(),
                    line_start: 7,
                    line_end: 8,
                    char_start: None,
                    char_end: None,
                    anchor_text: "head text".to_string(),
                    context_before: "before head".to_string(),
                    context_after: "after head".to_string(),
                    placement_status: AnchorPlacementStatus::Anchored,
                    match_method: AnchorMatchMethod::ExactAtLine,
                },
            ],
            aggregate_status: AnchorAggregateStatus::Anchored,
        };

        let comment = db.create_comment(&comment).unwrap();
        db.resolve_comment(&resolution_event(&comment)).unwrap();

        let segment_count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*)
                 FROM comment_resolution_anchor_segments
                 WHERE comment_id = ?1",
                [comment.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(segment_count, 2);
    }

    #[test]
    fn test_resolution_events_store_stable_patch_id_evidence() {
        let (_dir, db) = test_db();

        let columns: Vec<String> = db
            .conn
            .prepare("PRAGMA table_info(comment_resolution_events)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();

        assert!(
            columns.iter().any(|column| column == "resolved_patch_id"),
            "resolution events must store stable patch-id evidence"
        );
    }

    #[test]
    fn test_resolution_anchor_segments_store_blob_evidence() {
        let (_dir, db) = test_db();

        let columns: Vec<String> = db
            .conn
            .prepare("PRAGMA table_info(comment_resolution_anchor_segments)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();

        assert!(
            columns.iter().any(|column| column == "file_blob_sha"),
            "resolution anchor snapshots must store side blob evidence"
        );
    }

    #[test]
    fn test_resolution_scope_can_follow_rebased_equivalent_history() {
        let (_dir, db) = test_db();

        let mut comment = simple_comment("a.rs", 1, "head text", "fix this");
        comment.merge_base = "old-base".to_string();
        comment.head_ref = "feature".to_string();
        let comment = db.create_comment(&comment).unwrap();
        db.resolve_comment(&resolution_event(&comment)).unwrap();

        assert!(
            db.comment_has_resolution_in_scope(comment.id, "new-base", "feature")
                .unwrap(),
            "resolution evidence should carry across equivalent rebased history"
        );
    }

    #[test]
    fn test_resolution_scope_can_follow_squashed_equivalent_patch_id() {
        let (_dir, db) = test_db();

        let mut comment = simple_comment("a.rs", 1, "head text", "fix this");
        comment.merge_base = "old-base".to_string();
        comment.head_ref = "feature".to_string();
        let comment = db.create_comment(&comment).unwrap();
        let mut event = resolution_event(&comment);
        event.resolved_commit = "pre-squash-resolution".to_string();
        db.resolve_comment(&event).unwrap();

        assert!(
            db.comment_has_resolution_in_scope(comment.id, "squash-base", "feature")
                .unwrap(),
            "stable patch-id evidence should carry resolution across squash history"
        );
    }

    #[test]
    fn test_resolution_anchor_evidence_can_prove_rebased_content_state() {
        let (_dir, db) = test_db();

        let mut comment = simple_comment("a.rs", 1, "head text", "fix this");
        comment.merge_base = "old-base".to_string();
        comment.head_ref = "feature".to_string();
        let comment = db.create_comment(&comment).unwrap();
        db.resolve_comment(&resolution_event(&comment)).unwrap();

        assert!(
            db.comment_has_resolution_in_scope(
                comment.id,
                "rebased-base-with-same-anchor",
                "feature"
            )
            .unwrap(),
            "matching resolution-time anchor evidence should prove the resolved content state"
        );
    }

    #[test]
    fn test_resolution_anchor_evidence_rejects_changed_rebased_content_state() {
        let (_dir, db) = test_db();

        let mut comment = simple_comment("a.rs", 1, "head text", "fix this");
        comment.merge_base = "old-base".to_string();
        comment.head_ref = "feature".to_string();
        let comment = db.create_comment(&comment).unwrap();
        db.resolve_comment(&resolution_event(&comment)).unwrap();

        assert!(
            !db.comment_has_resolution_in_scope(
                comment.id,
                "rebased-base-with-different-anchor",
                "feature"
            )
            .unwrap(),
            "resolution-time anchor evidence must not resolve changed content"
        );
    }

    #[test]
    fn test_comment_delete() {
        let (_dir, db) = test_db();

        let comment = db
            .create_comment(&simple_comment("a.rs", 1, "x", "delete me"))
            .unwrap();

        let deleted = db.delete_comment(comment.id).unwrap();
        assert!(deleted);

        let fetched = db.get_comment(comment.id).unwrap();
        assert!(fetched.is_none());

        // Deleting again returns false
        let deleted = db.delete_comment(comment.id).unwrap();
        assert!(!deleted);
    }

    #[test]
    fn test_comment_nonexistent_id() {
        let (_dir, db) = test_db();
        let fetched = db.get_comment(9999).unwrap();
        assert!(fetched.is_none());

        let updated = db.update_comment(9999, "nope").unwrap();
        assert!(!updated);
    }

    #[test]
    fn test_load_reviews_by_head_ref() {
        let (_dir, db) = test_db();

        // Store reviews under two different merge_bases for the same head_ref.
        db.store_review("old_base", "feature-a", "src/main.rs", "hash1", "commit1")
            .unwrap();
        db.store_review("old_base", "feature-a", "src/lib.rs", "hash2", "commit2")
            .unwrap();
        db.store_review("new_base", "feature-a", "src/main.rs", "hash3", "commit3")
            .unwrap();

        // Also store a review for a different head_ref (should not appear).
        db.store_review("old_base", "feature-b", "src/main.rs", "hash4", "commit4")
            .unwrap();

        let scopes = db.load_reviews_by_head_ref("feature-a").unwrap();

        // Should have 2 scopes for feature-a.
        assert_eq!(scopes.len(), 2);

        // Verify both scopes are present with the correct data.
        let old_scope = scopes.iter().find(|(mb, _)| mb == "old_base");
        let new_scope = scopes.iter().find(|(mb, _)| mb == "new_base");

        assert!(old_scope.is_some(), "should have old_base scope");
        assert!(new_scope.is_some(), "should have new_base scope");

        let old_reviews = &old_scope.unwrap().1;
        assert_eq!(old_reviews.len(), 2);
        assert_eq!(old_reviews["src/main.rs"].diff_hash, "hash1");
        assert_eq!(old_reviews["src/lib.rs"].diff_hash, "hash2");

        let new_reviews = &new_scope.unwrap().1;
        assert_eq!(new_reviews.len(), 1);
        assert_eq!(new_reviews["src/main.rs"].diff_hash, "hash3");
    }

    #[test]
    fn test_load_reviews_by_head_ref_empty() {
        let (_dir, db) = test_db();
        let scopes = db.load_reviews_by_head_ref("nonexistent").unwrap();
        assert!(scopes.is_empty());
    }

    #[test]
    fn test_timestamps_iso8601() {
        let (_dir, db) = test_db();

        let review = db
            .store_review("main", "feat", "a.rs", "hash", "c1")
            .unwrap();

        // Should parse as a valid datetime with timezone
        assert!(
            review.reviewed_at.contains('T'),
            "timestamp should be ISO 8601: {}",
            review.reviewed_at
        );
        assert!(
            review.reviewed_at.contains('+') || review.reviewed_at.contains('-'),
            "timestamp should have timezone offset: {}",
            review.reviewed_at
        );
    }
}
