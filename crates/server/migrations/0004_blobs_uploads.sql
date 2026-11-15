-- Blobs present in a hashed volume's store.
CREATE TABLE blobs (
    volume_id  BIGINT NOT NULL REFERENCES volumes ON DELETE CASCADE,
    hash       BYTEA NOT NULL,
    size       BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Set by GC before it deletes the object. A writer that finds the flag
    -- writes the object again and clears it (see vfs::store_blob).
    deleting   BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (volume_id, hash)
);

-- Which users stored which blobs. The upload negotiation only reveals chunks
-- the asking user stored themselves, never whether someone else has a file.
CREATE TABLE blob_owners (
    volume_id BIGINT NOT NULL,
    hash      BYTEA NOT NULL,
    user_id   BIGINT NOT NULL REFERENCES users ON DELETE CASCADE,
    PRIMARY KEY (volume_id, hash, user_id),
    FOREIGN KEY (volume_id, hash) REFERENCES blobs ON DELETE CASCADE
);

-- Resumable uploads in progress.
CREATE TABLE uploads (
    id          TEXT PRIMARY KEY,
    user_id     BIGINT NOT NULL REFERENCES users ON DELETE CASCADE,
    path        TEXT NOT NULL,
    volume_id   BIGINT NOT NULL REFERENCES volumes ON DELETE CASCADE,
    size        BIGINT NOT NULL,
    chunks      BYTEA[] NOT NULL,
    chunk_sizes BIGINT[] NOT NULL,
    mtime       TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX uploads_chunks ON uploads USING gin (chunks);

