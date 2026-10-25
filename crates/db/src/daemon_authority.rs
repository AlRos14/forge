//! Daemon visibility is discovery metadata, never a cross-account execution grant.
use crate::{Agent, Daemon, DaemonStatus};

/// `owner_is_admin` must come from the current durable User, never a token claim.
/// Reserved embedded identities represent the server host and require admin trust.
pub fn daemon_execution_authorized(
    daemon: &Daemon,
    owner_id: Option<&str>,
    owner_is_admin: bool,
    executor_type: &str,
) -> bool {
    let Some(owner_id) = owner_id.filter(|id| !id.is_empty()) else {
        return false;
    };
    let host_authorized = if daemon.machine_id.starts_with("embedded:") {
        owner_is_admin
            && daemon
                .owner_id
                .as_deref()
                .is_none_or(|host_owner_id| host_owner_id == owner_id)
    } else {
        daemon.owner_id.as_deref() == Some(owner_id)
    };
    host_authorized
        && matches!(daemon.visibility.as_str(), "account" | "global")
        && daemon.status == DaemonStatus::Online
        && serde_json::from_str::<serde_json::Value>(&daemon.detected_clis_json)
            .ok()
            .and_then(|value| value.as_array().cloned())
            .is_some_and(|items| {
                items.iter().any(|item| {
                    item.get("kind").and_then(|v| v.as_str()) == Some(executor_type)
                        && item.get("availability").and_then(|v| v.as_str())
                            == Some("authenticated")
                })
            })
}

/// Freeze authority-relevant identities, not operational report timestamps or secrets.
pub fn daemon_authority_snapshot(agent: &Agent, daemon: &Daemon) -> serde_json::Value {
    serde_json::json!({
        "agent_owner_id": agent.owner_id,
        "profile_id": agent.profile_id,
        "agent_daemon_id": agent.daemon_id,
        "executor_type": agent.executor_type,
        "daemon_id": daemon.id,
        "daemon_owner_id": daemon.owner_id,
        "daemon_visibility": daemon.visibility,
        "machine_id": daemon.machine_id,
    })
}
