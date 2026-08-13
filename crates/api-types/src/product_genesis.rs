use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Durable lifecycle of a typed Genesis discovery interaction.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProductGenesisLifecycle {
    Discovering,
    ReadyForProject,
    HandedOff,
    Cancelled,
}

/// The intended depth of a Product Genesis discovery session.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProductMaturity {
    Prototype,
    #[default]
    Mvp,
    Production,
    Critical,
}

impl ProductMaturity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prototype => "prototype",
            Self::Mvp => "mvp",
            Self::Production => "production",
            Self::Critical => "critical",
        }
    }
}

impl ProductGenesisLifecycle {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Discovering => "discovering",
            Self::ReadyForProject => "ready_for_project",
            Self::HandedOff => "handed_off",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ProductGenesisSession {
    pub id: String,
    pub account_id: String,
    pub main_chat_id: String,
    pub prompt_revision: String,
    pub maturity: ProductMaturity,
    pub initial_idea: Option<String>,
    pub lifecycle: ProductGenesisLifecycle,
    pub source_message_ids: Vec<String>,
    pub preferred_project_agent_identity_id: Option<String>,
    pub project_id: Option<String>,
    pub handoff_id: Option<String>,
    pub failure_reason: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct StartProductGenesisRequest {
    pub maturity: Option<ProductMaturity>,
    pub initial_idea: Option<String>,
    pub preferred_project_agent_identity_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct CancelProductGenesisRequest {
    pub expected_version: i64,
    pub reason: Option<String>,
}

/// A typed readiness acknowledgement from the Main discovery protocol. The
/// Project creation request remains a separate normal Project mutation so its
/// atomic chat/binding transaction is reused rather than duplicated here.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ReadyProductGenesisRequest {
    pub expected_version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ProductGenesisStartResponse {
    pub session: ProductGenesisSession,
    pub main_chat_id: String,
    pub admitted_turn_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ProductGenesisActiveResponse {
    pub session: Option<ProductGenesisSession>,
}
