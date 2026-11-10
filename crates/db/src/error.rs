use std::{io, path::PathBuf};

pub type Result<T> = std::result::Result<T, DbError>;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("not found")]
    NotFound,

    #[error("repository has associated tasks")]
    RepoInUse,

    #[error("Project has immutable Releases")]
    ProjectHasReleases,

    #[error("Project {project_id} has no other owner to receive canonical ownership")]
    ProjectOwnerRequired { project_id: String },

    #[error("cannot remove the last administrator")]
    LastAdmin,

    #[error("project owner or administrator membership is required")]
    ProjectAdminRequired,

    #[error("Project owner, admin, or member membership is required")]
    ProjectWriteRequired,

    #[error("version conflict")]
    VersionConflict,

    #[error("idempotency key conflicts with a different mutation")]
    IdempotencyConflict,

    #[error("orchestrator action authority became stale before its effect was written")]
    StaleOrchestratorAction,

    #[error("a Task integration operation is already active")]
    TaskIntegrationOperationBusy,

    #[error("task version conflict: expected {expected}, actual {actual}")]
    TaskVersionConflict { expected: i64, actual: i64 },

    #[error("board revision conflict: expected {expected}, actual {actual}")]
    BoardRevisionConflict { expected: i64, actual: i64 },

    #[error("invalid transition")]
    InvalidTransition,

    #[error("invalid soft delete")]
    InvalidSoftDelete,

    #[error("agent at capacity")]
    AgentAtCapacity,

    #[error("Task is at its automatic Execution capacity")]
    TaskAtCapacity,

    #[error("Task dispatch is paused")]
    TaskDispatchPaused,

    #[error("Task dispatch is cancelled")]
    TaskDispatchCancelled,

    #[error("WorkUnit must resolve its exact interrupted Execution before fresh admission")]
    WorkUnitRecoveryPending,

    #[error("agent is paused: {agent_id}")]
    AgentPaused { agent_id: String },

    #[error("WorkUnit is not open for Execution admission")]
    WorkUnitNotOpen,

    #[error("Execution Actor is not an active member of the WorkUnit role")]
    WorkUnitRoleMembershipInactive,

    #[error("WorkUnit Workspace is not available for Execution admission")]
    WorkUnitWorkspaceUnavailable,

    #[error("Project Copilot already has an active run")]
    ProjectCopilotActiveRun,

    #[error("Project Copilot run authority is stale")]
    ProjectCopilotRunAuthority,

    #[error("WorkUnit already has a Running Execution")]
    WorkUnitExecutionAlreadyRunning,

    #[error("Coordinator turn budget exhausted; a Human may authorize extra rounds")]
    CoordinationTurnBudgetExhausted,

    #[error("Task quality-request budget is exhausted")]
    QualityRequestBudgetExhausted,

    #[error("dependency gate")]
    DependencyGate,

    #[error("cycle detected")]
    CycleDetected,

    #[error("invalid cursor")]
    InvalidCursor,

    #[error("check constraint failed: {0}")]
    Check(String),

    #[error("failed to read migration directory {path}: {source}")]
    ReadMigrationDir { path: PathBuf, source: io::Error },

    #[error("failed to read migration file {path}: {source}")]
    ReadMigrationFile { path: PathBuf, source: io::Error },

    #[error("invalid migration filename {path}")]
    InvalidMigrationFilename { path: PathBuf },

    #[error("invalid migration version in {path}: {source}")]
    InvalidMigrationVersion {
        path: PathBuf,
        source: std::num::ParseIntError,
    },
}
