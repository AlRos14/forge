use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::{OutputError, OutputResult};

pub const MAX_MICRO_FIX_FINDINGS: usize = 3;
pub const MAX_MICRO_FIX_PATHS: usize = 3;
pub const MAX_MICRO_FIX_INSTRUCTIONS_BYTES: usize = 4096;

/// A bounded correction request. It gives the worker a small finding set and
/// a path allowlist; it never contains a patch or repository write capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MicroFixOutput {
    pub finding_refs: Vec<String>,
    pub instructions: String,
    pub paths: Vec<String>,
}

impl MicroFixOutput {
    pub fn validate(&self) -> OutputResult<()> {
        if self.finding_refs.is_empty() || self.finding_refs.len() > MAX_MICRO_FIX_FINDINGS {
            return Err(OutputError::invalid(
                "micro_fix requires 1 to 3 finding references",
            ));
        }
        let mut findings = HashSet::with_capacity(self.finding_refs.len());
        if self.finding_refs.iter().any(|reference| {
            reference.trim().is_empty()
                || reference.len() > 128
                || !findings.insert(reference.as_str())
        }) {
            return Err(OutputError::invalid(
                "micro_fix finding references must be unique and contain 1 to 128 bytes",
            ));
        }
        if self.instructions.trim().is_empty()
            || self.instructions.len() > MAX_MICRO_FIX_INSTRUCTIONS_BYTES
        {
            return Err(OutputError::invalid(
                "micro_fix instructions must contain 1 to 4096 bytes",
            ));
        }
        if self.paths.is_empty() || self.paths.len() > MAX_MICRO_FIX_PATHS {
            return Err(OutputError::invalid("micro_fix requires 1 to 3 paths"));
        }
        let mut paths = HashSet::with_capacity(self.paths.len());
        if self.paths.iter().any(|path| {
            path.trim().is_empty()
                || path.len() > 1024
                || path.starts_with('/')
                || path.contains('\\')
                || path.chars().any(char::is_control)
                || path
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
                || !paths.insert(path.as_str())
        }) {
            return Err(OutputError::invalid(
                "micro_fix paths must be unique, relative, normalized paths",
            ));
        }
        Ok(())
    }
}
