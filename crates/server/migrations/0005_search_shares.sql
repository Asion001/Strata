-- Name search and public share links.
CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE INDEX nodes_name_trgm ON nodes USING gin (name gin_trgm_ops);

CREATE TABLE shares (
    id            BIGSERIAL PRIMARY KEY,
    token         TEXT NOT NULL UNIQUE,
    owner_id      BIGINT NOT NULL REFERENCES users ON DELETE CASCADE,
    path          TEXT NOT NULL,
    -- What `path` resolved to when shared. If the owner's path later points
    -- elsewhere (renamed, remounted), the link stops working instead of
    -- serving whatever is there now.
    volume_id     BIGINT NOT NULL REFERENCES volumes ON DELETE CASCADE,
    rel_path      TEXT NOT NULL,
    -- The shared file or folder itself (NULL when a mount root is shared).
    -- If the item goes away the link stops working (the row stays, so the
    -- owner sees it); a different item at the same path never inherits it.
    node_id       BIGINT REFERENCES nodes ON DELETE SET NULL,
    is_dir        BOOLEAN NOT NULL,
    password_hash TEXT,
    expires_at    TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX shares_owner ON shares (owner_id);
