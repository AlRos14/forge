use std::path::Path;

pub(crate) fn execution_logs_path(
    workspace_root: &Path,
    project_id: &str,
    task_id: &str,
    execution_id: &str,
) -> String {
    workspace_root
        .join(".forge")
        .join("logs")
        .join(project_id)
        .join(task_id)
        .join(format!("{execution_id}.jsonl"))
        .to_string_lossy()
        .into_owned()
}
