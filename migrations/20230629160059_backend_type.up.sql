ALTER TABLE token
ADD COLUMN backend_type TEXT DEFAULT "local_fs" NOT NULL;
