use std::path::PathBuf;

pub struct LifecycleHookContext {
    pub event: api_types::LifecycleEvent,
    pub task_id: String,
    pub task_title: String,
    pub task_status: String,
    pub previous_status: String,
    pub project_id: String,
    pub project_name: String,
    pub repo_path: String,
    pub worktree_path: Option<String>,
    pub agent_id: Option<String>,
    pub execution_id: Option<String>,
    pub log_dir: Option<PathBuf>,
}
