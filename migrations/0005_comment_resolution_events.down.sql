ALTER TABLE comments
    ADD COLUMN resolved INTEGER NOT NULL DEFAULT 0;

UPDATE comments
SET resolved = 1
WHERE EXISTS (
    SELECT 1
    FROM comment_resolution_events event
    WHERE event.comment_id = comments.id
);

DROP INDEX idx_comment_resolution_events_comment;
DROP TABLE comment_resolution_events;
