CREATE TABLE IF NOT EXISTS conversation (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    agent_id TEXT REFERENCES agent(id) ON DELETE SET NULL,
    title TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'archived')),
    system_prompt TEXT,
    message_count INTEGER NOT NULL DEFAULT 0,
    last_message_at TEXT,
    agent_session_id TEXT,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_conversation_project ON conversation(project_id, last_message_at DESC);

CREATE TABLE IF NOT EXISTS conversation_message (
    id TEXT PRIMARY KEY NOT NULL,
    conversation_id TEXT NOT NULL REFERENCES conversation(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system')),
    content TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('complete', 'streaming', 'failed', 'cancelled')),
    model TEXT,
    token_usage_json TEXT,
    duration_ms INTEGER,
    error TEXT,
    sequence INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(conversation_id, sequence)
);

CREATE INDEX IF NOT EXISTS idx_conversation_message_conv ON conversation_message(conversation_id, sequence ASC);
