DROP INDEX idx_comments_scope;

ALTER TABLE file_reviews RENAME COLUMN base_ref TO merge_base;
ALTER TABLE comments RENAME COLUMN base_ref TO merge_base;

CREATE INDEX idx_comments_scope
    ON comments (merge_base, head_ref, file_path);
