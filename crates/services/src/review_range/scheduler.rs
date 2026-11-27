//! Durable review triggers. EventBus only wakes the scheduler; every decision
//! is taken from persisted facts. B0 declares the contract: nothing is hooked
//! to the event loop and no operation schedules a review yet.

use super::not_implemented;
use crate::Result;
use db::{
    DomainEvent, ReviewCheckpoint, ReviewCheckpointReason, ReviewCheckpointState,
    ReviewRangeReport, SqliteDb, UpsertQueuedReviewCheckpoint,
};
use std::{path::PathBuf, sync::Arc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewRequestSource {
    Human {
        user_id: String,
    },
    Coordinator {
        execution_id: String,
        request_id: String,
    },
}

pub struct ReviewScheduler {
    #[allow(dead_code)] // Read by the B scheduling step.
    db: Arc<SqliteDb>,
    #[allow(dead_code)] // Read by the B scheduling step.
    workspace_root: PathBuf,
}

impl ReviewScheduler {
    pub fn new(db: Arc<SqliteDb>, workspace_root: PathBuf) -> Self {
        Self { db, workspace_root }
    }

    pub async fn process_domain_event(&self, _event: &DomainEvent) -> Result<usize> {
        not_implemented("processing a domain event")
    }

    pub async fn recover(&self, _limit: u32) -> Result<usize> {
        not_implemented("scheduler recovery")
    }

    pub async fn reconcile_task(&self, _task_id: &str) -> Result<usize> {
        not_implemented("reconciling a Task")
    }

    pub async fn request(
        &self,
        _source: ReviewRequestSource,
        _task_id: &str,
        _repo_id: &str,
        _reason: String,
        _idempotency_key: &str,
    ) -> Result<ReviewCheckpoint> {
        not_implemented("requesting a range review")
    }

    /// Supersedes a `blocked` checkpoint and queues the next attempt.
    pub async fn retry(
        &self,
        _user_id: &str,
        _checkpoint_id: &str,
        _expected_version: i64,
    ) -> Result<ReviewCheckpoint> {
        not_implemented("retrying a review checkpoint")
    }

    /// The recheck checkpoint of a report, consumed by triage.
    ///
    /// A report does not carry the head, policy revision or attempt of its
    /// checkpoint, so this value is deliberately unwritable: the empty head
    /// commit is rejected by persistence.
    pub fn recheck_input(
        &self,
        report: &ReviewRangeReport,
        now: &str,
    ) -> UpsertQueuedReviewCheckpoint {
        UpsertQueuedReviewCheckpoint {
            checkpoint: ReviewCheckpoint {
                id: db::new_uuid_v4(),
                task_id: report.task_id.clone(),
                repo_id: report.repo_id.clone(),
                state: ReviewCheckpointState::Queued,
                reasons: vec![ReviewCheckpointReason::Recheck],
                attempt: 0,
                base_commit: None,
                head_commit: String::new(),
                range_id: None,
                policy_revision: 0,
                blocked_code: None,
                blocked_detail_json: None,
                version: 1,
                created_at: now.to_owned(),
                updated_at: now.to_owned(),
            },
            request: None,
            supersede: Vec::new(),
            events: Vec::new(),
        }
    }
}
