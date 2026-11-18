DELETE FROM comment_resolution_events;
DELETE FROM anchor_versions;
DELETE FROM comments;

DROP VIEW IF EXISTS v_current_anchors;
DROP INDEX IF EXISTS idx_anchor_versions_comment;
DROP TABLE IF EXISTS anchor_versions;

ALTER TABLE comments
    ADD COLUMN created_head_commit TEXT NOT NULL DEFAULT '';

CREATE TABLE anchor_versions (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    comment_id          INTEGER NOT NULL,
    aggregate_status    TEXT NOT NULL,
    created_at          TEXT NOT NULL,
    FOREIGN KEY (comment_id) REFERENCES comments(id) ON DELETE CASCADE
);

CREATE TABLE anchor_segments (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    anchor_version_id   INTEGER NOT NULL,
    side                TEXT NOT NULL,
    file_path           TEXT NOT NULL,
    file_blob_sha       TEXT NOT NULL,
    line_start          INTEGER NOT NULL,
    line_end            INTEGER NOT NULL,
    char_start          INTEGER,
    char_end            INTEGER,
    anchor_text         TEXT NOT NULL,
    context_before      TEXT NOT NULL,
    context_after       TEXT NOT NULL,
    placement_status    TEXT NOT NULL,
    match_method        TEXT NOT NULL,
    created_at          TEXT NOT NULL,
    FOREIGN KEY (anchor_version_id) REFERENCES anchor_versions(id)
        ON DELETE CASCADE
);

CREATE INDEX idx_anchor_versions_comment
    ON anchor_versions (comment_id, created_at, id);

CREATE INDEX idx_anchor_segments_version
    ON anchor_segments (anchor_version_id, side);

CREATE VIEW v_current_anchor_versions AS
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
