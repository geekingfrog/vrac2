CREATE TABLE IF NOT EXISTS account
( id INTEGER PRIMARY KEY NOT NULL
, username TEXT NOT NULL
, phc TEXT NOT NULL
, UNIQUE(username)
) STRICT;
