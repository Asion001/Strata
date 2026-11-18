-- Where a connection's files physically live (computed from its config when
-- it is saved): connections with the same storage_id reach the same files,
-- so their folders can overlap. Lets saves find overlapping volumes without
-- decrypting every connection.
ALTER TABLE connections
    ADD COLUMN storage_id     TEXT,
    ADD COLUMN storage_folder TEXT;
