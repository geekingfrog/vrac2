DROP TABLE IF EXISTS file;

ALTER TABLE token DROP COLUMN attempt_counter;
ALTER TABLE token DROP COLUMN used_at;
