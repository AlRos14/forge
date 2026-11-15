//! Typed coordinator outputs (plan: coordinator-verification-and-pr-loop,
//! Congelado 4, node R1). Phase 1 covers `request_validation` and `ask_human`.
//!
//! This module is pure: it parses and validates the frozen JSON shapes and
//! evaluates the asymmetric action-policy rule. It performs no I/O and does not
//! depend on the coordination envelope type; callers pass the envelope pins
//! through [`CheckPins`]. Applying an output (queueing a check request, opening
//! a human decision) is the job of the wiring node and of
//! `human_decision_service`.

mod ask_human;
mod validation_request;

pub use ask_human::{
    AskHumanOutput, EvidenceKind, EvidenceLink, HumanDecisionOption, MAX_ASK_HUMAN_EVIDENCE_LINKS,
    MAX_ASK_HUMAN_OPTIONS, MAX_QUESTION_BYTES, MIN_ASK_HUMAN_OPTIONS,
};
pub use validation_request::{
    CheckPins, RequestValidationOutput, ValidationSubject, ValidationSubjectKind, MAX_REASON_BYTES,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ServiceError;

/// Maximum number of new-kind outputs a single coordinator turn may carry.
pub const MAX_NEW_OUTPUTS_PER_TURN: usize = 4;

pub const CODE_OUTPUT_NOT_PERMITTED: &str = "coordination.output_not_permitted";
pub const CODE_INVALID_OUTPUT: &str = "coordination.invalid_plan";
pub const CODE_CHECK_NOT_AUTHORIZED: &str = "validation.check_not_authorized";
pub const CODE_INVALID_OPTIONS: &str = "human_decision.invalid_options";
pub const CODE_EVIDENCE_INVALID: &str = "human_decision.evidence_invalid";

/// A rejected coordinator output with its stable public error code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputError {
    pub code: &'static str,
    pub message: String,
}

impl OutputError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new(CODE_INVALID_OUTPUT, message)
    }
}

impl std::fmt::Display for OutputError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for OutputError {}

impl From<OutputError> for ServiceError {
    fn from(error: OutputError) -> Self {
        ServiceError::Coordination {
            code: error.code,
            message: error.message,
        }
    }
}

pub type OutputResult<T> = std::result::Result<T, OutputError>;

/// The new output kinds that, unlike the legacy kinds, are never implied by an
/// absent `allowed_actions` list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorOutputKind {
    RequestValidation,
    AskHuman,
}

impl CoordinatorOutputKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RequestValidation => "request_validation",
            Self::AskHuman => "ask_human",
        }
    }
}

/// Asymmetric policy rule: the new kinds are permitted only when explicitly
/// listed in `allowed_actions`. `None` (the legacy "everything allowed") and an
/// empty list both deny them, so existing roles are not widened.
pub fn ensure_new_kind_permitted(
    kind: CoordinatorOutputKind,
    allowed_actions: Option<&[CoordinatorOutputKind]>,
) -> OutputResult<()> {
    match allowed_actions {
        Some(allowed) if allowed.contains(&kind) => Ok(()),
        _ => Err(OutputError::new(
            CODE_OUTPUT_NOT_PERMITTED,
            format!(
                "the TaskRole policy does not explicitly allow the {} output",
                kind.as_str()
            ),
        )),
    }
}

/// One typed coordinator output. `deny_unknown_fields` on each variant body
/// rejects any command, path, commit, workspace or timeout the coordinator
/// might try to smuggle in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CoordinatorOutput {
    RequestValidation(RequestValidationOutput),
    AskHuman(AskHumanOutput),
}

impl CoordinatorOutput {
    pub fn kind(&self) -> CoordinatorOutputKind {
        match self {
            Self::RequestValidation(_) => CoordinatorOutputKind::RequestValidation,
            Self::AskHuman(_) => CoordinatorOutputKind::AskHuman,
        }
    }

    /// Strictly decode one output value and run its structural validation.
    pub fn from_value(value: Value) -> OutputResult<Self> {
        let output: Self = serde_json::from_value(value).map_err(|error| {
            OutputError::invalid(format!(
                "coordinator output is not a supported shape: {error}"
            ))
        })?;
        match &output {
            Self::RequestValidation(request) => request.validate()?,
            Self::AskHuman(ask) => ask.validate()?,
        }
        Ok(output)
    }
}

/// Decode and validate a turn's new-kind outputs: count bound, per-output
/// structure, explicit policy permission, and (for validation requests) the
/// envelope pin.
pub fn parse_outputs<P: CheckPins + ?Sized>(
    values: Vec<Value>,
    pins: &P,
    allowed_actions: Option<&[CoordinatorOutputKind]>,
) -> OutputResult<Vec<CoordinatorOutput>> {
    if values.len() > MAX_NEW_OUTPUTS_PER_TURN {
        return Err(OutputError::invalid(format!(
            "a coordinator turn may carry at most {MAX_NEW_OUTPUTS_PER_TURN} new outputs"
        )));
    }
    let mut outputs = Vec::with_capacity(values.len());
    for value in values {
        let output = CoordinatorOutput::from_value(value)?;
        ensure_new_kind_permitted(output.kind(), allowed_actions)?;
        if let CoordinatorOutput::RequestValidation(request) = &output {
            request.ensure_pinned(pins)?;
        }
        outputs.push(output);
    }
    Ok(outputs)
}

#[cfg(test)]
mod tests;
