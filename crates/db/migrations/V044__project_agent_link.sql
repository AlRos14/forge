CREATE TABLE IF NOT EXISTS project_agent_link (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL REFERENCES agent(id) ON DELETE CASCADE,
    linked_by_user_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(project_id, agent_id)
);
