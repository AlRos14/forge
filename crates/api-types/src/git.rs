use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum MergeOutcomeResponse {
    Done {
        before_sha: String,
        after_sha: String,
        branch: String,
    },
    PullRequest {
        branch: String,
        pr_url: Option<String>,
    },
    Conflict {
        details: String,
        conflict_paths: Vec<String>,
    },
    Dirty {
        files: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct ConflictStateResponse {
    pub operation: String,
    pub conflict_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct RepoSyncResponse {
    pub pull_output: String,
    pub push_output: String,
}
