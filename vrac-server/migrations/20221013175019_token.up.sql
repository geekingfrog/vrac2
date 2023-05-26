CREATE TABLE IF NOT EXISTS token
( id INTEGER PRIMARY KEY NOT NULL
, path TEXT NOT NULL
, max_size_mib INTEGER
, valid_until TEXT NOT NULL -- datetime
, content_expires_after_hours INTEGER
, created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now', 'utc')) -- datetime
, deleted_at TEXT -- datetime
-- because we can't resume aborted upload, a strictly increasing counter
-- is associated to uploaded files, and that allow one to delete stray
-- files from previous attempts, without having to invalidate the token.
, attempt_counter INTEGER DEFAULT 0
, used_at TEXT -- datetime
, content_expires_at TEXT -- datetime
) STRICT;
