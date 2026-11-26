-- Name conflicts (move, copy and uploads onto a taken name).

-- What to do when the name is taken, as asked at create (a commit can
-- answer anew), and where a committed upload landed (a commit sent again
-- gets the same answer, never a second copy).
ALTER TABLE uploads
    ADD COLUMN on_conflict    TEXT NOT NULL DEFAULT 'replace',
    ADD COLUMN committed_path TEXT;

-- An item a replace is writing beside the old one, under a hidden name,
-- until it takes the old one's name. A row left by a crash is settled by
-- `conflicts::reconcile`: part of a move is shown beside the old item (it is
-- the user's only copy), part of a copy goes. The operation writing it
-- refreshes `alive_at`, so no server settles a replace still running.
CREATE TABLE staged_items (
    id         BIGSERIAL PRIMARY KEY,
    user_id    BIGINT NOT NULL REFERENCES users ON DELETE CASCADE,
    path       TEXT NOT NULL,
    target     TEXT NOT NULL,
    is_dir     BOOLEAN NOT NULL,
    is_move    BOOLEAN NOT NULL,
    alive_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
