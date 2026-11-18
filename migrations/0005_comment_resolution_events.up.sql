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
