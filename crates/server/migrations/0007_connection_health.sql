-- Health probes: state ok | degraded | offline (plus 'unknown' before the
-- first probe), consecutive failures, last latency, and a reason anyone may
-- see next to the error detail only admins see.
ALTER TABLE connections
    ADD COLUMN health_failures   INT NOT NULL DEFAULT 0,
    ADD COLUMN health_latency_ms INT,
    ADD COLUMN health_reason     TEXT,
    -- Local connections: the probes wrote `.strata-probe/conn-<id>` in the
    -- folder; from then on a folder without it counts as missing.
    ADD COLUMN root_marker       BOOLEAN NOT NULL DEFAULT FALSE;
UPDATE connections SET health = 'degraded' WHERE health = 'error';
