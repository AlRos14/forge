use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ProjectPlanSource {
    pub id: String,
    pub label: String,
    pub kind: String,
    #[serde(default)]
    pub reference: Option<String>,
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default)]
    pub excerpt: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ProjectPlanNode {
    pub key: String,
    #[serde(default)]
    pub task_id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub phase: Option<String>,
    #[ts(type = "number | null")]
    #[serde(default)]
    pub estimate_min_minutes: Option<i64>,
    #[ts(type = "number | null")]
    #[serde(default)]
    pub estimate_max_minutes: Option<i64>,
    #[serde(default)]
    pub source_ids: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ProjectPlanEdge {
    pub predecessor_key: String,
    pub successor_key: String,
    #[serde(default)]
    pub inferred: bool,
    #[serde(default)]
    pub source_ids: Vec<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ProjectPlanDocument {
    pub nodes: Vec<ProjectPlanNode>,
    #[serde(default)]
    pub edges: Vec<ProjectPlanEdge>,
    #[serde(default)]
    pub sources: Vec<ProjectPlanSource>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectPlanProposalResponse {
    pub id: String,
    pub project_id: String,
    #[ts(type = "number")]
    pub base_version: i64,
    pub document: ProjectPlanDocument,
    pub digest: String,
    pub state: String,
    #[ts(type = "number | null")]
    pub applied_version: Option<i64>,
    pub created_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectPlanBatchResponse {
    pub id: String,
    pub project_id: String,
    #[ts(type = "number")]
    pub graph_version: i64,
    #[ts(type = "number")]
    pub version: i64,
    pub state: String,
    pub task_ids: Vec<String>,
    #[ts(type = "number")]
    pub max_parallel_tasks: i64,
    pub authorized_by: String,
    pub created_at: String,
    pub updated_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectPlanNodeResponse {
    pub key: String,
    pub task_id: String,
    pub title: String,
    pub lifecycle_state: String,
    pub dispatch_state: String,
    pub readiness: String,
    pub blockers: Vec<String>,
    pub unsatisfied_task_ids: Vec<String>,
    #[ts(type = "number")]
    pub parallel_level: usize,
    #[ts(type = "number")]
    pub running_executions: i64,
    #[ts(type = "number | null")]
    pub estimate_min_minutes: Option<i64>,
    #[ts(type = "number | null")]
    pub estimate_max_minutes: Option<i64>,
    pub phase: Option<String>,
    pub source_ids: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectPlanForecastResponse {
    pub critical_path_task_ids: Vec<String>,
    #[ts(type = "number | null")]
    pub min_minutes: Option<i64>,
    #[ts(type = "number | null")]
    pub max_minutes: Option<i64>,
    pub incomplete_estimates: bool,
    pub basis: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectPlanGraphResponse {
    pub project_id: String,
    #[ts(type = "number")]
    pub version: i64,
    pub nodes: Vec<ProjectPlanNodeResponse>,
    pub edges: Vec<ProjectPlanEdge>,
    pub sources: Vec<ProjectPlanSource>,
    pub proposals: Vec<ProjectPlanProposalResponse>,
    pub batches: Vec<ProjectPlanBatchResponse>,
    pub forecast: ProjectPlanForecastResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ProposeProjectPlanRequest {
    #[ts(type = "number")]
    pub expected_version: i64,
    pub document: ProjectPlanDocument,
    pub idempotency_key: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ApplyProjectPlanRequest {
    #[ts(type = "number")]
    pub expected_version: i64,
    pub idempotency_key: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct AuthorizeProjectPlanBatchRequest {
    #[ts(type = "number")]
    pub expected_version: i64,
    pub task_ids: Vec<String>,
    #[ts(type = "number")]
    pub max_parallel_tasks: i64,
    pub idempotency_key: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProjectPlanBatchAction {
    Pause,
    Resume,
    Revoke,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct UpdateProjectPlanBatchRequest {
    #[ts(type = "number")]
    pub expected_version: i64,
    pub action: ProjectPlanBatchAction,
    pub idempotency_key: String,
}
