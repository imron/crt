CREATE TEMP TABLE inline_comments_migration AS
    SELECT id, merge_base, head_ref, file_path, line_start,
           line_end, char_start, char_end, anchor_text,
           context_before, context_after, body, resolved,
           created_at, updated_at
    FROM comments;

DROP TABLE comments;

CREATE TABLE comments (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    merge_base      TEXT NOT NULL,
    head_ref        TEXT NOT NULL,
    file_path       TEXT NOT NULL,
    body            TEXT NOT NULL,
    resolved        INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE TABLE anchor_versions (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    comment_id      INTEGER NOT NULL,
    file_blob_sha   TEXT NOT NULL,
    line_start      INTEGER NOT NULL,
    line_end        INTEGER NOT NULL,
    char_start      INTEGER,
    char_end        INTEGER,
    anchor_text     TEXT NOT NULL,
    context_before  TEXT NOT NULL,
    context_after   TEXT NOT NULL,
    status          TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    FOREIGN KEY (comment_id) REFERENCES comments(id) ON DELETE CASCADE
);

CREATE INDEX idx_comments_scope
    ON comments (merge_base, head_ref, file_path);

CREATE INDEX idx_anchor_versions_comment
    ON anchor_versions (comment_id, created_at, id);

CREATE VIEW v_current_anchors AS
    SELECT av.*
    FROM anchor_versions av
    WHERE NOT EXISTS (
        SELECT 1
        FROM anchor_versions newer
        WHERE newer.comment_id = av.comment_id
          AND (
            newer.created_at > av.created_at
            OR (newer.created_at = av.created_at AND newer.id > av.id)
          )
    );

INSERT INTO comments
    (id, merge_base, head_ref, file_path, body, resolved,
     created_at, updated_at)
SELECT id, merge_base, head_ref, file_path, body, resolved,
       created_at, updated_at
FROM inline_comments_migration;

INSERT INTO anchor_versions
    (comment_id, file_blob_sha, line_start, line_end, char_start,
     char_end, anchor_text, context_before, context_after, status,
     created_at)
SELECT id, '', line_start, line_end, char_start, char_end,
       anchor_text, context_before, context_after, 'anchored',
       created_at
FROM inline_comments_migration;

DROP TABLE inline_comments_migration;
