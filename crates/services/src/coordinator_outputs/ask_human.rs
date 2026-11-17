use std::collections::HashSet;

use db::AuthorityEffect;
use serde::{Deserialize, Serialize};

use super::{OutputError, OutputResult, CODE_EVIDENCE_INVALID, CODE_INVALID_OPTIONS};

pub const MAX_QUESTION_BYTES: usize = 1024;
pub const MIN_ASK_HUMAN_OPTIONS: usize = 2;
pub const MAX_ASK_HUMAN_OPTIONS: usize = 6;
pub const MAX_ASK_HUMAN_EVIDENCE_LINKS: usize = 16;
const MAX_OPTION_ID_BYTES: usize = 32;
const MAX_OPTION_LABEL_BYTES: usize = 80;
const MAX_OPTION_TEXT_BYTES: usize = 512;
const MAX_RATIONALE_BYTES: usize = 2048;
const MAX_OPTIONS_JSON_BYTES: usize = 8192;
const MAX_EVIDENCE_ID_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanDecisionOption {
    pub id: String,
    pub label: String,
    pub description: String,
    pub consequence: String,
    pub authority_effect: AuthorityEffect,
}

/// Closed set mirrored by the V174 insert guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Artifact,
    ReviewReport,
    ValidationRun,
    ValidationComparison,
    GateEvaluation,
    Execution,
    ExternalFeedback,
    WorkUnit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceLink {
    pub kind: EvidenceKind,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

impl EvidenceLink {
    /// Structural validation shared by the coordinator output and the service.
    pub fn validate(&self) -> OutputResult<()> {
        if self.id.trim().is_empty() || self.id.len() > MAX_EVIDENCE_ID_BYTES {
            return Err(OutputError::new(
                CODE_EVIDENCE_INVALID,
                "evidence link id must contain 1 to 128 bytes",
            ));
        }
        if let Some(digest) = self.digest.as_deref() {
            if digest.len() != 64
                || !digest
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
            {
                return Err(OutputError::new(
                    CODE_EVIDENCE_INVALID,
                    "evidence link digest must be 64 lowercase hex characters",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskHumanOutput {
    pub question: String,
    pub options: Vec<HumanDecisionOption>,
    #[serde(default)]
    pub recommended_option_id: Option<String>,
    #[serde(default)]
    pub recommendation_rationale: Option<String>,
    #[serde(default)]
    pub evidence_links: Vec<EvidenceLink>,
}

fn invalid_options(message: impl Into<String>) -> OutputError {
    OutputError::new(CODE_INVALID_OPTIONS, message)
}

impl AskHumanOutput {
    pub fn validate(&self) -> OutputResult<()> {
        if self.question.trim().is_empty() || self.question.len() > MAX_QUESTION_BYTES {
            return Err(invalid_options(
                "ask_human question must contain 1 to 1024 bytes",
            ));
        }
        if !(MIN_ASK_HUMAN_OPTIONS..=MAX_ASK_HUMAN_OPTIONS).contains(&self.options.len()) {
            return Err(invalid_options("ask_human requires 2 to 6 options"));
        }
        let mut ids = HashSet::with_capacity(self.options.len());
        for option in &self.options {
            if option.id.is_empty()
                || option.id.len() > MAX_OPTION_ID_BYTES
                || option
                    .id
                    .bytes()
                    .any(|byte| !matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
            {
                return Err(invalid_options(
                    "option id must be a 1-32 byte slug of [a-z0-9_-]",
                ));
            }
            if !ids.insert(option.id.as_str()) {
                return Err(invalid_options("option ids must be unique"));
            }
            if option.label.trim().is_empty() || option.label.len() > MAX_OPTION_LABEL_BYTES {
                return Err(invalid_options("option label must contain 1 to 80 bytes"));
            }
            if option.description.len() > MAX_OPTION_TEXT_BYTES
                || option.consequence.len() > MAX_OPTION_TEXT_BYTES
            {
                return Err(invalid_options(
                    "option description and consequence are limited to 512 bytes",
                ));
            }
        }
        if self.options_json()?.len() > MAX_OPTIONS_JSON_BYTES {
            return Err(invalid_options("ask_human options exceed 8192 bytes"));
        }
        match self.recommended_option_id.as_deref() {
            Some(id) if !ids.contains(id) => {
                return Err(invalid_options("recommended_option_id must name an option"));
            }
            None if self.recommendation_rationale.is_some() => {
                return Err(invalid_options(
                    "a recommendation rationale requires recommended_option_id",
                ));
            }
            _ => {}
        }
        if self
            .recommendation_rationale
            .as_deref()
            .is_some_and(|rationale| rationale.len() > MAX_RATIONALE_BYTES)
        {
            return Err(invalid_options(
                "recommendation_rationale is limited to 2048 bytes",
            ));
        }
        if self.evidence_links.len() > MAX_ASK_HUMAN_EVIDENCE_LINKS {
            return Err(OutputError::new(
                CODE_EVIDENCE_INVALID,
                "ask_human accepts at most 16 evidence links",
            ));
        }
        for link in &self.evidence_links {
            link.validate()?;
        }
        Ok(())
    }

    /// Highest authority effect among the options (matches the V174 trigger).
    pub fn max_authority_effect(&self) -> AuthorityEffect {
        self.options
            .iter()
            .map(|option| option.authority_effect)
            .max()
            .unwrap_or(AuthorityEffect::None)
    }

    /// Canonical, deterministic storage form of the options.
    pub fn options_json(&self) -> OutputResult<String> {
        serde_json::to_string(&self.options)
            .map_err(|error| invalid_options(format!("options are not serializable: {error}")))
    }

    /// Canonical, deterministic storage form of the evidence links.
    pub fn evidence_links_json(&self) -> OutputResult<String> {
        serde_json::to_string(&self.evidence_links).map_err(|error| {
            OutputError::new(
                CODE_EVIDENCE_INVALID,
                format!("evidence links are not serializable: {error}"),
            )
        })
    }
}
