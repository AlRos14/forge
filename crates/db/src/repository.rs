use crate::{models::*, pagination::*, DbError, Result};
use async_trait::async_trait;
use sqlx::{Sqlite, Transaction};

#[async_trait]
pub trait TaskRepo: Send + Sync {
    async fn create(&self, input: CreateTask) -> Result<Task>;
    async fn create_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        input: CreateTask,
    ) -> Result<Task>;
    async fn get_by_id(&self, id: &str, include_deleted: bool) -> Result<Option<Task>>;
    async fn list(&self, query: TaskListQuery) -> Result<Page<Task>>;
    async fn list_by_executing_agent(&self, query: AgentTaskListQuery) -> Result<Page<Task>>;
    async fn list_subtasks_ordered(&self, parent_task_id: &str) -> Result<Vec<Task>>;
    async fn next_subtask_order(&self, parent_task_id: &str) -> Result<i64>;
    async fn reorder_subtasks(
        &self,
        parent_task_id: &str,
        ordered_ids: &[String],
        updated_at: &str,
    ) -> Result<()>;
    async fn update(&self, input: UpdateTask) -> Result<Task>;
    async fn archive(&self, input: ArchiveTask) -> Result<Task>;
    async fn soft_delete(&self, input: SoftDeleteTask) -> Result<Task>;
    async fn set_review_passed_at(
        &self,
        id: &str,
        review_passed_at: Option<String>,
        updated_at: &str,
    ) -> Result<Task>;
    async fn set_metadata_json(
        &self,
        id: &str,
        metadata_json: Option<String>,
        updated_at: &str,
    ) -> Result<()>;
    async fn set_entry_barrier(
        &self,
        id: &str,
        expected_version: i64,
        entry_barrier_json: Option<String>,
        updated_at: &str,
    ) -> Result<Task>;
    async fn claim(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        input: ClaimTask,
    ) -> Result<ClaimedTask>;
    async fn update_status(&self, input: UpdateTaskStatus) -> Result<Task>;
}

#[async_trait]
pub trait TaskBoardRepo: Send + Sync {
    async fn board_revision(&self, project_id: &str) -> Result<i64>;
    async fn replay_move_task(
        &self,
        operation_id: &str,
        identity: &MoveTaskIdentity,
    ) -> Result<Option<MoveTaskResult>>;
    async fn compare_and_move_task(&self, input: CompareAndMoveTask)
        -> Result<MoveTaskPersistence>;
    async fn complete_move_operation(
        &self,
        operation_id: &str,
        result: &MoveTaskResult,
        updated_at: &str,
    ) -> Result<()>;
}

#[async_trait]
pub trait AgentRepo: Send + Sync {
    async fn create(&self, input: CreateAgent) -> Result<Agent>;
    async fn get_by_id(&self, id: &str) -> Result<Option<Agent>>;
    async fn list(&self, query: AgentListQuery) -> Result<Page<Agent>>;
    async fn update(&self, input: UpdateAgent) -> Result<Agent>;
    async fn set_paused(&self, id: &str, paused: bool) -> Result<()>;
    async fn duplicate_agent(
        &self,
        source_id: &str,
        new_id: String,
        new_name: String,
        now: String,
    ) -> Result<Agent>;
    async fn delete(&self, id: &str) -> Result<()>;
    async fn count_active_tasks(&self, agent_id: &str) -> Result<i64>;
}

#[async_trait]
pub trait WorkspaceRepo: Send + Sync {
    async fn create(&self, input: CreateWorkspace) -> Result<Workspace>;
    async fn get_by_id(&self, id: &str) -> Result<Option<Workspace>>;
    async fn get_by_task_id(&self, task_id: &str) -> Result<Option<Workspace>>;
    async fn set_cleanup_after(
        &self,
        id: &str,
        cleanup_after: Option<String>,
        updated_at: &str,
    ) -> Result<Workspace>;
    async fn mark_cleaned(&self, id: &str, updated_at: &str) -> Result<Workspace>;
    async fn list_pending_cleanup(&self, now: &str) -> Result<Vec<Workspace>>;
    async fn update_status(
        &self,
        id: &str,
        status: WorkspaceStatus,
        error: Option<String>,
        updated_at: &str,
    ) -> Result<Workspace>;
    async fn delete(&self, id: &str) -> Result<()>;
}

#[async_trait]
pub trait DaemonRepo: Send + Sync {
    async fn upsert_by_machine_id(&self, input: UpsertDaemon) -> Result<Daemon>;
    async fn get_by_id(&self, id: &str) -> Result<Option<Daemon>>;
    async fn get_by_machine_id(&self, machine_id: &str) -> Result<Option<Daemon>>;
    async fn list(&self, page: PageRequest) -> Result<Page<Daemon>>;
    async fn list_visible(&self, user_id: Option<&str>, page: PageRequest) -> Result<Page<Daemon>>;
    async fn get_visible(&self, id: &str, user_id: Option<&str>) -> Result<Option<Daemon>>;
    async fn update_report(&self, input: UpdateDaemonReport) -> Result<Daemon>;
    async fn mark_online(&self, id: &str, last_report_at: &str) -> Result<Daemon>;
    async fn mark_offline(&self, id: &str, updated_at: &str) -> Result<Daemon>;
    async fn list_available_for_executor(&self, executor_type: &str) -> Result<Vec<Daemon>>;
}

