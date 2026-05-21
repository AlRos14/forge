use async_trait::async_trait;
use db::{Project, SqliteDb};

use crate::{project_hooks::EvaluationCause, Result};

pub mod all_work_completed;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerMatch {
    pub trigger_type: String,
    pub dedupe_key: String,
    pub source_task_id: Option<String>,
    pub source_execution_id: Option<String>,
    pub reason: Option<String>,
}

pub struct TriggerContext<'a> {
    pub db: &'a SqliteDb,
    pub project: &'a Project,
    pub cause: &'a EvaluationCause,
}

#[async_trait]
pub trait HookTrigger {
    async fn evaluate(&self, context: &TriggerContext<'_>) -> Result<Option<TriggerMatch>>;
}
