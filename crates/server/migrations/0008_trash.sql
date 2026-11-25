-- Trash: deleted items wait here until they are restored or purged.
--
-- Hashed volumes: the item's `nodes` rows move to `trash_nodes` (paths kept
-- as they were), so their blobs stay referenced until the purge.
-- Raw local volumes: the item is renamed to `<connection root>/.strata-trash/<id>/<name>`
-- on disk. `ready` is false between recording the row and that rename; a
-- crash in between is settled by `trash::reconcile`.
CREATE TABLE trash (
    id            BIGSERIAL PRIMARY KEY,
    owner_id      BIGINT NOT NULL REFERENCES users ON DELETE CASCADE,
    volume_id     BIGINT NOT NULL REFERENCES volumes ON DELETE CASCADE,
    -- Where it was in the owner's tree when deleted (shown to the user).
    original_path TEXT NOT NULL,
    -- Where it was inside the volume (where a restore puts it back).
    rel_path      TEXT NOT NULL,
    name          TEXT NOT NULL,
    is_dir        BOOLEAN NOT NULL,
    size          BIGINT NOT NULL DEFAULT 0,
    ready         BOOLEAN NOT NULL DEFAULT FALSE,
    -- Raw: the item's cache row id when trashed (its share links follow it back).
    node_id       BIGINT,
    deleted_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    purge_after   TIMESTAMPTZ NOT NULL
);
CREATE INDEX trash_owner ON trash (owner_id, id DESC);
CREATE INDEX trash_purge ON trash (purge_after);
CREATE INDEX trash_volume ON trash (volume_id);

CREATE TABLE trash_nodes (
    trash_id     BIGINT NOT NULL REFERENCES trash ON DELETE CASCADE,
    -- The row's `nodes.id`, put back on restore so share links still match.
    node_id      BIGINT NOT NULL,
    volume_id    BIGINT NOT NULL,
    parent       TEXT NOT NULL,
    name         TEXT NOT NULL,
    is_dir       BOOLEAN NOT NULL,
    size         BIGINT NOT NULL DEFAULT 0,
    mtime        TIMESTAMPTZ NOT NULL,
    etag         TEXT,
    content_hash BYTEA,
    chunks       BYTEA[],
    chunk_sizes  BIGINT[]
);
CREATE INDEX trash_nodes_trash ON trash_nodes (trash_id);
-- Blob GC checks these references per volume, like `nodes_chunks`.
CREATE INDEX trash_nodes_chunks ON trash_nodes USING gin (chunks);
CREATE INDEX trash_nodes_volume ON trash_nodes (volume_id);

-- Share links that pointed into a trashed item (deleting the rows set their
-- node_id to NULL); a restore to the same path points them back.
CREATE TABLE trash_shares (
    trash_id BIGINT NOT NULL REFERENCES trash ON DELETE CASCADE,
    share_id BIGINT NOT NULL REFERENCES shares ON DELETE CASCADE,
    node_id  BIGINT NOT NULL
);
CREATE INDEX trash_shares_trash ON trash_shares (trash_id);
CREATE INDEX trash_shares_share ON trash_shares (share_id);
