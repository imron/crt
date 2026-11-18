CREATE TABLE comment_resolution_events (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    comment_id          INTEGER NOT NULL,
    resolved_at         TEXT NOT NULL,
    resolved_commit     TEXT NOT NULL,
    resolved_head_ref   TEXT NOT NULL,
    resolved_merge_base TEXT NOT NULL,
    file_path           TEXT NOT NULL,
    line_start          INTEGER NOT NULL,
    line_end            INTEGER NOT NULL,
    char_start          INTEGER,
    char_end            INTEGER,
    anchor_text         TEXT NOT NULL,
    context_before      TEXT NOT NULL,
    context_after       TEXT NOT NULL,
    anchor_status       TEXT NOT NULL,
    FOREIGN KEY (comment_id) REFERENCES comments(id) ON DELETE CASCADE
);

CREATE INDEX idx_comment_resolution_events_comment
    ON comment_resolution_events (comment_id, resolved_at, id);

INSERT INTO comment_resolution_events
    (comment_id, resolved_at, resolved_commit, resolved_head_ref,
     resolved_merge_base, file_path, line_start, line_end, char_start,
     char_end, anchor_text, context_before, context_after, anchor_status)
SELECT c.id, c.updated_at, '', c.head_ref, c.merge_base, c.file_path,
       a.line_start, a.line_end, a.char_start, a.char_end, a.anchor_text,
       a.context_before, a.context_after, a.status
FROM comments c
JOIN v_current_anchors a ON a.comment_id = c.id
WHERE c.resolved != 0;

ALTER TABLE comments DROP COLUMN resolved;
