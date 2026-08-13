ALTER TABLE protected_interaction ADD COLUMN request_ciphertext BLOB;
ALTER TABLE protected_interaction ADD COLUMN request_nonce BLOB;
ALTER TABLE protected_interaction ADD COLUMN request_fingerprint TEXT;

CREATE INDEX idx_protected_interaction_session_status
    ON protected_interaction(session_id, status, created_at);
