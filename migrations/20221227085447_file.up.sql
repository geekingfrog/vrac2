-- Add up migration script here
CREATE TABLE IF NOT EXISTS file
( id INTEGER PRIMARY KEY NOT NULL
, token_id INTEGER NOT NULL
, attempt_counter INTEGER NOT NULL
, mime_type TEXT
-- identifier to allow different backend, like local filesystem, or S3
, backend_type TEXT NOT NULL
, backend_data TEXT NOT NULL -- JSON
, created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now', 'utc')) -- datetime
, completed_at TEXT -- datetime
, FOREIGN KEY(token_id) REFERENCES token(id)
) STRICT;
