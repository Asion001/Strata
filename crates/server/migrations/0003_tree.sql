-- A volume is (connection, folder inside it, mode). Its tree (hashed) or
-- listing cache (raw) lives in `nodes`. Several mounts may show one volume,
-- and removing a mount never deletes a volume's data.
CREATE TABLE volumes (
    id            BIGSERIAL PRIMARY KEY,
    connection_id BIGINT NOT NULL REFERENCES connections ON DELETE RESTRICT,
    base_path     TEXT NOT NULL DEFAULT '',
    mode          TEXT NOT NULL CHECK (mode IN ('raw', 'hashed')),
    -- Last journal sequence number used, and the newest one pruned.
    journal_seq   BIGINT NOT NULL DEFAULT 0,
    journal_floor BIGINT NOT NULL DEFAULT 0,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (connection_id, base_path, mode)
);

-- A mount shows a volume at a folder in a user's tree. The root mount has path ''.
CREATE TABLE mounts (
    id          BIGSERIAL PRIMARY KEY,
    owner_id    BIGINT NOT NULL REFERENCES users ON DELETE CASCADE,
    path        TEXT NOT NULL,
    parent_path TEXT,          -- NULL for the root mount
    name        TEXT NOT NULL, -- '' for the root mount
    volume_id   BIGINT NOT NULL REFERENCES volumes ON DELETE RESTRICT,
    -- The volume's journal_seq when mounted: older history is not replayed.
    journal_from BIGINT NOT NULL DEFAULT 0,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (owner_id, path)
);
CREATE INDEX mounts_parent ON mounts (owner_id, parent_path);
CREATE INDEX mounts_volume ON mounts (volume_id);

-- Files and folders of a volume. Paths are relative to the volume:
-- `parent` is '' for top-level entries.
CREATE TABLE nodes (
    id           BIGSERIAL PRIMARY KEY,
    volume_id    BIGINT NOT NULL REFERENCES volumes ON DELETE CASCADE,
    parent       TEXT NOT NULL,
    name         TEXT NOT NULL,
    is_dir       BOOLEAN NOT NULL,
    size         BIGINT NOT NULL DEFAULT 0,
    mtime        TIMESTAMPTZ NOT NULL DEFAULT now(),
    etag         TEXT,
    content_hash BYTEA,   -- hashed: BLAKE3 over the chunk hashes
    chunks       BYTEA[], -- hashed: BLAKE3 hash of each chunk, in order
    chunk_sizes  BIGINT[],
    UNIQUE (volume_id, parent, name)
);
CREATE INDEX nodes_parent_prefix ON nodes (volume_id, parent text_pattern_ops);
CREATE INDEX nodes_chunks ON nodes USING gin (chunks);

-- When each raw folder was last listed from the backend.
CREATE TABLE raw_scans (
    volume_id  BIGINT NOT NULL REFERENCES volumes ON DELETE CASCADE,
    dir        TEXT NOT NULL,
    scanned_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (volume_id, dir)
);

-- Change journal, kept per storage location (volume) so a folder seen by
-- several users is recorded once. Mount-level changes (a mount added, moved or
-- removed) belong to one user's tree and carry user_id instead. Sequence
-- numbers are per scope, gap-free and allocated under a row lock in the same
-- transaction as the insert, so they commit in order. A user's feed merges the
-- journals of the volumes they mount with their own rows; the cursor is an
-- opaque map of scope -> seq.
ALTER TABLE users
    ADD COLUMN journal_seq   BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN journal_floor BIGINT NOT NULL DEFAULT 0;

CREATE TABLE journal (
    id        BIGSERIAL PRIMARY KEY,
    volume_id BIGINT REFERENCES volumes ON DELETE CASCADE,
    user_id   BIGINT REFERENCES users ON DELETE CASCADE,
    seq       BIGINT NOT NULL,
    op        TEXT NOT NULL CHECK (op IN ('create', 'modify', 'delete', 'move', 'mount_added', 'mount_removed')),
    path      TEXT NOT NULL,
    new_path  TEXT,
    is_dir    BOOLEAN NOT NULL,
    at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((volume_id IS NULL) <> (user_id IS NULL)),
    UNIQUE (volume_id, seq),
    UNIQUE (user_id, seq)
);
CREATE INDEX journal_at ON journal (at);
