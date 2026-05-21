use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::TaskType;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectHookRule {
    pub id: String,
    pub enabled: bool,
    pub name: String,
    pub trigger: ProjectHookTrigger,
    #[ts(type = "unknown")]
    pub filters: Option<Value>,
    pub action: ProjectHookAction,
    pub cooldown_seconds: Option<u64>,
    pub max_concurrent_runs: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type")]
#[ts(export)]
pub enum ProjectHookTrigger {
    #[serde(rename = "project.all_work_completed")]
    AllWorkCompleted,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type")]
#[ts(export)]
pub enum ProjectHookAction {
    #[serde(rename = "dispatch_agent")]
    DispatchAgent {
        agent_id: String,
        prompt: Option<String>,
        #[ts(type = "unknown")]
        follow_up: Option<Value>,
    },
    #[serde(rename = "create_task")]
    CreateTask {
        title: String,
        description: Option<String>,
        task_type: Option<TaskType>,
        priority: Option<i64>,
    },
    #[serde(rename = "add_comment")]
    AddComment {
        target_task_id: Option<String>,
        content: String,
    },
    #[serde(rename = "notify")]
    Notify {
        title: String,
        message: String,
        severity: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProjectHookRunStatus {
    Queued,
    Running,
    Dispatched,
    Skipped,
    Failed,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectHookRunResponse {
    pub id: String,
    pub project_id: String,
    pub rule_id: String,
    pub trigger_type: String,
    pub dedupe_key: String,
    pub status: ProjectHookRunStatus,
    pub source_task_id: Option<String>,
    pub source_execution_id: Option<String>,
    pub automation_task_id: Option<String>,
    pub execution_id: Option<String>,
    pub agent_id: Option<String>,
    pub reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectHookRunsResponse {
    pub items: Vec<ProjectHookRunResponse>,
    pub next_cursor: Option<String>,
}

pub fn parse_project_hooks_json(json: &str) -> Result<Vec<ProjectHookRule>, String> {
    let raw_rules: Vec<Value> = serde_json::from_str(json)
        .map_err(|error| format!("invalid project hooks JSON: {error}"))?;

    for (index, raw_rule) in raw_rules.iter().enumerate() {
        let Some(trigger_type) = raw_rule
            .get("trigger")
            .and_then(|trigger| trigger.get("type"))
            .and_then(Value::as_str)
        else {
            continue;
        };

        match trigger_type {
            "project.all_work_completed" => {}
            "task.stuck" => {
                return Err(format!(
                    "project hook rule at index {index} trigger requires a future persisted stuck signal"
                ));
            }
            _ => {
                return Err(format!(
                    "unsupported project hook trigger type `{trigger_type}` at index {index}"
                ));
            }
        }
    }

    let rules: Vec<ProjectHookRule> = serde_json::from_value(Value::Array(raw_rules))
        .map_err(|error| format!("invalid project hook rule: {error}"))?;
    let mut ids = HashSet::new();
    for rule in &rules {
        if rule.id.trim().is_empty() {
            return Err("project hook rule id must be non-empty".to_string());
        }
        if !ids.insert(rule.id.as_str()) {
            return Err(format!("duplicate project hook rule id `{}`", rule.id));
        }
        if rule.name.trim().is_empty() {
            return Err(format!(
                "project hook rule `{}` name must be non-empty",
                rule.id
            ));
        }
        if !(1..=10).contains(&rule.max_concurrent_runs) {
            return Err(format!(
                "project hook rule `{}` max_concurrent_runs must be between 1 and 10",
                rule.id
            ));
        }
        validate_project_hook_action(rule)?;
    }
    Ok(rules)
}

fn validate_project_hook_action(rule: &ProjectHookRule) -> Result<(), String> {
    match &rule.action {
        ProjectHookAction::DispatchAgent { agent_id, .. } => {
            if agent_id.trim().is_empty() {
                return Err(format!(
                    "project hook rule `{}` dispatch_agent.agent_id must be non-empty",
                    rule.id
                ));
            }
        }
        ProjectHookAction::CreateTask { title, .. } => {
            if title.trim().is_empty() {
                return Err(format!(
                    "project hook rule `{}` create_task.title must be non-empty",
                    rule.id
                ));
            }
        }
        ProjectHookAction::AddComment {
            target_task_id,
            content,
        } => {
            if target_task_id
                .as_ref()
                .is_some_and(|target_task_id| target_task_id.trim().is_empty())
            {
                return Err(format!(
                    "project hook rule `{}` add_comment.target_task_id must be non-empty when provided",
                    rule.id
                ));
            }
            if content.trim().is_empty() {
                return Err(format!(
                    "project hook rule `{}` add_comment.content must be non-empty",
                    rule.id
                ));
            }
        }
        ProjectHookAction::Notify { title, message, .. } => {
            if title.trim().is_empty() {
                return Err(format!(
                    "project hook rule `{}` notify.title must be non-empty",
                    rule.id
                ));
            }
            if message.trim().is_empty() {
                return Err(format!(
                    "project hook rule `{}` notify.message must be non-empty",
                    rule.id
                ));
            }
        }
    }
    Ok(())
}
