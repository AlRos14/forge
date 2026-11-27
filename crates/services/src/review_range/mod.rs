//! Commit-range review and per-repository coverage
//! (`docs/plans/review-ranges.md`, contract C0.6).
//!
//! Git facts are computed outside transactions; coverage is a pure function of
//! those facts and durable rows.

pub mod attribution;
pub mod coverage;
pub mod policy;
pub mod resolution;
pub mod scheduler;

pub use attribution::{FindingLocation, RangeContributors, ReviewAttribution};
pub use coverage::{
    evaluate_coverage, CommitFacts, CoverageFacts, CoverageSnapshot, CoveredRange, RepoCandidate,
    RepoCoverage, ReviewCoverageService, ReviewCoverageStatus, SeverityCounts,
};
pub use policy::ReviewPolicyService;
pub use resolution::ReviewFindingService;
pub use scheduler::{ReviewRequestSource, ReviewScheduler};

use crate::{Result, ServiceError};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// Lowercase SHA-256 hex of the canonical JSON of `value`: struct fields in
/// declaration order, collections already ordered by the caller.
pub(crate) fn canonical_digest<T: Serialize>(value: &T) -> Result<String> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        ServiceError::invalid_operation(format!("review value cannot be serialized: {error}"))
    })?;
    Ok(hex::encode(Sha256::digest(bytes)))
}
