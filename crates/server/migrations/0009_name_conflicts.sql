-- Name conflicts (move, copy and uploads onto a taken name).

-- What to do when the name is taken, as asked at create (a commit can
-- answer anew), and the answer a commit gave (a commit sent again gets the
-- same answer, never a second copy).
ALTER TABLE uploads
    ADD COLUMN on_conflict TEXT NOT NULL DEFAULT 'replace',
    ADD COLUMN committed   JSONB;

-- An item a replace is writing beside the old one, under a hidden name,
-- until it takes the old one's name. A row left by a crash is settled by
-- `conflicts::reconcile`: part of a move is shown beside the old item (it is
-- the user's only copy), part of a copy goes. The operation writing it
-- refreshes `alive_at`, so no server settles a replace still running.
CREATE TABLE staged_items (
    id         BIGSERIAL PRIMARY KEY,
    user_id    BIGINT NOT NULL REFERENCES users ON DELETE CASCADE,
    path       TEXT NOT NULL,
    -- Where it is in storage (`conflicts::claim_key_of`): the same item
    -- can show under several paths.
    key        TEXT NOT NULL,
    target     TEXT NOT NULL,
    is_dir     BOOLEAN NOT NULL,
    is_move    BOOLEAN NOT NULL,
    -- The whole item is there (only then can part of a copy take the name).
    written    BOOLEAN NOT NULL DEFAULT FALSE,
    alive_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX staged_items_key ON staged_items (key text_pattern_ops);
