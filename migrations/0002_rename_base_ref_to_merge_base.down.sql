DROP INDEX idx_comments_scope;

ALTER TABLE file_reviews RENAME COLUMN merge_base TO base_ref;
ALTER TABLE comments RENAME COLUMN merge_base TO base_ref;

CREATE INDEX idx_comments_scope
    ON comments (base_ref, head_ref, file_path);
