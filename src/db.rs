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
}

/// Parameters for creating a new comment.
#[derive(Debug, Clone)]
pub struct NewComment {
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
}

// ---------------------------------------------------------------------------
// Database handle
// ---------------------------------------------------------------------------

/// Handle to the review database.
pub struct Database {
    conn: Connection,
}

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
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "
            CREATE TABLE IF NOT EXISTS file_reviews (
                file_path   TEXT NOT NULL,
                merge_base  TEXT NOT NULL,
                head_ref    TEXT NOT NULL,
                diff_hash   TEXT NOT NULL,
                reviewed_at TEXT NOT NULL,
                PRIMARY KEY (merge_base, head_ref, file_path)
            );

            CREATE TABLE IF NOT EXISTS comments (
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

            CREATE INDEX IF NOT EXISTS idx_comments_scope
                ON comments (merge_base, head_ref, file_path);
            ",
            )
            .context("Failed to initialize database schema")?;

        Ok(())
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
    ) -> Result<StoredReview> {
        let now = now_iso8601();
        self.conn
            .execute(
                "INSERT INTO file_reviews (merge_base, head_ref, file_path, diff_hash, reviewed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (merge_base, head_ref, file_path)
                 DO UPDATE SET diff_hash = ?4, reviewed_at = ?5",
                params![merge_base, head_ref, file_path, diff_hash, now],
            )
            .context("Failed to store review")?;

        Ok(StoredReview {
            file_path: file_path.to_string(),
            merge_base: merge_base.to_string(),
            head_ref: head_ref.to_string(),
            diff_hash: diff_hash.to_string(),
            reviewed_at: now,
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
                "SELECT file_path, merge_base, head_ref, diff_hash, reviewed_at
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
                    (merge_base, head_ref, file_path, line_start, line_end,
                     char_start, char_end, anchor_text, context_before,
                     context_after, body, resolved, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, ?12, ?12)",
                params![
                    new.merge_base,
                    new.head_ref,
                    new.file_path,
                    new.line_start,
                    new.line_end,
                    new.char_start,
                    new.char_end,
                    new.anchor_text,
                    new.context_before,
                    new.context_after,
                    new.body,
                    now,
                ],
            )
            .context("Failed to create comment")?;

        let id = self.conn.last_insert_rowid();

        Ok(StoredComment {
            id,
            merge_base: new.merge_base.clone(),
            head_ref: new.head_ref.clone(),
            file_path: new.file_path.clone(),
            line_start: new.line_start,
            line_end: new.line_end,
            char_start: new.char_start,
            char_end: new.char_end,
            anchor_text: new.anchor_text.clone(),
            context_before: new.context_before.clone(),
            context_after: new.context_after.clone(),
            body: new.body.clone(),
            resolved: false,
            created_at: now.clone(),
            updated_at: now,
        })
    }

    /// List comments for a `(merge_base, head_ref)` pair.
    pub fn list_comments(
        &self,
        merge_base: &str,
        head_ref: &str,
        file_path: Option<&str>,
        include_resolved: bool,
    ) -> Result<Vec<StoredComment>> {
        let (sql, param_values): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
            match (file_path, include_resolved) {
                (None, true) => (
                    "SELECT id, merge_base, head_ref, file_path, line_start, line_end,
                            char_start, char_end, anchor_text, context_before,
                            context_after, body, resolved, created_at, updated_at
                     FROM comments
                     WHERE merge_base = ?1 AND head_ref = ?2
                     ORDER BY file_path, line_start"
                        .to_string(),
                    vec![
                        Box::new(merge_base.to_string()),
                        Box::new(head_ref.to_string()),
                    ],
                ),
                (None, false) => (
                    "SELECT id, merge_base, head_ref, file_path, line_start, line_end,
                            char_start, char_end, anchor_text, context_before,
                            context_after, body, resolved, created_at, updated_at
                     FROM comments
                     WHERE merge_base = ?1 AND head_ref = ?2 AND resolved = 0
                     ORDER BY file_path, line_start"
                        .to_string(),
                    vec![
                        Box::new(merge_base.to_string()),
                        Box::new(head_ref.to_string()),
                    ],
                ),
                (Some(fp), true) => (
                    "SELECT id, merge_base, head_ref, file_path, line_start, line_end,
                            char_start, char_end, anchor_text, context_before,
                            context_after, body, resolved, created_at, updated_at
                     FROM comments
                     WHERE merge_base = ?1 AND head_ref = ?2 AND file_path = ?3
                     ORDER BY line_start"
                        .to_string(),
                    vec![
                        Box::new(merge_base.to_string()),
                        Box::new(head_ref.to_string()),
                        Box::new(fp.to_string()),
                    ],
                ),
                (Some(fp), false) => (
                    "SELECT id, merge_base, head_ref, file_path, line_start, line_end,
                            char_start, char_end, anchor_text, context_before,
                            context_after, body, resolved, created_at, updated_at
                     FROM comments
                     WHERE merge_base = ?1 AND head_ref = ?2 AND file_path = ?3
                       AND resolved = 0
                     ORDER BY line_start"
                        .to_string(),
                    vec![
                        Box::new(merge_base.to_string()),
                        Box::new(head_ref.to_string()),
                        Box::new(fp.to_string()),
                    ],
                ),
            };

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
            .prepare(
                "SELECT id, merge_base, head_ref, file_path, line_start, line_end,
                        char_start, char_end, anchor_text, context_before,
                        context_after, body, resolved, created_at, updated_at
                 FROM comments WHERE id = ?1",
            )
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
    pub fn resolve_comment(&self, id: i64) -> Result<bool> {
        let now = now_iso8601();
        let count = self
            .conn
            .execute(
                "UPDATE comments SET resolved = 1, updated_at = ?1 WHERE id = ?2",
                params![now, id],
            )
            .context("Failed to resolve comment")?;
        Ok(count > 0)
    }

    /// Unresolve a comment.
    pub fn unresolve_comment(&self, id: i64) -> Result<bool> {
        let now = now_iso8601();
        let count = self
            .conn
            .execute(
                "UPDATE comments SET resolved = 0, updated_at = ?1 WHERE id = ?2",
                params![now, id],
            )
            .context("Failed to unresolve comment")?;
        Ok(count > 0)
    }

    /// Delete a comment.
    pub fn delete_comment(&self, id: i64) -> Result<bool> {
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

fn read_comment_row(row: &rusqlite::Row) -> rusqlite::Result<StoredComment> {
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
            line_start: line,
            line_end: line,
            char_start: None,
            char_end: None,
            anchor_text: anchor.to_string(),
            context_before: String::new(),
            context_after: String::new(),
            body: body.to_string(),
        }
    }

    #[test]
    fn test_schema_created() {
        let (dir, _db) = test_db();
        let db_path = dir.path().join("test.db");
        assert!(db_path.exists());
    }

    #[test]
    fn test_store_and_load_review() {
        let (_dir, db) = test_db();

        db.store_review("main", "feature-a", "src/main.rs", "abc123")
            .unwrap();

        let reviews = db.load_reviews("main", "feature-a").unwrap();
        assert_eq!(reviews.len(), 1);

        let review = &reviews["src/main.rs"];
        assert_eq!(review.file_path, "src/main.rs");
        assert_eq!(review.diff_hash, "abc123");
        assert!(!review.reviewed_at.is_empty());
    }

    #[test]
    fn test_review_upsert() {
        let (_dir, db) = test_db();

        db.store_review("main", "feature-a", "src/main.rs", "hash1")
            .unwrap();
        db.store_review("main", "feature-a", "src/main.rs", "hash2")
            .unwrap();

        let reviews = db.load_reviews("main", "feature-a").unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews["src/main.rs"].diff_hash, "hash2");
    }

    #[test]
    fn test_review_scope_isolation() {
        let (_dir, db) = test_db();

        db.store_review("main", "feature-a", "src/main.rs", "hash-a")
            .unwrap();
        db.store_review("main", "feature-b", "src/main.rs", "hash-b")
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

        db.store_review("main", "feat", "a.rs", "h1").unwrap();
        db.store_review("main", "feat", "b.rs", "h2").unwrap();

        db.remove_review("main", "feat", "a.rs").unwrap();

        let reviews = db.load_reviews("main", "feat").unwrap();
        assert_eq!(reviews.len(), 1);
        assert!(reviews.contains_key("b.rs"));
    }

    #[test]
    fn test_clear_reviews() {
        let (_dir, db) = test_db();

        db.store_review("main", "feat-a", "a.rs", "h1").unwrap();
        db.store_review("main", "feat-a", "b.rs", "h2").unwrap();
        db.store_review("main", "feat-b", "a.rs", "h3").unwrap();

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
                line_start: 10,
                line_end: 12,
                char_start: None,
                char_end: None,
                anchor_text: "fn foo() {}".to_string(),
                context_before: "// before".to_string(),
                context_after: "// after".to_string(),
                body: "This should handle errors".to_string(),
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
    }

    #[test]
    fn test_comment_char_selection() {
        let (_dir, db) = test_db();

        let mut c = simple_comment("a.rs", 5, "some_var", "Rename this");
        c.char_start = Some(10);
        c.char_end = Some(20);
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
        db.resolve_comment(c3.id).unwrap();

        // All unresolved
        let comments = db.list_comments("main", "feat", None, false).unwrap();
        assert_eq!(comments.len(), 2);

        // All including resolved
        let comments = db.list_comments("main", "feat", None, true).unwrap();
        assert_eq!(comments.len(), 3);

        // Filtered by file, unresolved only
        let comments = db
            .list_comments("main", "feat", Some("a.rs"), false)
            .unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].body, "comment 1");

        // Filtered by file, including resolved
        let comments = db
            .list_comments("main", "feat", Some("a.rs"), true)
            .unwrap();
        assert_eq!(comments.len(), 2);
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

        db.resolve_comment(comment.id).unwrap();
        let fetched = db.get_comment(comment.id).unwrap().unwrap();
        assert!(fetched.resolved);

        db.unresolve_comment(comment.id).unwrap();
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
    fn test_timestamps_iso8601() {
        let (_dir, db) = test_db();

        let review = db.store_review("main", "feat", "a.rs", "hash").unwrap();

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
