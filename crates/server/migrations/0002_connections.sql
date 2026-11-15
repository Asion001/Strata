-- A storage backend plus encrypted credentials.
CREATE TABLE connections (
    id                BIGSERIAL PRIMARY KEY,
    owner_id          BIGINT NOT NULL REFERENCES users ON DELETE CASCADE,
    name              TEXT NOT NULL,
    kind              TEXT NOT NULL CHECK (kind IN ('local', 's3', 'ftp')),
    config_enc        BYTEA NOT NULL,
    -- {"snapshots": "none|btrfs|zfs|s3_versioning|smb", "snapshots_create": bool, ...}
    capabilities      JSONB NOT NULL DEFAULT '{}',
    -- The server's default storage; new users get their root folder here.
    is_default        BOOLEAN NOT NULL DEFAULT FALSE,
    health            TEXT NOT NULL DEFAULT 'unknown',
    health_error      TEXT,
    health_checked_at TIMESTAMPTZ,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

