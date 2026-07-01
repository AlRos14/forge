use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct RepoSyncResponse {
    pub pull_output: String,
    pub push_output: String,
}
