-- Add up migration script here
CREATE TABLE IF NOT EXISTS file
( id INTEGER PRIMARY KEY NOT NULL
, token_id INTEGER NOT NULL
, attempt_counter INTEGER NOT NULL
, mime_type TEXT
-- identifier to allow different backend, like local filesystem, or S3
, backend_type TEXT
, backend_data TEXT -- JSON
, created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now', 'utc')) -- datetime
, completed_at TEXT -- datetime
, FOREIGN KEY(token_id) REFERENCES token(id)
) STRICT;

-- because we can't resume aborted upload, a strictly increasing counter
-- is associated to uploaded files, and that allow one to delete stray
-- files from previous attempts, without having to invalidate the token.
ALTER TABLE token ADD COLUMN attempt_counter INTEGER DEFAULT 0;
ALTER TABLE token ADD COLUMN used_at TEXT; -- datetime
