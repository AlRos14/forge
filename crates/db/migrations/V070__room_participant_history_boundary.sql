-- A participant may be admitted with an exclusive Room timeline floor.
-- NULL retains the historical full-history behavior for existing/owner rows.
ALTER TABLE room_participant
    ADD COLUMN history_after_sequence INTEGER
        CHECK (history_after_sequence IS NULL OR history_after_sequence >= 0);

CREATE INDEX idx_room_participant_history_boundary
    ON room_participant(room_id, participant_type, participant_id, history_after_sequence);
