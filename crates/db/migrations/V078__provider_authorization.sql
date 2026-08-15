ALTER TABLE credential_handle
    ADD COLUMN credential_method TEXT NOT NULL DEFAULT 'api_key'
        CHECK (credential_method IN ('api_key', 'oauth_bundle'));

ALTER TABLE credential_handle
    ADD COLUMN metadata_json TEXT NOT NULL DEFAULT '{}';

ALTER TABLE credential_handle
    ADD COLUMN version INTEGER NOT NULL DEFAULT 1;

CREATE TABLE provider_authorization_operation (
    id                      TEXT PRIMARY KEY,
    owner_user_id           TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    provider                TEXT NOT NULL
                                CHECK (provider IN (
                                    'openai', 'xai', 'gemini',
                                    'openrouter', 'openai_compatible'
                                )),
    method                  TEXT NOT NULL
                                CHECK (method IN ('browser_oauth', 'device_oauth')),
    status                  TEXT NOT NULL
                                CHECK (status IN (
                                    'starting', 'awaiting_browser', 'awaiting_device',
                                    'polling', 'exchanging', 'verifying', 'publishing',
                                    'succeeded', 'denied', 'expired', 'cancelled', 'failed'
                                )),
    authorization_url       TEXT,
    user_code               TEXT,
    redirect_origin         TEXT NOT NULL,
    callback_state_hash     TEXT,
    request_json            TEXT NOT NULL,
    poll_interval_seconds   INTEGER NOT NULL DEFAULT 5,
    expires_at              TEXT NOT NULL,
    profile_id              TEXT REFERENCES agent_profile(id) ON DELETE SET NULL,
    credential_handle_id    TEXT REFERENCES credential_handle(id) ON DELETE SET NULL,
    error_code              TEXT,
    error_message           TEXT,
    version                 INTEGER NOT NULL DEFAULT 1,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL,
    completed_at            TEXT
);

CREATE INDEX idx_provider_authorization_owner
    ON provider_authorization_operation(owner_user_id, created_at DESC);

CREATE UNIQUE INDEX idx_provider_authorization_callback_state
    ON provider_authorization_operation(callback_state_hash)
    WHERE callback_state_hash IS NOT NULL;

CREATE TABLE protected_provider_authorization_state (
    operation_id            TEXT PRIMARY KEY
                                REFERENCES provider_authorization_operation(id) ON DELETE CASCADE,
    ciphertext              BLOB NOT NULL,
    nonce                   BLOB NOT NULL,
    key_revision            INTEGER NOT NULL,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL
);
