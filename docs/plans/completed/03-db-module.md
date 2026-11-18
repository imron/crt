# Stage 3: Database Module

## Goal

Implement the SQLite persistence layer for storing and retrieving file
review state and comments. The database is per-repository, stored in the
main repo root, and scoped by `(base_ref, head_ref)`.

## Why

Review state must survive across invocations. When the user runs `crt main`,
marks files as reviewed, quits, and later runs `crt main` again, the
reviewed files should still show as reviewed (provided their diffs haven't
changed). SQLite gives us reliable, structured storage for this with zero
configuration. The per-repo database design means each repository's state
is isolated and naturally follows the repo if it's moved.

## Requirements

1. **Database location**: the database lives at `<repo_root>/.crt/reviews.db`
   where `<repo_root>` is the main repository root (not the worktree path).
   This is resolved via the git module's worktree resolution.

2. **Database initialization**: on first access, create the SQLite database
   and both tables if they do not exist. Use `CREATE TABLE IF NOT EXISTS`.
   Enable WAL mode for concurrent read/write performance (server + multiple
   clients).

3. **Schema**:
   ```sql
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
   ```

4. **Review operations**:
   - Store review: upsert a row in `file_reviews`.
   - Load reviews: return all reviews for a `(base_ref, head_ref)` pair.
   - Remove single review: delete a specific file's review.
   - Clear reviews: delete all rows for a `(base_ref, head_ref)` pair.

5. **Comment operations**:
   - Create comment: insert and return the new ID.
   - List comments: by `(base_ref, head_ref)`, optionally filtered by
     `file_path` and/or `include_resolved`.
   - Get comment by ID.
   - Update comment body.
   - Resolve / unresolve comment.
   - Delete comment.

6. **Timestamps**: stored as ISO 8601 strings with timezone offset
   (e.g. `2026-03-29T14:30:00+10:00`). Use the local timezone.

7. **Error handling**: all database operations should return `Result` types.
   Database errors should propagate with context (e.g. "failed to open
   review database at .crt/reviews.db: ...").

## Acceptance Criteria

- [ ] On first run, `.crt/reviews.db` is created in the main repo root
      with the correct schema and WAL mode enabled.
- [ ] Storing a review and loading it back returns the same data.
- [ ] Reviews for `("main", "feature-a")` are isolated from reviews for
      `("main", "feature-b")`.
- [ ] Storing a review for the same `(base_ref, head_ref, file_path)`
      replaces the previous entry (upsert behavior).
- [ ] Loading reviews for a scope with no stored data returns an empty
      result.
- [ ] Clearing reviews removes all entries for that `(base_ref, head_ref)`
      pair and does not affect other scopes.
- [ ] Comment CRUD operations (create, read, update, delete) all work.
- [ ] Resolve and unresolve toggle the `resolved` flag correctly.
- [ ] Timestamps are in ISO 8601 format with timezone.
- [ ] Database errors produce human-readable error messages.
- [ ] Running from a worktree creates the DB in the main repo root, not
      in the worktree.

## Open Questions

- Do we need a schema migration strategy for future changes? Should we
  store a schema version somewhere?
- Should the DB module be the only code that touches SQLite, or should the
  server's API layer also run queries directly?
- Should we index `comments` on `(base_ref, head_ref, file_path)` for
  query performance?
