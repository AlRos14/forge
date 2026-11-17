use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
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
    #[serde(default)]
    pub estimate_min_minutes: Option<i64>,
    #[serde(default)]
    pub estimate_max_minutes: Option<i64>,
    #[serde(default)]
    pub source_ids: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectPlanEdge {
    pub predecessor_key: String,
    pub successor_key: String,
    #[serde(default)]
    pub inferred: bool,
    #[serde(default)]
    pub source_ids: Vec<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectPlanDocument {
    pub nodes: Vec<ProjectPlanNode>,
    #[serde(default)]
    pub edges: Vec<ProjectPlanEdge>,
    #[serde(default)]
    pub sources: Vec<ProjectPlanSource>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectPlanProposal {
    pub id: String,
    pub project_id: String,
    pub base_version: i64,
    pub document: ProjectPlanDocument,
    pub digest: String,
    pub state: String,
    pub applied_version: Option<i64>,
    pub created_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectPlanBatch {
    pub id: String,
    pub project_id: String,
    pub graph_version: i64,
    pub version: i64,
    pub state: String,
    pub task_ids: Vec<String>,
    pub max_parallel_tasks: i64,
    pub authorized_by: String,
    pub created_at: String,
    pub updated_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectPlanNodeView {
    pub key: String,
    pub task_id: String,
    pub title: String,
    pub lifecycle_state: String,
    pub dispatch_state: String,
    pub readiness: String,
    pub blockers: Vec<String>,
    pub unsatisfied_task_ids: Vec<String>,
    pub parallel_level: usize,
    pub running_executions: i64,
    pub estimate_min_minutes: Option<i64>,
    pub estimate_max_minutes: Option<i64>,
    pub phase: Option<String>,
    pub source_ids: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectPlanForecast {
    pub critical_path_task_ids: Vec<String>,
    pub min_minutes: Option<i64>,
    pub max_minutes: Option<i64>,
    pub incomplete_estimates: bool,
    pub basis: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectPlanGraph {
    pub project_id: String,
    pub version: i64,
    pub nodes: Vec<ProjectPlanNodeView>,
    pub edges: Vec<ProjectPlanEdge>,
    pub sources: Vec<ProjectPlanSource>,
    pub proposals: Vec<ProjectPlanProposal>,
    pub batches: Vec<ProjectPlanBatch>,
    pub forecast: ProjectPlanForecast,
}

#[derive(Debug, Clone)]
pub(super) struct BatchProjection {
    pub(super) id: String,
    pub(super) project_id: String,
    pub(super) graph_version: i64,
    pub(super) version: i64,
    pub(super) state: String,
    pub(super) preparation_complete: bool,
    pub(super) max_parallel_tasks: i64,
    pub(super) authorized_by: String,
    pub(super) created_at: String,
    pub(super) updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct BatchCoordinatorBaseline {
    /// Batch version captured when the human authorized this exact selection.
    /// Later lifecycle re-preparation may mint an exact grant at the current
    /// version, but retries of the initial grant must keep this original pin.
    pub(super) batch_version: i64,
    pub(super) task_version: i64,
    pub(super) task_objective_digest: String,
    pub(super) lifecycle_state: String,
    pub(super) has_open_work_units: bool,
    pub(super) has_non_cancelled_work_units: bool,
    pub(super) has_approved_or_materialized_plan: bool,
    pub(super) authorization_version: Option<i64>,
    pub(super) team_generation: Option<i64>,
    pub(super) team_digest: Option<String>,
    pub(super) envelope_json: Option<String>,
    pub(super) envelope_digest: Option<String>,
    pub(super) mode: Option<String>,
    pub(super) role_id: Option<String>,
    pub(super) actor_kind: Option<String>,
    pub(super) actor_id: Option<String>,
    pub(super) requested_by: Option<String>,
    pub(super) revoked_at: Option<String>,
    pub(super) team_stale: Option<bool>,
    /// Captured using the same semantic default-envelope predicate used by
    /// Project batch preparation. The persisted bit lets SQLite's grant guard
    /// distinguish a copied TaskCoordination placeholder from Human-configured
    /// manual coordination without relying on an authorization-version value.
    #[serde(default)]
    pub(super) unconfigured_manual_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct BatchCoordinatorSnapshot {
    pub(super) task_version: i64,
    pub(super) task_title: String,
    pub(super) task_description: Option<String>,
    pub(super) task_repo_id: Option<String>,
    pub(super) task_parent_task_id: Option<String>,
    pub(super) authorization_version: i64,
    pub(super) team_generation: i64,
    pub(super) team_digest: String,
    pub(super) envelope_digest: String,
    pub(super) role_id: String,
    pub(super) actor_kind: String,
    pub(super) actor_id: String,
    pub(super) requested_by: String,
}
