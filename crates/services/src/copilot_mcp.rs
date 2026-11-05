//! Ephemeral native MCP settings. Credentials live only in the child environment.
use std::path::Path;

use executors::ExecutorKind;
use serde_json::{json, Value};

use crate::{Result, ServiceError};

pub struct CopilotMcpConfig {
    pub config: Value,
    pub runtime_env: Value,
}

pub async fn configure_copilot_mcp(
    kind: ExecutorKind,
    mut config: Value,
    context_dir: &Path,
    mcp_url: &str,
    token: &str,
) -> Result<CopilotMcpConfig> {
    let url = url::Url::parse(mcp_url)
        .map_err(|_| ServiceError::invalid_operation("Invalid Copilot MCP endpoint"))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ServiceError::invalid_operation(
            "Invalid Copilot MCP endpoint",
        ));
    }
    if kind != ExecutorKind::ClaudeCode {
        return Err(ServiceError::invalid_operation(
            "Project Copilot currently requires Claude Code's MCP-only tool controls",
        ));
    }
    let object = config.as_object_mut().ok_or_else(|| {
        ServiceError::invalid_operation("Copilot harness configuration must be an object")
    })?;
    // An explicit Agent identity is retained; fallback to a different harness is
    // not a delegation to that other native account.
    if object
        .get("fallbacks")
        .is_some_and(|value| value.as_array().is_some_and(|v| !v.is_empty()))
    {
        return Err(ServiceError::invalid_operation(
            "Copilot requires one explicit harness",
        ));
    }

    // These generic command channels can replace the executable, append flags
    // after Forge's restrictions, or alter the child environment. MCP-only
    // mode accepts the typed model/reasoning settings, but no opaque command
    // overrides from the Agent profile.
    for key in ["base_command_override", "additional_params", "env"] {
        let nonempty = match object.get(key) {
            None | Some(Value::Null) => false,
            Some(Value::String(value)) if key == "base_command_override" => !value.is_empty(),
            Some(Value::Array(values)) if key == "additional_params" => !values.is_empty(),
            Some(Value::Object(values)) if key == "env" => !values.is_empty(),
            Some(_) => true,
        };
        if nonempty {
            return Err(ServiceError::invalid_operation(format!(
                "Project Copilot does not accept Agent {key} overrides"
            )));
        }
    }
    if object.get("claude_code_router").and_then(Value::as_bool) == Some(true) {
        return Err(ServiceError::invalid_operation(
            "Project Copilot does not accept Claude Code Router overrides",
        ));
    }

    // These Agent settings may widen native permissions or turn the run into a
    // read-only planning session. Copilot is a management run whose only
    // callable tools are the delegated Project MCP tools configured below.
    object.insert("permission_policy".to_owned(), json!("supervised"));
    object.insert(
        "dangerously_skip_permissions".to_owned(),
        Value::Bool(false),
    );
    object.insert("plan".to_owned(), Value::Bool(false));
    object.insert("claude_code_router".to_owned(), Value::Bool(false));
    object.remove("claude_code_router_profile");
    object.remove("sandbox");
    object.remove("sandbox_mode");

    let runtime_env = json!({"FORGE_COPILOT_MCP_TOKEN": token});
    let path = context_dir.join(".mcp.json");
    tokio::fs::create_dir_all(context_dir)
        .await
        .map_err(|_| ServiceError::invalid_operation("Cannot prepare Copilot MCP configuration"))?;
    let settings = json!({"mcpServers":{"forge_project":{
        "type":"http", "url":mcp_url,
        "headers":{"Authorization":"Bearer ${FORGE_COPILOT_MCP_TOKEN}"}
    }}});
    let settings = serde_json::to_vec(&settings)
        .map_err(|_| ServiceError::invalid_operation("Cannot encode Copilot MCP configuration"))?;
    tokio::fs::write(&path, settings)
        .await
        .map_err(|_| ServiceError::invalid_operation("Cannot write Copilot MCP configuration"))?;
    Ok(CopilotMcpConfig {
        config,
        runtime_env,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn copilot_native_mcp_configuration_keeps_credentials_out_of_arguments_and_files() {
        let dir = tempfile::tempdir().unwrap();
        let prepared = configure_copilot_mcp(
            ExecutorKind::ClaudeCode,
            json!({
                "model":"selected-model",
                "effort":"high",
                "permission_policy":"auto",
                "dangerously_skip_permissions":true,
                "plan":true,
                "sandbox":"danger-full-access"
            }),
            dir.path(),
            "http://127.0.0.1:8080/mcp",
            "never-persist-this-credential",
        )
        .await
        .unwrap();
        assert_eq!(prepared.config["model"], "selected-model");
        assert_eq!(prepared.config["effort"], "high");
        assert_eq!(prepared.config["permission_policy"], "supervised");
        assert_eq!(prepared.config["dangerously_skip_permissions"], false);
        assert_eq!(prepared.config["plan"], false);
        assert!(prepared.config.get("sandbox").is_none());
        assert!(!prepared
            .config
            .to_string()
            .contains("never-persist-this-credential"));
        assert_eq!(
            prepared.runtime_env["FORGE_COPILOT_MCP_TOKEN"],
            "never-persist-this-credential"
        );
        assert!(!std::fs::read_to_string(dir.path().join(".mcp.json"))
            .unwrap()
            .contains("never-persist-this-credential"));
        for unsupported in [
            ExecutorKind::Codex,
            ExecutorKind::Opencode,
            ExecutorKind::Shell,
        ] {
            assert!(configure_copilot_mcp(
                unsupported,
                json!({}),
                dir.path(),
                "http://127.0.0.1/mcp",
                "secret"
            )
            .await
            .is_err());
        }
        assert!(configure_copilot_mcp(
            ExecutorKind::Codex,
            json!({}),
            dir.path(),
            "http://127.0.0.1/mcp?token=secret",
            "secret"
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn copilot_rejects_agent_command_and_environment_overrides() {
        let dir = tempfile::tempdir().unwrap();
        for config in [
            json!({"base_command_override":"claude-wrapper"}),
            json!({"additional_params":["--tools","default"]}),
            json!({"env":{"PATH":"/tmp/agent-bin"}}),
            json!({"claude_code_router":true}),
            json!({"fallbacks":[{"executor_type":"codex","config":{}}]}),
        ] {
            assert!(configure_copilot_mcp(
                ExecutorKind::ClaudeCode,
                config,
                dir.path(),
                "http://127.0.0.1/mcp",
                "secret"
            )
            .await
            .is_err());
        }
    }
}
