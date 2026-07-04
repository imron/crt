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

use crate::review_types::AnchorStatus;

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

/// Anchor data stored as a versioned record.
#[derive(Debug, Clone)]
pub struct NewCommentAnchor {
    pub file_blob_sha: String,
    pub line_start: i64,
    pub line_end: i64,
    pub char_start: Option<i64>,
    pub char_end: Option<i64>,
    pub anchor_text: String,
    pub context_before: String,
    pub context_after: String,
    pub status: AnchorStatus,
}

/// A stored comment record.
#[derive(Debug, Clone)]
pub struct StoredComment {
    pub id: i64,
    pub merge_base: String,
    pub head_ref: String,
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
                    (merge_base, head_ref, file_path, body, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                params![new.merge_base, new.head_ref, new.file_path, new.body, now,],
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

        Ok(StoredComment {
            id,
            merge_base: new.merge_base.clone(),
            head_ref: new.head_ref.clone(),
            file_path: new.file_path.clone(),
            line_start: new.anchor.line_start,
            line_end: new.anchor.line_end,
            char_start: new.anchor.char_start,
            char_end: new.anchor.char_end,
            anchor_text: new.anchor.anchor_text.clone(),
            context_before: new.anchor.context_before.clone(),
            context_after: new.anchor.context_after.clone(),
            body: new.body.clone(),
            resolved: false,
            created_at: now.clone(),
            updated_at: now,
            file_blob_sha: new.anchor.file_blob_sha.clone(),
            anchor_status: new.anchor.status,
        })
    }

    /// Append a new anchor version for an existing comment.
    pub fn insert_anchor_version(&self, version: &NewAnchorVersion) -> Result<()> {
        let now = now_iso8601();
        self.insert_anchor_version_at(version, &now)
    }

    fn insert_anchor_version_at(&self, version: &NewAnchorVersion, created_at: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO anchor_versions
                    (comment_id, file_blob_sha, line_start, line_end, char_start,
                     char_end, anchor_text, context_before, context_after, status,
                     created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    version.comment_id,
                    version.anchor.file_blob_sha,
                    version.anchor.line_start,
                    version.anchor.line_end,
                    version.anchor.char_start,
                    version.anchor.char_end,
                    version.anchor.anchor_text,
                    version.anchor.context_before,
                    version.anchor.context_after,
                    anchor_status_to_db(version.anchor.status),
                    created_at,
                ],
            )
            .context("Failed to insert anchor version")?;

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
        sql.push_str(" ORDER BY c.file_path, a.line_start");

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|b| b.as_ref()).collect();

        let mut stmt = self
            .conn
            .prepare(&sql)
            .context("Failed to prepare comment query")?;
        let rows = stmt
            .query_map(&*params_refs, read_comment_row)
            .context("Failed to list comments")?;

        let mut comments = Vec::new();
        for row in rows {
            comments.push(row.context("Failed to read comment row")?);
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
            .query_map(params![id], read_comment_row)
            .context("Failed to get comment")?;

        match rows.next() {
            Some(row) => Ok(Some(row.context("Failed to read comment row")?)),
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
                     anchor_text, context_before, context_after, anchor_status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
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
                ],
            )
            .context("Failed to record comment resolution event")?;
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
        let count: i64 = self
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
        Ok(count > 0)
    }

    /// Delete a comment.
    pub fn delete_comment(&self, id: i64) -> Result<bool> {
        self.conn
            .execute(
                "DELETE FROM comment_resolution_events WHERE comment_id = ?1",
                params![id],
            )
            .context("Failed to delete comment resolution events")?;
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
    SELECT c.id, c.merge_base, c.head_ref, c.file_path,
           a.line_start, a.line_end, a.char_start, a.char_end,
           a.anchor_text, a.context_before, a.context_after,
           c.body,
           CASE WHEN EXISTS (
               SELECT 1
               FROM comment_resolution_events event
               WHERE event.comment_id = c.id
           ) THEN 1 ELSE 0 END AS resolved,
           c.created_at, c.updated_at,
           a.file_blob_sha, a.status
    FROM comments c
    JOIN v_current_anchors a ON a.comment_id = c.id";

const COMMENT_RESOLVED_EXPR: &str = "EXISTS (
    SELECT 1
    FROM comment_resolution_events event
    WHERE event.comment_id = c.id
)";

