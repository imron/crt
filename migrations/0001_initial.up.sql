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
