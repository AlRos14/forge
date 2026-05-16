CREATE TABLE user (
    id TEXT PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    display_name TEXT CHECK(length(display_name) <= 255),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE refresh_token (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    family_id TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX idx_refresh_token_user_id ON refresh_token(user_id);
CREATE INDEX idx_refresh_token_family_id ON refresh_token(family_id);
