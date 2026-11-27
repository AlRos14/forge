//! Deterministic attribution of range commits and finding locations to recorded
//! integrations. B0 declares the contract; Git commit metadata is never used as
//! Actor attribution.

use super::not_implemented;
use crate::Result;
use db::{ReviewLocationAttribution, ReviewRange, ReviewRangeContributor, SqliteDb};
use std::{path::PathBuf, sync::Arc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingLocation {
    pub path: String,
    pub line_start: Option<i64>,
    pub line_end: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeContributors {
    pub contributors: Vec<ReviewRangeContributor>,
    pub unrecorded_commits: Vec<String>,
    pub upstream_commits: Vec<String>,
}

pub struct ReviewAttribution {
    #[allow(dead_code)] // Read by the B attribution step.
    db: Arc<SqliteDb>,
    #[allow(dead_code)] // Read by the B attribution step.
    workspace_root: PathBuf,
}

impl ReviewAttribution {
    pub fn new(db: Arc<SqliteDb>, workspace_root: PathBuf) -> Self {
        Self { db, workspace_root }
    }

    /// One attribution per location, in order.
    pub async fn attribute(
        &self,
        _range: &ReviewRange,
        _locations: &[FindingLocation],
    ) -> Result<Vec<ReviewLocationAttribution>> {
        not_implemented("attributing finding locations")
    }

    pub async fn contributors(
        &self,
        _task_id: &str,
        _repo_id: &str,
        _base: &str,
        _head: &str,
    ) -> Result<RangeContributors> {
        not_implemented("resolving range contributors")
    }

    /// Not implemented: no primary author is chosen, so callers must treat the
    /// finding as unattributed.
    pub fn primary_author(
        _locations: &[ReviewLocationAttribution],
    ) -> Option<ReviewLocationAttribution> {
        None
    }
}
