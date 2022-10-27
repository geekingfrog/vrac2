CREATE TABLE IF NOT EXISTS token
( id INTEGER PRIMARY KEY NOT NULL
, path TEXT NOT NULL
, max_size_mib INTEGER
, valid_until TEXT NOT NULL -- datetime
, content_expires_after_hours INTEGER
, created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now', 'utc')) -- datetime
, deleted_at TEXT -- datetime
) STRICT;
