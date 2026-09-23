use crate::ExecutorKind;
use api_types::EffectiveExecutionPolicy;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

/// Build the common policy snapshot after a HarnessAdapter has interpreted
/// its own config into generic permission and isolation labels. High-risk
/// classification stays here with Forge's deterministic policy authority.
pub fn from_harness_interpretation(
    executor_kind: &ExecutorKind,
    permission_policy: &str,
    isolation_posture: &str,
    effective_cwd: Option<&str>,
    workspace_root: Option<&str>,
    config: &Value,
) -> EffectiveExecutionPolicy {
    EffectiveExecutionPolicy {
        executor_kind: executor_kind.to_string(),
        permission_policy: permission_policy.to_owned(),
        isolation_posture: isolation_posture.to_owned(),
        is_high_risk: matches!(
            isolation_posture,
            "danger-full-access" | "dangerously_skip_permissions" | "force"
        ),
        effective_cwd: effective_cwd.map(str::to_owned),
        workspace_root: workspace_root.map(str::to_owned),
        environment_posture: if config
            .get("env")
            .and_then(Value::as_object)
            .is_some_and(|env| !env.is_empty())
        {
            "custom".to_owned()
        } else {
            "inherited".to_owned()
        },
        scoped_tools: collect_string_values(config, config, "scoped_tools"),
        mcp_servers: collect_string_values(config, config, "mcp_servers"),
    }
}

pub fn validate_workspace_policy(
    effective_cwd: Option<&str>,
    workspace_root: Option<&str>,
    isolation_posture: &str,
) -> Result<(), WorkspacePolicyError> {
    let cwd = effective_cwd
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| WorkspacePolicyError::PathResolutionFailed {
            path: effective_cwd.unwrap_or_default().to_owned(),
            reason: "effective cwd is required".to_owned(),
        })?;

    if isolation_posture.contains("workspace-write")
        && workspace_root
            .map(|path| path.trim().is_empty())
            .unwrap_or(true)
    {
        return Err(WorkspacePolicyError::MissingWorkspaceRoot);
    }

    let canonical_cwd = resolve_path(cwd)?;
    let Some(root) = workspace_root.filter(|path| !path.trim().is_empty()) else {
        return Ok(());
    };
    let canonical_root = resolve_path(root)?;

    if !canonical_cwd.starts_with(&canonical_root) {
        return Err(WorkspacePolicyError::CwdOutsideWorkspace {
            cwd: canonical_cwd.display().to_string(),
            workspace_root: canonical_root.display().to_string(),
        });
    }

    Ok(())
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WorkspacePolicyError {
    #[error("cwd outside workspace: {cwd} is not under {workspace_root}")]
    CwdOutsideWorkspace { cwd: String, workspace_root: String },
    #[error("workspace-write isolation requires a workspace root")]
    MissingWorkspaceRoot,
    #[error("failed to resolve path {path}: {reason}")]
    PathResolutionFailed { path: String, reason: String },
}

fn collect_string_values(config: &Value, config_snapshot: &Value, key: &str) -> Vec<String> {
    let Some(value) = config.get(key).or_else(|| config_snapshot.get(key)) else {
        return Vec::new();
    };

    match value {
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Value::Object(items) => items.keys().cloned().collect(),
        Value::String(item) => vec![item.clone()],
        _ => Vec::new(),
    }
}

fn resolve_path(path: &str) -> Result<PathBuf, WorkspacePolicyError> {
    let path_ref = Path::new(path);
    match std::fs::canonicalize(path_ref) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => absolute_lexical(path_ref)
            .map_err(|error| WorkspacePolicyError::PathResolutionFailed {
                path: path.to_owned(),
                reason: error.to_string(),
            }),
        Err(error) => Err(WorkspacePolicyError::PathResolutionFailed {
            path: path.to_owned(),
            reason: error.to_string(),
        }),
    }
}

fn absolute_lexical(path: &Path) -> std::io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    Ok(normalize_lexical(&absolute))
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn core_classifies_adapter_reported_high_risk_posture() {
        let policy = from_harness_interpretation(
            &ExecutorKind::Codex,
            "auto",
            "danger-full-access",
            Some("/tmp/workspace/task/repo"),
            Some("/tmp/workspace"),
            &json!({"env": {}}),
        );
        assert!(policy.is_high_risk);
        assert_eq!(policy.isolation_posture, "danger-full-access");
        assert_eq!(policy.effective_cwd.as_deref(), Some("/tmp/workspace/task/repo"));
        assert_eq!(policy.workspace_root.as_deref(), Some("/tmp/workspace"));
    }

    #[test]
    fn permission_plan_does_not_create_native_planning_or_risk() {
        let policy = from_harness_interpretation(
            &ExecutorKind::Shell,
            "plan",
            "not_applicable",
            None,
            None,
            &json!({}),
        );
        assert_eq!(policy.permission_policy, "plan");
        assert!(!policy.is_high_risk);
    }

    #[test]
    fn validates_cwd_inside_workspace() {
        let workspace = tempfile::tempdir().expect("workspace root creates");
        let cwd = workspace.path().join("task").join("repo");
        std::fs::create_dir_all(&cwd).expect("cwd creates");

        let result =
            validate_workspace_policy(cwd.to_str(), workspace.path().to_str(), "workspace-write");

        assert!(result.is_ok());
    }

    #[test]
    fn rejects_cwd_outside_workspace() {
        let workspace = tempfile::tempdir().expect("workspace root creates");
        let outside = tempfile::tempdir().expect("outside root creates");

        let result = validate_workspace_policy(
            outside.path().to_str(),
            workspace.path().to_str(),
            "workspace-write",
        );

        assert!(matches!(
            result,
            Err(WorkspacePolicyError::CwdOutsideWorkspace { .. })
        ));
    }

    #[test]
    fn rejects_workspace_write_without_root() {
        let workspace = tempfile::tempdir().expect("workspace root creates");

        let result = validate_workspace_policy(workspace.path().to_str(), None, "workspace-write");

        assert_eq!(result, Err(WorkspacePolicyError::MissingWorkspaceRoot));
    }
}
