use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct MemorySearchQuery {
    pub query: String,
    pub layer: Option<u8>,
    pub token_budget: Option<u32>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct MemoryGetQuery {
    pub layer: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct MemorySearchResultDto {
    pub id: String,
    pub layer: u8,
    pub content: String,
    pub score: f32,
    pub source_type: String,
    pub source_id: String,
    pub project_id: String,
    pub task_id: Option<String>,
    pub created_at: String,
    pub creator: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct MemorySearchResponse {
    pub items: Vec<MemorySearchResultDto>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
}
