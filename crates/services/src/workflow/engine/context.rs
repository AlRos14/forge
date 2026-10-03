use db::{ExecutionRepo, PageRequest, SortBy, SortOrder};

pub(super) async fn latest_execution_context(
    db: &db::SqliteDb,
    task_id: &str,
) -> crate::Result<Option<db::Execution>> {
    let page = ExecutionRepo::list_by_task(
        db,
        task_id,
        PageRequest {
            cursor: None,
            limit: 100,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await?;
    Ok(page
        .items
        .into_iter()
        .find(|execution| execution.work_unit_id.is_none()))
}

pub(super) async fn latest_executor_context(
    db: &db::SqliteDb,
    task_id: &str,
) -> crate::Result<Option<db::Execution>> {
    let page = ExecutionRepo::list_by_task(
        db,
        task_id,
        PageRequest {
            cursor: None,
            limit: 20,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await?;
    Ok(page.items.into_iter().find(|execution| {
        execution.work_unit_id.is_none() && matches!(execution.role.as_str(), "coder" | "executor")
    }))
}
