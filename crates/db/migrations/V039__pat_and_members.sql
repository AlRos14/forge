CREATE TABLE personal_access_token (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    prefix TEXT NOT NULL,
    scopes TEXT NOT NULL DEFAULT '*',
    expires_at TEXT,
    last_used_at TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX idx_pat_user_id ON personal_access_token(user_id);
CREATE INDEX idx_pat_token_hash ON personal_access_token(token_hash);

CREATE TABLE project_member (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    role TEXT NOT NULL DEFAULT 'member' CHECK(role IN ('owner', 'admin', 'member', 'viewer')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(project_id, user_id)
);

CREATE INDEX idx_project_member_project ON project_member(project_id);
CREATE INDEX idx_project_member_user ON project_member(user_id);
