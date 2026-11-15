use serde::{Deserialize, Serialize};

use super::{OutputError, OutputResult, CODE_CHECK_NOT_AUTHORIZED};

pub const MAX_REASON_BYTES: usize = 1024;
const MAX_CHECK_KEY_BYTES: usize = 64;
const MAX_SUBJECT_ID_BYTES: usize = 128;

/// Envelope pins visible to the validator. The coordination envelope type lives
/// elsewhere; any source of pins (the envelope, a test fixture) implements this.
pub trait CheckPins {
    /// `true` when `check_key` is pinned in the envelope and its trigger
    /// authorization admits a coordinator (`on_request`) request.
    fn pins_coordinator_request(&self, check_key: &str) -> bool;
}

impl CheckPins for [String] {
    fn pins_coordinator_request(&self, check_key: &str) -> bool {
        self.iter().any(|pinned| pinned == check_key)
    }
}

impl CheckPins for Vec<String> {
    fn pins_coordinator_request(&self, check_key: &str) -> bool {
        self.as_slice().pins_coordinator_request(check_key)
    }
}

impl CheckPins for std::collections::HashSet<String> {
    fn pins_coordinator_request(&self, check_key: &str) -> bool {
        self.contains(check_key)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationSubjectKind {
    WorkUnitResult,
    TaskIntegration,
}

/// The exact subject version the coordinator saw. For `task_integration`, `id`
/// is the Task id and `version` the Task version. Forge resolves the workspace,
/// commit and snapshot itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationSubject {
    pub kind: ValidationSubjectKind,
    pub id: String,
    pub version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestValidationOutput {
    pub check_key: String,
    pub subject: ValidationSubject,
    #[serde(default)]
    pub baseline: bool,
    pub reason: String,
}

impl RequestValidationOutput {
    pub fn validate(&self) -> OutputResult<()> {
        if self.check_key.is_empty()
            || self.check_key.len() > MAX_CHECK_KEY_BYTES
            || self
                .check_key
                .bytes()
                .any(|byte| !matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
        {
            return Err(OutputError::invalid(
                "request_validation check_key must be a 1-64 byte slug of [a-z0-9_-]",
            ));
        }
        if self.subject.id.trim().is_empty() || self.subject.id.len() > MAX_SUBJECT_ID_BYTES {
            return Err(OutputError::invalid(
                "request_validation subject id must contain 1 to 128 bytes",
            ));
        }
        if self.subject.version < 1 {
            return Err(OutputError::invalid(
                "request_validation subject version must be at least 1",
            ));
        }
        if self.reason.trim().is_empty() || self.reason.len() > MAX_REASON_BYTES {
            return Err(OutputError::invalid(
                "request_validation reason must contain 1 to 1024 bytes",
            ));
        }
        Ok(())
    }

    /// The check must be pinned by the exact current authorization envelope.
    pub fn ensure_pinned<P: CheckPins + ?Sized>(&self, pins: &P) -> OutputResult<()> {
        if pins.pins_coordinator_request(&self.check_key) {
            Ok(())
        } else {
            Err(OutputError::new(
                CODE_CHECK_NOT_AUTHORIZED,
                format!(
                    "check {} is not pinned for coordinator requests in the current envelope",
                    self.check_key
                ),
            ))
        }
    }
}
