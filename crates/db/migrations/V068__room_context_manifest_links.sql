-- Link each canonical Room response to the immutable redaction-safe context
-- manifest that produced it.  Existing messages are historical and remain
-- nullable; native/legacy turns populate this only at response admission.
ALTER TABLE room_message
    ADD COLUMN context_manifest_id TEXT
        REFERENCES context_manifest(id) ON DELETE SET NULL;

CREATE INDEX idx_room_message_context_manifest
    ON room_message(context_manifest_id);
