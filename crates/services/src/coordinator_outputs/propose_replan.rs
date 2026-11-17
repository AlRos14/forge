use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::{EvidenceLink, OutputError, OutputResult};

pub const MAX_REPLAN_REASON_BYTES: usize = 2048;
pub const MAX_REPLAN_INSTRUCTIONS_BYTES: usize = 4096;
pub const MAX_REPLAN_EVIDENCE_LINKS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplanRequest {
    pub instructions: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposeReplanOutput {
    pub reason: String,
    #[serde(default)]
    pub evidence_links: Vec<EvidenceLink>,
    pub plan_request: ReplanRequest,
}

impl ProposeReplanOutput {
    pub fn validate(&self) -> OutputResult<()> {
        if self.reason.trim().is_empty() || self.reason.len() > MAX_REPLAN_REASON_BYTES {
            return Err(OutputError::invalid(
                "propose_replan reason must contain 1 to 2048 bytes",
            ));
        }
        if self.evidence_links.len() > MAX_REPLAN_EVIDENCE_LINKS {
            return Err(OutputError::invalid(
                "propose_replan accepts at most 16 evidence links",
            ));
        }
        let mut links = HashSet::with_capacity(self.evidence_links.len());
        for link in &self.evidence_links {
            link.validate()?;
            if !links.insert((link.kind, link.id.as_str())) {
                return Err(OutputError::invalid(
                    "propose_replan evidence links must be unique",
                ));
            }
        }
        if self.plan_request.instructions.trim().is_empty()
            || self.plan_request.instructions.len() > MAX_REPLAN_INSTRUCTIONS_BYTES
        {
            return Err(OutputError::invalid(
                "propose_replan plan_request.instructions must contain 1 to 4096 bytes",
            ));
        }
        Ok(())
    }
}
