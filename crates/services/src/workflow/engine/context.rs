use db::{ExecutionRepo, PageRequest, ReviewRepo, SortBy, SortOrder};

pub(super) async fn latest_review(
    db: &db::SqliteDb,
    task_id: &str,
) -> crate::Result<Option<db::Review>> {
    let reviews = ReviewRepo::list_by_task(db, task_id).await?;
    Ok(reviews
        .into_iter()
        .max_by_key(|review| review.attempt_number))
}

pub(super) async fn latest_execution_context(
    db: &db::SqliteDb,
    task_id: &str,
) -> crate::Result<Option<db::Execution>> {
    let page = ExecutionRepo::list_by_task(
        db,
        task_id,
        PageRequest {
            cursor: None,
            limit: 1,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await?;
    Ok(page.items.into_iter().next())
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
    Ok(page
        .items
        .into_iter()
        .find(|execution| matches!(execution.role.as_str(), "coder" | "executor")))
}
