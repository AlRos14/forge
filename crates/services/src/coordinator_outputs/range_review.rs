use serde::{Deserialize, Serialize};

use super::{OutputError, OutputResult};

pub const MAX_RANGE_REVIEW_REASON_BYTES: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestRangeReviewOutput {
    pub repo_id: String,
    pub reason: String,
}

impl RequestRangeReviewOutput {
    pub fn validate(&self) -> OutputResult<()> {
        if self.repo_id.trim().is_empty() {
            return Err(OutputError::invalid(
                "request_range_review repo_id must not be empty",
            ));
        }
        if self.reason.trim().is_empty() || self.reason.len() > MAX_RANGE_REVIEW_REASON_BYTES {
            return Err(OutputError::invalid(
                "request_range_review reason must contain 1 to 1024 bytes",
            ));
        }
        Ok(())
    }
}