#[async_trait]
pub trait RuntimeRepo: Send + Sync {
    async fn create(&self, input: CreateRuntime) -> Result<Runtime>;
    async fn get_by_id(&self, id: &str) -> Result<Option<Runtime>>;
    async fn get_by_daemon_id(&self, daemon_id: &str) -> Result<Option<Runtime>>;
    async fn upsert_by_daemon_kind(&self, input: CreateRuntime) -> Result<Runtime>;
    async fn list(&self, query: RuntimeListQuery) -> Result<Page<Runtime>>;
}

#[async_trait]
pub trait ExecutionRepo: Send + Sync {
    async fn create(&self, input: CreateExecution) -> Result<Execution>;
    async fn get_by_id(&self, id: &str) -> Result<Option<Execution>>;
    async fn stats_by_agent(&self, agent_id: &str) -> Result<AgentExecutionStats>;
    async fn list_by_task(&self, task_id: &str, page: PageRequest) -> Result<Page<Execution>>;
    async fn list_latest_executions_for_tasks(&self, task_ids: &[&str]) -> Result<Vec<Execution>>;
    async fn list_by_task_and_role(
        &self,
        task_id: &str,
        role: &str,
        page: PageRequest,
    ) -> Result<Page<Execution>>;
    async fn count_by_task_and_role(&self, task_id: &str, role: &str) -> Result<i64>;
    async fn update(&self, input: UpdateExecution) -> Result<Execution>;
    async fn update_last_activity_at(&self, id: &str, timestamp: &str) -> Result<()>;
    async fn list_stalled_running(&self, stale_before: &str) -> Result<Vec<Execution>>;
    async fn list_running(&self) -> Result<Vec<Execution>>;
    async fn list_running_for_daemon_not_in(
        &self,
        daemon_id: &str,
        created_before: &str,
        exclude_ids: &[String],
    ) -> Result<Vec<Execution>>;
    async fn get_logs_path(&self, id: &str) -> Result<Option<String>>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentExecutionStats {
    pub total_runs: i64,
    pub avg_duration_ms: Option<i64>,
    pub success_rate: Option<f64>,
}

#[async_trait]
pub trait ConversationRepo: Send + Sync {
    async fn create(&self, input: CreateConversation) -> Result<Conversation>;
    async fn get_by_id(&self, id: &str) -> Result<Option<Conversation>>;
    async fn list_by_project(&self, query: ConversationListQuery) -> Result<Page<Conversation>>;
    async fn update(&self, input: UpdateConversation) -> Result<Conversation>;
}

#[async_trait]
pub trait ConversationMessageRepo: Send + Sync {
    async fn create(&self, input: CreateConversationMessage) -> Result<ConversationMessage>;
    async fn get_by_id(&self, id: &str) -> Result<Option<ConversationMessage>>;
    async fn list_by_conversation(
        &self,
        query: ConversationMessageListQuery,
    ) -> Result<Page<ConversationMessage>>;
    async fn update(&self, input: UpdateConversationMessage) -> Result<ConversationMessage>;
    async fn next_sequence(&self, conversation_id: &str) -> Result<i64>;
    async fn get_active_streaming_message(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ConversationMessage>>;
}

#[async_trait]
pub trait MemoryRepository: Send + Sync {
    async fn insert_memory_item(&self, item: &MemoryItem) -> std::result::Result<(), DbError>;
    async fn get_memory_item(&self, id: &str) -> std::result::Result<Option<MemoryItem>, DbError>;
    async fn memory_source_exists(
        &self,
        project_id: &str,
        source_type: &str,
        source_ref: &str,
    ) -> std::result::Result<bool, DbError>;
    async fn memory_source_exists_with_confidence(
        &self,
        project_id: &str,
        source_type: &str,
        source_ref: &str,
        confidence: &str,
    ) -> std::result::Result<bool, DbError>;
    async fn search_memory_items(
        &self,
        project_id: &str,
        query: &str,
        limit: usize,
        cursor: Option<String>,
    ) -> std::result::Result<(Vec<MemoryItem>, bool), DbError>;
    async fn list_memory_items_by_source(
        &self,
        project_id: &str,
        source_type: &str,
        source_id: &str,
    ) -> std::result::Result<Vec<MemoryItem>, DbError>;
}

#[async_trait]
pub trait ReviewRepo: Send + Sync {
    async fn create(&self, input: CreateReview) -> Result<Review>;
    async fn update_status(
        &self,
        id: &str,
        status: ReviewStatus,
        step_results_json: String,
        finished_at: Option<String>,
        updated_at: &str,
    ) -> Result<Review>;
    async fn get_by_id(&self, id: &str) -> Result<Option<Review>>;
    async fn list_by_task(&self, task_id: &str) -> Result<Vec<Review>>;
    async fn list_latest_reviews_for_tasks(&self, task_ids: &[&str]) -> Result<Vec<Review>>;
    async fn next_attempt_number(&self, task_id: &str) -> Result<i64>;
}

#[async_trait]
pub trait TaskCommentRepo: Send + Sync {
    async fn create_comment(&self, input: CreateTaskComment) -> Result<TaskComment>;
    async fn list_comments(&self, task_id: &str, page: PageRequest) -> Result<Page<TaskComment>>;
    async fn get_comment_by_id(&self, id: &str) -> Result<Option<TaskComment>>;
    async fn delete_comment(&self, id: &str) -> Result<()>;
}

#[async_trait]
pub trait TaskMediaRepo: Send + Sync {
    async fn create_media(&self, input: CreateTaskMedia) -> Result<TaskMedia>;
    async fn list_media(&self, task_id: &str, page: PageRequest) -> Result<Page<TaskMedia>>;
    async fn list_active_media_for_task(&self, task_id: &str) -> Result<Vec<TaskMedia>>;
    async fn get_media_by_id(&self, id: &str, include_deleted: bool) -> Result<Option<TaskMedia>>;
    async fn soft_delete_media(&self, id: &str, deleted_at: &str) -> Result<TaskMedia>;
}

#[async_trait]
pub trait TerminalSessionRepo: Send + Sync {
    async fn create_terminal_session(
        &self,
        input: CreateTerminalSession,
    ) -> Result<TerminalSession>;
    async fn get_terminal_session(&self, id: &str) -> Result<Option<TerminalSession>>;
    async fn list_terminal_sessions_for_task(
        &self,
        task_id: &str,
        include_ended: bool,
    ) -> Result<Vec<TerminalSession>>;
    async fn list_running_terminal_sessions_for_task(
        &self,
        task_id: &str,
    ) -> Result<Vec<TerminalSession>>;
    async fn list_running_terminal_sessions_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<TerminalSession>>;
    async fn list_running_terminal_sessions_for_workspace(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<TerminalSession>>;
    async fn list_all_running_terminal_sessions(&self) -> Result<Vec<TerminalSession>>;
    async fn update_terminal_session_status(
        &self,
        id: &str,
        expected_version: i64,
        update: UpdateTerminalSessionStatus,
    ) -> Result<TerminalSession>;
    async fn update_terminal_session_size(
        &self,
        id: &str,
        rows: i64,
        cols: i64,
        last_activity_at: &str,
    ) -> Result<TerminalSession>;
    async fn touch_terminal_session_activity(&self, id: &str, last_activity_at: &str)
        -> Result<()>;
    async fn delete_terminal_sessions_for_workspace(&self, workspace_id: &str) -> Result<u64>;
}

#[async_trait]
pub trait ProjectRepo: Send + Sync {
    async fn create(&self, input: CreateProject) -> Result<Project>;
    async fn get_by_id(&self, id: &str) -> Result<Option<Project>>;
    async fn list(&self, page: PageRequest) -> Result<Page<Project>>;
    async fn update(&self, input: UpdateProject) -> Result<Project>;
    async fn set_project_hooks_json(
        &self,
        id: &str,
        project_hooks_json: &str,
        updated_at: &str,
    ) -> Result<()>;
    async fn increment_project_work_epoch(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        project_id: &str,
        by: i64,
    ) -> Result<i64>;
    async fn set_paused_at(&self, id: &str, paused_at: Option<String>) -> Result<()>;
    async fn delete(&self, id: &str) -> Result<()>;
}

#[async_trait]
pub trait ProjectHookRunRepo: Send + Sync {
    async fn try_claim(&self, input: CreateProjectHookRun) -> Result<Option<ProjectHookRun>>;
    async fn try_claim_or_skip_at_limit(
        &self,
        input: CreateProjectHookRun,
        max_active_runs: i64,
        skip_reason: &str,
    ) -> Result<Option<ProjectHookRun>>;
    async fn update_status(&self, input: UpdateProjectHookRun) -> Result<ProjectHookRun>;
    async fn list_recent_for_project(
        &self,
        project_id: &str,
        limit: i64,
    ) -> Result<Vec<ProjectHookRun>>;
    async fn list_for_project(
        &self,
        project_id: &str,
        page: PageRequest,
    ) -> Result<Page<ProjectHookRun>>;
    async fn count_active_for_rule(&self, project_id: &str, rule_id: &str) -> Result<i64>;
}

#[async_trait]
pub trait RepoRepo: Send + Sync {
    async fn create(&self, input: CreateRepo) -> Result<Repo>;
    async fn get_by_id(&self, id: &str) -> Result<Option<Repo>>;
    async fn list_by_project(&self, project_id: &str, page: PageRequest) -> Result<Page<Repo>>;
    async fn update(&self, input: UpdateRepo) -> Result<Repo>;
    async fn delete(&self, id: &str) -> Result<()>;
}

#[async_trait]
pub trait PrProviderConfigRepo: Send + Sync {
    async fn create(&self, input: CreatePrProviderConfig) -> Result<PrProviderConfig>;
    async fn get_by_repo_id(&self, repo_id: &str) -> Result<Option<PrProviderConfig>>;
    async fn update(&self, input: UpdatePrProviderConfig) -> Result<PrProviderConfig>;
    async fn delete(&self, id: &str) -> Result<()>;
}

#[async_trait]
pub trait PrMetadataRepo: Send + Sync {
    async fn create(&self, input: CreatePrMetadata) -> Result<PrMetadata>;
    async fn get_by_task_id(&self, task_id: &str) -> Result<Option<PrMetadata>>;
    async fn update(&self, input: UpdatePrMetadata) -> Result<PrMetadata>;
    async fn delete(&self, id: &str) -> Result<()>;
}

#[async_trait]
pub trait IntegrationRepo: Send + Sync {
    async fn create_integration(
        &self,
        input: CreateProjectIntegration,
    ) -> Result<ProjectIntegration>;
    async fn get_by_id(&self, id: &str) -> Result<Option<ProjectIntegration>>;
    async fn get_by_project_id(&self, project_id: &str) -> Result<Option<ProjectIntegration>>;
    async fn update_integration(
        &self,
        input: UpdateProjectIntegration,
    ) -> Result<ProjectIntegration>;
    async fn update_last_polled_at(
        &self,
        id: &str,
        last_polled_at: &str,
        updated_at: &str,
    ) -> Result<()>;
    async fn list_enabled(&self) -> Result<Vec<ProjectIntegration>>;
    async fn delete_integration(&self, id: &str) -> Result<()>;
}

#[async_trait]
pub trait ExternalLinkRepo: Send + Sync {
    async fn create_link(&self, input: CreateTaskExternalLink) -> Result<TaskExternalLink>;
    async fn get_by_id(&self, id: &str) -> Result<Option<TaskExternalLink>>;
    async fn get_by_global_id(&self, global_id: &str) -> Result<Option<TaskExternalLink>>;
    async fn get_by_task_id(&self, task_id: &str) -> Result<Option<TaskExternalLink>>;
    async fn list_by_task_id(&self, task_id: &str) -> Result<Vec<TaskExternalLink>>;
    async fn list_by_integration(&self, integration_id: &str) -> Result<Vec<TaskExternalLink>>;
    async fn delete_link(&self, id: &str) -> Result<()>;
}

#[async_trait]
pub trait SkillRepo: Send + Sync {
    async fn create(&self, input: CreateSkill) -> Result<Skill>;
    async fn get_by_id(&self, id: &str) -> Result<Option<Skill>>;
    async fn list_by_project(&self, project_id: &str, page: PageRequest) -> Result<Page<Skill>>;
    async fn update(&self, input: UpdateSkill) -> Result<Skill>;
    async fn delete(&self, id: &str) -> Result<()>;
}

#[async_trait]
pub trait NotificationRepo: Send + Sync {
    async fn create(&self, input: CreateNotification) -> Result<Notification>;
    async fn list(&self, query: NotificationListQuery) -> Result<Page<Notification>>;
    async fn unread_count(&self, project_id: Option<&str>) -> Result<i64>;
    async fn mark_read(&self, id: &str) -> Result<Notification>;
    async fn mark_all_read(&self, project_id: Option<&str>) -> Result<u64>;
    async fn delete(&self, id: &str) -> Result<()>;
}

#[async_trait]
pub trait TaskDependencyRepo: Send + Sync {
    async fn add_dependency(&self, task_id: &str, depends_on_id: &str, now: &str) -> Result<()>;
    async fn remove_dependency(&self, task_id: &str, depends_on_id: &str) -> Result<()>;
    async fn list_dependencies(&self, task_id: &str) -> Result<Vec<String>>;
    async fn list_dependents(&self, depends_on_id: &str) -> Result<Vec<String>>;
    async fn unsatisfied_dependencies(&self, task_id: &str) -> Result<Vec<String>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskListQuery {
    pub project_id: String,
    pub q: Option<String>,
    pub statuses: Vec<String>,
    pub agent_ids: Vec<String>,
    pub assignee_types: Vec<String>,
    pub assignee_ids: Vec<String>,
    pub priority: Option<i64>,
    pub include_archived: bool,
    pub include_cancelled: bool,
    pub include_deleted: bool,
    pub page: PageRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTaskListQuery {
    pub agent_id: String,
    pub include_archived: bool,
    pub include_cancelled: bool,
    pub include_deleted: bool,
    pub page: PageRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentListQuery {
    pub status: Option<AgentStatus>,
    pub executor_type: Option<String>,
    pub capabilities: Vec<String>,
    pub page: PageRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationListQuery {
    pub project_id: Option<String>,
    pub read: Option<bool>,
    pub page: PageRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProject {
    pub id: String,
    pub name: String,
    pub settings: String,
    pub workflow_definition: String,
    pub primary_repo_id: Option<String>,
    pub owner_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateProject {
    pub id: String,
    pub name: Option<String>,
    pub settings: Option<String>,
    pub primary_repo_id: Option<Option<String>>,
    pub paused_at: Option<Option<String>>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateRepo {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub remote_url: String,
    pub local_path: Option<String>,
    pub work_mode: WorkMode,
    pub default_branch: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateRepo {
    pub id: String,
    pub name: Option<String>,
    pub local_path: Option<Option<String>>,
    pub remote_url: Option<String>,
    pub work_mode: Option<WorkMode>,
    pub default_branch: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatePrProviderConfig {
    pub id: String,
    pub repo_id: String,
    pub provider_type: String,
    pub base_url: Option<String>,
    pub polling_interval_seconds: i64,
    pub token_secret_ref: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatePrProviderConfig {
    pub id: String,
    pub provider_type: Option<String>,
    pub base_url: Option<Option<String>>,
    pub polling_interval_seconds: Option<i64>,
    pub token_secret_ref: Option<Option<String>>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatePrMetadata {
    pub id: String,
    pub task_id: String,
    pub provider_type: String,
    pub provider_pr_id: Option<String>,
    pub pr_url: Option<String>,
    pub source_branch: String,
    pub target_branch: String,
    pub pr_state: String,
    pub merge_status: String,
    pub last_synced_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatePrMetadata {
    pub id: String,
    pub provider_type: Option<String>,
    pub provider_pr_id: Option<Option<String>>,
    pub pr_url: Option<Option<String>>,
    pub source_branch: Option<String>,
    pub target_branch: Option<String>,
    pub pr_state: Option<String>,
    pub merge_status: Option<String>,
    pub last_synced_at: Option<Option<String>>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateAgent {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub executor_type: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub permission_policy: Option<String>,
    pub prompt_template: Option<String>,
    pub capabilities_json: String,
    pub config_json: String,
    pub daemon_id: Option<String>,
    pub max_concurrent_tasks: i64,
    pub heartbeat_interval_seconds: i64,
    pub max_missed_heartbeats: i64,
    pub status: AgentStatus,
    pub last_heartbeat_at: Option<String>,
    pub is_default: bool,
    pub paused: bool,
    pub owner_id: Option<String>,
    pub visibility: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateAgent {
    pub id: String,
    pub expected_version: i64,
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub model: Option<Option<String>>,
    pub reasoning_effort: Option<Option<String>>,
    pub permission_policy: Option<Option<String>>,
    pub prompt_template: Option<Option<String>>,
    pub capabilities_json: Option<String>,
    pub config_json: Option<String>,
    pub daemon_id: Option<Option<String>>,
    pub max_concurrent_tasks: Option<i64>,
    pub heartbeat_interval_seconds: Option<i64>,
    pub max_missed_heartbeats: Option<i64>,
    pub status: Option<AgentStatus>,
    pub last_heartbeat_at: Option<Option<String>>,
    pub is_default: Option<bool>,
    pub paused: Option<bool>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateWorkspace {
    pub id: String,
    pub task_id: String,
    pub repo_id: String,
    pub worktree_path: String,
    pub branch: String,
    pub status: WorkspaceStatus,
    pub before_sha: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpsertDaemon {
    pub id: String,
    pub machine_id: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub agent_version: Option<String>,
    pub labels_json: String,
    pub status: DaemonStatus,
    pub registration_token_hash: Option<String>,
    pub owner_id: Option<String>,
    pub visibility: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateDaemonReport {
    pub id: String,
    pub last_report_at: String,
    pub status: DaemonStatus,
    pub detected_clis_json: String,
    pub labels_json: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateRuntime {
    pub id: String,
    pub daemon_id: String,
    pub kind: String,
    pub workspace_root: String,
    pub status: RuntimeStatus,
    pub labels_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeListQuery {
    pub daemon_id: Option<String>,
    pub page: PageRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateSkill {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateSkill {
    pub id: String,
    pub name: Option<String>,
    pub content: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTask {
    pub id: String,
    pub project_id: String,
    pub repo_id: Option<String>,
    pub parent_task_id: Option<String>,
    pub assignee_type: Option<String>,
    pub assignee_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub task_type: String,
    pub status: String,
    pub is_automation: bool,
    pub priority: i64,
    pub subtask_order: Option<i64>,
    pub task_state_config: Option<String>,
    pub merge_config: Option<String>,
    pub plan: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateTask {
    pub id: String,
    pub expected_version: i64,
    pub title: Option<String>,
    pub description: Option<Option<String>>,
    pub priority: Option<i64>,
    pub merge_config: Option<Option<String>>,
    pub plan: Option<Option<String>>,
    pub error_annotation: Option<Option<String>>,
    pub blocked_json: Option<Option<String>>,
    pub failed_json: Option<Option<String>>,
    pub task_state_config: Option<Option<String>>,
    pub parent_task_id: Option<Option<String>>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoftDeleteTask {
    pub id: String,
    pub expected_version: i64,
    pub deleted_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveTask {
    pub id: String,
    pub expected_version: i64,
    pub archived_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimTask {
    pub task_id: String,
    pub assignee_type: String,
    pub assignee_id: Option<String>,
    pub expected_version: i64,
    pub source_status: String,
    pub target_status: String,
    pub capacity_statuses: Vec<String>,
    pub execution: CreateExecution,
    pub max_concurrent_tasks: i64,
    pub claimed_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaimedTask {
    pub task: Task,
    pub execution: Execution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateTaskStatus {
    pub id: String,
    pub expected_version: i64,
    pub status: String,
    pub assignee_id: Option<Option<String>>,
    pub error_annotation: Option<Option<String>>,
    pub blocked_json: Option<Option<String>>,
    pub failed_json: Option<Option<String>>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateExecution {
    pub id: String,
    pub task_id: String,
    pub agent_id: Option<String>,
    pub role: String,
    pub status: ExecutionStatus,
    pub stop_reason: Option<StopReason>,
    pub stopped_by: Option<String>,
    pub resume_policy: Option<ResumePolicy>,
    pub stopped_at: Option<String>,
    pub parent_execution_id: Option<String>,
    pub agent_session_id: Option<String>,
    pub agent_message_id: Option<String>,
    pub last_activity_at: Option<String>,
    pub summary: Option<String>,
    pub logs_path: Option<String>,
    pub before_sha: Option<String>,
    pub after_sha: Option<String>,
    pub error: Option<String>,
    pub executor_config_snapshot_json: Option<String>,
    pub workspace_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateExecution {
    pub id: String,
    pub status: Option<ExecutionStatus>,
    pub stop_reason: Option<Option<StopReason>>,
    pub stopped_by: Option<Option<String>>,
    pub resume_policy: Option<Option<ResumePolicy>>,
    pub stopped_at: Option<Option<String>>,
    pub agent_session_id: Option<Option<String>>,
    pub agent_message_id: Option<Option<String>>,
    pub last_activity_at: Option<Option<String>>,
    pub summary: Option<Option<String>>,
    pub logs_path: Option<Option<String>>,
    pub before_sha: Option<Option<String>>,
    pub after_sha: Option<Option<String>>,
    pub error: Option<Option<String>>,
    pub executor_config_snapshot_json: Option<Option<String>>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateConversation {
    pub id: String,
    pub project_id: String,
    pub agent_id: Option<String>,
    pub title: String,
    pub status: ConversationStatus,
    pub system_prompt: Option<String>,
    pub message_count: i64,
    pub last_message_at: Option<String>,
    pub agent_session_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationListQuery {
    pub project_id: String,
    pub status: Option<ConversationStatus>,
    pub page: PageRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateConversation {
    pub id: String,
    pub expected_version: i64,
    pub agent_id: Option<Option<String>>,
    pub title: Option<String>,
    pub status: Option<ConversationStatus>,
    pub system_prompt: Option<Option<String>>,
    pub message_count: Option<i64>,
    pub last_message_at: Option<Option<String>>,
    pub agent_session_id: Option<Option<String>>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateConversationMessage {
    pub id: String,
    pub conversation_id: String,
    pub role: ConversationMessageRole,
    pub content: String,
    pub status: ConversationMessageStatus,
    pub model: Option<String>,
    pub token_usage_json: Option<String>,
    pub duration_ms: Option<i64>,
    pub error: Option<String>,
    pub sequence: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateConversationMessage {
    pub id: String,
    pub content: Option<String>,
    pub status: Option<ConversationMessageStatus>,
    pub model: Option<Option<String>>,
    pub token_usage_json: Option<Option<String>>,
    pub duration_ms: Option<Option<i64>>,
    pub error: Option<Option<String>>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationMessageListQuery {
    pub conversation_id: String,
    pub before_sequence: Option<i64>,
    pub page: PageRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateReview {
    pub id: String,
    pub task_id: String,
    pub execution_id: String,
    pub attempt_number: i64,
    pub status: ReviewStatus,
    pub step_results_json: String,
    pub started_at: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTaskComment {
    pub id: String,
    pub task_id: String,
    pub author_type: CommentAuthorType,
    pub author_id: Option<String>,
    pub author_name: String,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
}

#[async_trait]
pub trait TaskRoleAssignmentRepo: Send + Sync {
    async fn assign(
        &self,
        input: CreateTaskRoleAssignment,
    ) -> std::result::Result<TaskRoleAssignment, crate::DbError>;
    async fn get_by_task_and_role(
        &self,
        task_id: &str,
        role_name: &str,
    ) -> std::result::Result<Option<TaskRoleAssignment>, crate::DbError>;
    async fn list_by_task(
        &self,
        task_id: &str,
    ) -> std::result::Result<Vec<TaskRoleAssignment>, crate::DbError>;
    async fn remove(
        &self,
        task_id: &str,
        role_name: &str,
    ) -> std::result::Result<(), crate::DbError>;
}

#[async_trait]
pub trait TransitionLogRepo: Send + Sync {
    async fn insert(
        &self,
        input: CreateTransitionLog,
    ) -> std::result::Result<TransitionLog, crate::DbError>;
    async fn insert_recovery_marker(
        &self,
        task_id: &str,
        current_state: &str,
        action_kind: &str,
        reason: &str,
    ) -> std::result::Result<TransitionLog, crate::DbError>;
    async fn list_by_task(
        &self,
        task_id: &str,
    ) -> std::result::Result<Vec<TransitionLog>, crate::DbError>;
    async fn count_gate_rejections(
        &self,
        task_id: &str,
        gate_state: &str,
    ) -> std::result::Result<i64, crate::DbError>;
    async fn count_to_state_since(
        &self,
        task_id: &str,
        to_state: &str,
        since: Option<&str>,
    ) -> std::result::Result<i64, crate::DbError>;
    async fn update_hook_results(
        &self,
        id: &str,
        hook_results_json: &str,
    ) -> std::result::Result<(), crate::DbError>;
}

#[derive(Debug, Clone)]
pub struct UpsertExecutionUsage {
    pub execution_id: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskUsageSummary {
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_read_tokens: i64,
    pub total_cache_write_tokens: i64,
    pub total_cost_usd: Option<f64>,
    pub execution_count: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CiStepStats {
    pub command: String,
    pub total_runs: i64,
    pub pass_count: i64,
    pub fail_count: i64,
    pub avg_duration_ms: Option<i64>,
    pub p50_duration_ms: Option<i64>,
    pub p95_duration_ms: Option<i64>,
    pub last_run_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelTokenBreakdown {
    pub provider: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd: Option<f64>,
    pub execution_count: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectTokenStats {
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_read_tokens: i64,
    pub total_cache_write_tokens: i64,
    pub total_cost_usd: Option<f64>,
    pub execution_count: i64,
    pub by_model: Vec<ModelTokenBreakdown>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectReviewSummary {
    pub total_reviews: i64,
    pub passed: i64,
    pub failed: i64,
    pub cancelled: i64,
    pub avg_duration_ms: Option<i64>,
    pub pass_rate: f64,
}

#[async_trait]
pub trait ProjectAnalyticsRepo: Send + Sync {
    async fn get_project_ci_analytics(
        &self,
        project_id: &str,
        from: Option<&str>,
        to: Option<&str>,
    ) -> Result<Vec<CiStepStats>>;
    async fn get_project_token_analytics(
        &self,
        project_id: &str,
        from: Option<&str>,
        to: Option<&str>,
    ) -> Result<ProjectTokenStats>;
    async fn get_project_review_summary(
        &self,
        project_id: &str,
        from: Option<&str>,
        to: Option<&str>,
    ) -> Result<ProjectReviewSummary>;
}

#[async_trait]
pub trait ExecutionUsageRepo: Send + Sync {
    async fn upsert(&self, input: UpsertExecutionUsage) -> Result<ExecutionUsage>;
    async fn list_by_execution(&self, execution_id: &str) -> Result<Vec<ExecutionUsage>>;
    async fn get_task_usage_summary(&self, task_id: &str) -> Result<TaskUsageSummary>;
}

#[async_trait]
pub trait UserRepo: Send + Sync {
    async fn create_user(&self, user: &User) -> Result<()>;
    async fn get_user_by_id(&self, id: &str) -> Result<Option<User>>;
    async fn get_user_by_email(&self, email: &str) -> Result<Option<User>>;
    async fn search_users(&self, query: &str, limit: i64) -> Result<Vec<User>>;
    async fn list_users(&self, page: PageRequest) -> Result<Page<User>>;
    async fn set_admin(&self, id: &str, is_admin: bool) -> Result<()>;
    async fn update_profile(
        &self,
        id: &str,
        email: &str,
        display_name: Option<&str>,
        updated_at: &str,
    ) -> Result<()>;
    async fn count_admins(&self) -> Result<i64>;
    async fn delete_user(&self, id: &str) -> Result<bool>;
}

#[async_trait]
pub trait RefreshTokenRepo: Send + Sync {
    async fn create_refresh_token(&self, token: &RefreshToken) -> Result<()>;
    async fn delete_refresh_token_by_hash(&self, token_hash: &str) -> Result<Option<RefreshToken>>;
    async fn delete_refresh_tokens_by_family(&self, family_id: &str) -> Result<u64>;
    async fn delete_expired_refresh_tokens(&self) -> Result<u64>;
    async fn get_refresh_tokens_by_user(&self, user_id: &str) -> Result<Vec<RefreshToken>>;
}

#[async_trait]
pub trait PersonalAccessTokenRepo: Send + Sync {
    async fn create_pat(&self, input: CreatePersonalAccessToken) -> Result<PersonalAccessToken>;
    async fn get_pat_by_token_hash(&self, token_hash: &str) -> Result<Option<PersonalAccessToken>>;
    async fn list_pats_by_user(&self, user_id: &str) -> Result<Vec<PersonalAccessToken>>;
    async fn delete_pat(&self, id: &str, user_id: &str) -> Result<()>;
    async fn update_last_used(&self, id: &str, last_used_at: &str) -> Result<()>;
}

#[async_trait]
pub trait OAuthClientRepo: Send + Sync {
    async fn create_client(&self, input: CreateOAuthClient) -> Result<OAuthClient>;
    async fn get_client(&self, client_id: &str) -> Result<Option<OAuthClient>>;
    async fn touch_last_used(&self, client_id: &str, last_used_at: &str) -> Result<()>;
    async fn count_clients_created_since(&self, created_after_rfc3339: &str) -> Result<i64>;
}

#[async_trait]
pub trait OAuthAuthorizationCodeRepo: Send + Sync {
    async fn create_code(
        &self,
        input: CreateOAuthAuthorizationCode,
    ) -> Result<OAuthAuthorizationCode>;
    /// Returns the code row regardless of consumed_at / expires_at; service enforces semantics.
    async fn get_code_by_hash(&self, code_hash: &str) -> Result<Option<OAuthAuthorizationCode>>;
    /// Atomic single-use claim: UPDATE ... WHERE id = ? AND consumed_at IS NULL. Returns true if this caller won.
    async fn mark_code_consumed(&self, id: &str, consumed_at: &str) -> Result<bool>;
    async fn delete_expired_codes(&self, now_rfc3339: &str) -> Result<u64>;
}

#[async_trait]
pub trait OAuthRefreshTokenRepo: Send + Sync {
    async fn create_refresh_token(
        &self,
        input: CreateOAuthRefreshToken,
    ) -> Result<OAuthRefreshToken>;
    async fn create_refresh_token_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        input: CreateOAuthRefreshToken,
    ) -> Result<OAuthRefreshToken>;
    async fn get_refresh_token_by_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<OAuthRefreshToken>>;
    /// Atomic single-use claim for rotation. Returns true if this caller revoked the active row.
    async fn claim_refresh_token_for_rotation(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        id: &str,
        revoked_at: &str,
    ) -> Result<bool>;
    /// Set revoked_at on a single row by id. Idempotent.
    async fn revoke_refresh_token(&self, id: &str, revoked_at: &str) -> Result<()>;
    /// Set revoked_at on every non-revoked row in the family. Returns count of newly-revoked rows.
    async fn revoke_refresh_token_family(&self, family_id: &str, revoked_at: &str) -> Result<u64>;
    async fn revoke_refresh_token_family_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        family_id: &str,
        revoked_at: &str,
    ) -> Result<u64>;
    async fn delete_expired_refresh_tokens(&self, now_rfc3339: &str) -> Result<u64>;
}

#[async_trait]
pub trait ProjectMemberRepo: Send + Sync {
    async fn add_member(&self, input: CreateProjectMember) -> Result<ProjectMember>;
    async fn get_member(&self, project_id: &str, user_id: &str) -> Result<Option<ProjectMember>>;
    async fn list_members(&self, project_id: &str) -> Result<Vec<ProjectMember>>;
    async fn update_member_role(
        &self,
        project_id: &str,
        user_id: &str,
        role: &str,
        updated_at: &str,
    ) -> Result<ProjectMember>;
    async fn remove_member(&self, project_id: &str, user_id: &str) -> Result<()>;
}

#[async_trait]
pub trait ProjectAgentLinkRepo: Send + Sync {
    async fn create(&self, input: CreateProjectAgentLink) -> Result<ProjectAgentLink>;
    async fn list_by_project(&self, project_id: &str) -> Result<Vec<ProjectAgentLink>>;
    async fn delete_by_project_and_agent(&self, project_id: &str, agent_id: &str) -> Result<()>;
    async fn get_by_project_and_agent(
        &self,
        project_id: &str,
        agent_id: &str,
    ) -> Result<Option<ProjectAgentLink>>;
}

#[async_trait]
pub trait SystemSettingRepo: Send + Sync {
    async fn get_setting(&self, key: &str) -> Result<Option<String>>;
    async fn set_setting(&self, key: &str, value: &str, updated_at: &str) -> Result<()>;
    async fn list_settings(&self) -> Result<Vec<(String, String)>>;
    async fn delete_setting(&self, key: &str) -> Result<()>;
}