fn anchor_status_to_db(status: AnchorStatus) -> &'static str {
    match status {
        AnchorStatus::Anchored => "anchored",
        AnchorStatus::Shifted => "shifted",
        AnchorStatus::Approximate => "approximate",
        AnchorStatus::Orphaned => "orphaned",
    }
}

fn anchor_status_from_db(value: &str) -> rusqlite::Result<AnchorStatus> {
    match value {
        "anchored" => Ok(AnchorStatus::Anchored),
        "shifted" => Ok(AnchorStatus::Shifted),
        "approximate" => Ok(AnchorStatus::Approximate),
        "orphaned" => Ok(AnchorStatus::Orphaned),
        unknown => Err(rusqlite::Error::FromSqlConversionFailure(
            16,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown anchor status {unknown:?}"),
            )),
        )),
    }
}

fn read_comment_row(row: &rusqlite::Row) -> rusqlite::Result<StoredComment> {
    let anchor_status: String = row.get(16)?;

    Ok(StoredComment {
        id: row.get(0)?,
        merge_base: row.get(1)?,
        head_ref: row.get(2)?,
        file_path: row.get(3)?,
        line_start: row.get(4)?,
        line_end: row.get(5)?,
        char_start: row.get(6)?,
        char_end: row.get(7)?,
        anchor_text: row.get(8)?,
        context_before: row.get(9)?,
        context_after: row.get(10)?,
        body: row.get(11)?,
        resolved: row.get::<_, i64>(12)? != 0,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
        file_blob_sha: row.get(15)?,
        anchor_status: anchor_status_from_db(&anchor_status)?,
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
            file_path: file.to_string(),
            body: body.to_string(),
            anchor: NewCommentAnchor {
                file_blob_sha: "blob-1".to_string(),
                line_start: line,
                line_end: line,
                char_start: None,
                char_end: None,
                anchor_text: anchor.to_string(),
                context_before: String::new(),
                context_after: String::new(),
                status: AnchorStatus::Anchored,
            },
        }
    }

    fn resolution_event(comment: &StoredComment) -> NewCommentResolutionEvent {
        NewCommentResolutionEvent {
            comment_id: comment.id,
            resolved_commit: "resolved-commit".to_string(),
            resolved_head_ref: comment.head_ref.clone(),
            resolved_merge_base: comment.merge_base.clone(),
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
        let fetched = db.get_comment(42).unwrap().unwrap();

        assert_eq!(fetched.body, "body text");
        assert_eq!(fetched.line_start, 7);
        assert_eq!(fetched.line_end, 8);
        assert_eq!(fetched.char_start, Some(1));
        assert_eq!(fetched.char_end, Some(4));
        assert_eq!(fetched.anchor_text, "old anchor");
        assert_eq!(fetched.context_before, "before");
        assert_eq!(fetched.context_after, "after");
        assert_eq!(fetched.file_blob_sha, "");
        assert_eq!(fetched.anchor_status, AnchorStatus::Anchored);
        assert!(fetched.resolved);

        let count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM comment_resolution_events WHERE comment_id = 42",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
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

        let fetched = db.get_comment(7).unwrap().unwrap();
        assert_eq!(fetched.merge_base, "main");
        assert_eq!(fetched.anchor_text, "anchor");
        assert_eq!(fetched.body, "legacy body");
        assert_eq!(fetched.anchor_status, AnchorStatus::Anchored);

        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 5);
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
                file_path: "src/lib.rs".to_string(),
                body: "This should handle errors".to_string(),
                anchor: NewCommentAnchor {
                    file_blob_sha: "blob-1".to_string(),
                    line_start: 10,
                    line_end: 12,
                    char_start: None,
                    char_end: None,
                    anchor_text: "fn foo() {}".to_string(),
                    context_before: "// before".to_string(),
                    context_after: "// after".to_string(),
                    status: AnchorStatus::Anchored,
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
        c.anchor.char_start = Some(10);
        c.anchor.char_end = Some(20);
        let comment = db.create_comment(&c).unwrap();

        let fetched = db.get_comment(comment.id).unwrap().unwrap();
        assert_eq!(fetched.char_start, Some(10));
        assert_eq!(fetched.char_end, Some(20));
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
