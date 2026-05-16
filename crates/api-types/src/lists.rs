use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{InterruptionMetadata, TaskResponse, TaskStatus, TaskType};

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Task {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    pub task_type: TaskType,
    pub priority: i32,
    pub board_position: f64,
    pub blocked: Option<InterruptionMetadata>,
    pub failed: Option<InterruptionMetadata>,
    pub agent_id: Option<String>,
    pub agent_name: Option<String>,
    pub external_issue_number: Option<i64>,
    pub external_issue_url: Option<String>,
    pub version: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TasksResponse {
    pub items: Vec<Task>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub status: String,
    pub executor_type: String,
    pub daemon_id: Option<String>,
    pub active_task_count: i32,
    pub max_concurrent_tasks: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentsResponse {
    pub items: Vec<Agent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Project {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectsResponse {
    pub items: Vec<Project>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PositionRequest {
    pub before_id: Option<String>,
    pub after_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PositionResponse {
    pub task: TaskResponse,
}
