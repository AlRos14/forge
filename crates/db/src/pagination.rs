#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    pub total_count: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRequest {
    pub cursor: Option<String>,
    pub limit: i64,
    pub include_total: bool,
    pub sort_by: SortBy,
    pub sort_order: SortOrder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SortBy {
    CreatedAt,
    UpdatedAt,
    Priority,
    BoardPosition,
    Title,
    Status,
    Agent,
    TaskType,
    Id,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SortOrder {
    Asc,
    Desc,
}
