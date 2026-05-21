CREATE TABLE task_terminal_session (
    id                 TEXT PRIMARY KEY,
    task_id            TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    workspace_id       TEXT NOT NULL REFERENCES workspace(id) ON DELETE CASCADE,
    daemon_id          TEXT REFERENCES daemon(id) ON DELETE SET NULL,
    status             TEXT NOT NULL CHECK (status IN ('starting', 'running', 'exited', 'terminated', 'timed_out', 'orphaned', 'cleanup_terminated')),
    rows               INTEGER NOT NULL CHECK (rows >= 1),
    cols               INTEGER NOT NULL CHECK (cols >= 1),
    pid                INTEGER,
    exit_code          INTEGER,
    exit_signal        TEXT,
    exit_reason        TEXT,
    created_by_user_id TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    created_at         TEXT NOT NULL,
    started_at         TEXT,
    last_activity_at   TEXT,
    ended_at           TEXT,
    version            INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1)
);

CREATE INDEX idx_task_terminal_session_task_created ON task_terminal_session(task_id, created_at);
CREATE INDEX idx_task_terminal_session_task_status ON task_terminal_session(task_id, status);
CREATE INDEX idx_task_terminal_session_user_status ON task_terminal_session(created_by_user_id, status);
CREATE INDEX idx_task_terminal_session_workspace_status ON task_terminal_session(workspace_id, status);
