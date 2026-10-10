use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::ActorRef;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CreateIntegrationRequest {
    pub platform: String,
    pub base_url: String,
    pub owner: String,
    pub repo: String,
    pub credential_env_var: String,
    #[ts(optional = nullable)]
    pub default_implementer: Option<ActorRef>,
    #[ts(type = "number | null")]
    #[ts(optional = nullable)]
    pub poll_interval_secs: Option<i64>,
    #[ts(type = "Record<string, unknown> | null")]
    #[ts(optional = nullable)]
    pub sync_filter: Option<Value>,
    #[ts(optional = nullable)]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct PatchIntegrationRequest {
    #[ts(optional = nullable)]
    pub platform: Option<String>,
    #[ts(optional = nullable)]
    pub base_url: Option<String>,
    #[ts(optional = nullable)]
    pub owner: Option<String>,
    #[ts(optional = nullable)]
    pub repo: Option<String>,
    #[ts(optional = nullable)]
    pub credential_env_var: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_update_field")]
    #[ts(optional)]
    pub default_implementer: Option<Option<ActorRef>>,
    #[ts(type = "number | null")]
    #[ts(optional = nullable)]
    pub poll_interval_secs: Option<i64>,
    #[ts(type = "Record<string, unknown> | null")]
    #[ts(optional = nullable)]
    pub sync_filter: Option<Value>,
    #[ts(optional = nullable)]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateExternalLinkRequest {
    #[ts(type = "number")]
    pub remote_issue_number: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct IntegrationResponse {
    pub id: String,
    pub project_id: String,
    pub platform: String,
    pub base_url: String,
    pub owner: String,
    pub repo: String,
    pub credential_env_var: String,
    pub default_implementer: Option<ActorRef>,
    #[ts(type = "number")]
    pub poll_interval_secs: i64,
    #[ts(type = "Record<string, unknown>")]
    pub sync_filter: Value,
    pub enabled: bool,
    pub last_polled_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

fn deserialize_optional_update_field<'de, D, T>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExternalLinkResponse {
    pub id: String,
    pub task_id: String,
    pub integration_id: String,
    pub platform: String,
    pub remote_owner: String,
    pub remote_repo: String,
    #[ts(type = "number")]
    pub remote_issue_number: i64,
    pub remote_url: String,
    pub global_id: String,
    pub synced_at: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SyncTriggerResponse {
    #[ts(type = "number")]
    pub imported: u32,
    #[ts(type = "number")]
    pub skipped: u32,
    #[ts(type = "number")]
    pub errors: u32,
}
