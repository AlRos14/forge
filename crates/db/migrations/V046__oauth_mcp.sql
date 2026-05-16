CREATE TABLE oauth_client (
    id TEXT PRIMARY KEY,
    client_id TEXT NOT NULL UNIQUE,
    client_name TEXT,
    redirect_uris_json TEXT NOT NULL,
    token_endpoint_auth_method TEXT NOT NULL DEFAULT 'none',
    created_at TEXT NOT NULL,
    last_used_at TEXT
);

CREATE TABLE oauth_authorization_code (
    id TEXT PRIMARY KEY,
    code_hash TEXT NOT NULL UNIQUE,
    user_id TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    client_id TEXT NOT NULL REFERENCES oauth_client(client_id) ON DELETE CASCADE,
    redirect_uri TEXT NOT NULL,
    code_challenge TEXT NOT NULL,
    code_challenge_method TEXT NOT NULL,
    resource TEXT NOT NULL,
    scopes TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    consumed_at TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX idx_oauth_authorization_code_user_id ON oauth_authorization_code(user_id);
CREATE INDEX idx_oauth_authorization_code_client_id ON oauth_authorization_code(client_id);
CREATE INDEX idx_oauth_authorization_code_expires_at ON oauth_authorization_code(expires_at);

CREATE TABLE oauth_refresh_token (
    id TEXT PRIMARY KEY,
    token_hash TEXT NOT NULL UNIQUE,
    family_id TEXT NOT NULL,
    user_id TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    client_id TEXT NOT NULL REFERENCES oauth_client(client_id) ON DELETE CASCADE,
    resource TEXT NOT NULL,
    scopes TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    revoked_at TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX idx_oauth_refresh_token_family_id ON oauth_refresh_token(family_id);
CREATE INDEX idx_oauth_refresh_token_user_id ON oauth_refresh_token(user_id);
CREATE INDEX idx_oauth_refresh_token_client_id ON oauth_refresh_token(client_id);
