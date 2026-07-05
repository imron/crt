CREATE TABLE comment_resolution_anchor_segments (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    resolution_event_id   INTEGER NOT NULL,
    comment_id            INTEGER NOT NULL,
    side                  TEXT NOT NULL,
    file_path             TEXT NOT NULL,
    line_start            INTEGER NOT NULL,
    line_end              INTEGER NOT NULL,
    char_start            INTEGER,
    char_end              INTEGER,
    anchor_text           TEXT NOT NULL,
    context_before        TEXT NOT NULL,
    context_after         TEXT NOT NULL,
    placement_status      TEXT NOT NULL,
    match_method          TEXT NOT NULL,
    created_at            TEXT NOT NULL,
    FOREIGN KEY (resolution_event_id) REFERENCES comment_resolution_events(id)
        ON DELETE CASCADE,
    FOREIGN KEY (comment_id) REFERENCES comments(id) ON DELETE CASCADE
);

CREATE INDEX idx_comment_resolution_anchor_segments_event
    ON comment_resolution_anchor_segments (resolution_event_id, side);

CREATE INDEX idx_comment_resolution_anchor_segments_comment
    ON comment_resolution_anchor_segments (comment_id);
