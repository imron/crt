CREATE TEMP TABLE current_comment_anchors AS
    SELECT c.id, c.merge_base, c.head_ref, c.file_path,
           a.line_start, a.line_end, a.char_start, a.char_end,
           a.anchor_text, a.context_before, a.context_after,
           c.body, c.resolved, c.created_at, c.updated_at
    FROM comments c
    JOIN v_current_anchors a ON a.comment_id = c.id;

DROP VIEW v_current_anchors;
DROP TABLE anchor_versions;
DROP TABLE comments;

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

CREATE INDEX idx_comments_scope
    ON comments (merge_base, head_ref, file_path);

INSERT INTO comments
    (id, merge_base, head_ref, file_path, line_start, line_end,
     char_start, char_end, anchor_text, context_before, context_after,
     body, resolved, created_at, updated_at)
SELECT id, merge_base, head_ref, file_path, line_start, line_end,
       char_start, char_end, anchor_text, context_before, context_after,
       body, resolved, created_at, updated_at
FROM current_comment_anchors;

DROP TABLE current_comment_anchors;
