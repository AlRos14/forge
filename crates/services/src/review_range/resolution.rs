//! Open findings and the Human decisions that change them. B0 declares the
//! contract.
//!
//! Waiver authority (C0.13): any Human with an active RoleMembership in the
//! Task, or a Project owner/admin, always with a non-empty reference. P0 is
//! never waivable.

use super::not_implemented;
use crate::Result;
use db::{
    ActorRef, ReviewCheckpoint, ReviewFinding, ReviewFindingResolution, SqliteDb, TaskReviewBase,
};
use std::{path::PathBuf, sync::Arc};

pub struct ReviewFindingService {
    #[allow(dead_code)] // Read by the B resolution step.
    db: Arc<SqliteDb>,
    #[allow(dead_code)] // Read by the B resolution step.
    workspace_root: PathBuf,
}

impl ReviewFindingService {
    pub fn new(db: Arc<SqliteDb>, workspace_root: PathBuf) -> Self {
        Self { db, workspace_root }
    }

    pub async fn open_findings(
        &self,
        _task_id: &str,
        _repo_id: &str,
        _candidate: &str,
    ) -> Result<Vec<ReviewFinding>> {
        not_implemented("listing open findings")
    }

    pub async fn waive(
        &self,
        _user_id: &str,
        _task_id: &str,
        _finding_id: &str,
        _expected_coverage_revision: i64,
        _rationale: String,
        _reference: String,
    ) -> Result<ReviewFindingResolution> {
        not_implemented("waiving a finding")
    }

    /// The new base is the merge-base of the default branch and the candidate,
    /// resolved by Forge.
    pub async fn rebaseline(
        &self,
        _user_id: &str,
        _task_id: &str,
        _repo_id: &str,
        _expected_coverage_revision: i64,
        _rationale: String,
    ) -> Result<TaskReviewBase> {
        not_implemented("re-baselining a review")
    }

    pub async fn attribute_commits(
        &self,
        _user_id: &str,
        _task_id: &str,
        _checkpoint_id: &str,
        _expected_version: i64,
        _attributions: Vec<(String, ActorRef)>,
        _rationale: String,
    ) -> Result<ReviewCheckpoint> {
        not_implemented("attributing unrecorded commits")
    }
}
