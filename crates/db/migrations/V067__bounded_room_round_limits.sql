-- Persist the explicit fan-out limit and optional synthesis identity for
-- bounded Room rounds.  V061 intentionally keeps the base tables minimal;
-- this additive migration preserves all existing Room/round data.
ALTER TABLE bounded_room_round
    ADD COLUMN max_participants INTEGER NOT NULL DEFAULT 1
        CHECK (max_participants BETWEEN 1 AND 16);

ALTER TABLE bounded_room_round
    ADD COLUMN synthesis_identity_id TEXT
        REFERENCES agent_identity(id) ON DELETE SET NULL;

CREATE INDEX idx_bounded_room_round_room_status
    ON bounded_room_round(room_id, status, deadline_at, id);

CREATE INDEX idx_bounded_room_round_participant_job
    ON bounded_room_round_participant(turn_job_id);
