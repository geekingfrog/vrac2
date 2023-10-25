CREATE TABLE IF NOT EXISTS file_metadata
( file_id INTEGER PRIMARY KEY NOT NULL
, size_b INTEGER
, mime_type TEXT
, FOREIGN KEY(file_id) REFERENCES file(id)
) STRICT;
