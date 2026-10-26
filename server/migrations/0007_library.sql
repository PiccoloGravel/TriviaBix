-- Songs from the operator's own music library (synced from Lidarr). A library song has a
-- `file_path` the server can stream; curated songs that are also in the library get one too and
-- play from the file instead of an Apple preview.
ALTER TABLE songs ADD COLUMN source TEXT NOT NULL DEFAULT 'hits' CHECK (source IN ('hits', 'library'));
ALTER TABLE songs ADD COLUMN file_path TEXT;
ALTER TABLE songs ADD COLUMN album TEXT;
ALTER TABLE songs ADD COLUMN genre TEXT;
ALTER TABLE songs ADD COLUMN duration_ms INTEGER;
CREATE INDEX songs_file_path ON songs(file_path) WHERE file_path IS NOT NULL;
