use super::*;
use db::{TaskRepositoryRole, TaskRepositoryWrite};
use sqlx::{Sqlite, Transaction};
use std::collections::HashSet;

pub(super) fn validate_task_repository_writes(rows: &[TaskRepositoryWrite]) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }

    let primary_count = rows
        .iter()
        .filter(|row| row.role == TaskRepositoryRole::Primary)
        .count();
    let mut repo_ids = HashSet::with_capacity(rows.len());
    if primary_count != 1
        || rows
            .iter()
            .any(|row| !repo_ids.insert(row.repo_id.as_str()))
    {
        return Err(ServiceError::TaskRepositoriesInvalid);
    }
    Ok(())
}

pub(super) async fn validate_repositories_belong_to_project_in_tx(
    transaction: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    rows: &[TaskRepositoryWrite],
) -> Result<()> {
    for row in rows {
        let repository_project_id: Option<String> =
            sqlx::query_scalar("SELECT project_id FROM repo WHERE id = ?")
                .bind(&row.repo_id)
                .fetch_optional(&mut **transaction)
                .await?;
        if repository_project_id.as_deref() != Some(project_id) {
            return Err(ServiceError::TaskRepositoryNotInProject);
        }
    }
    Ok(())
}

impl TaskService {
    pub async fn replace_task_repositories(
        &self,
        task_id: &str,
        expected_version: i64,
        repositories: Vec<TaskRepositoryWrite>,
    ) -> Result<Task> {
        validate_task_repository_writes(&repositories)?;
        let now = now_rfc3339();
        let mut transaction = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let task = TaskRepo::get_by_id_in_tx(&*self.db, &mut transaction, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        if task.version != expected_version {
            return Err(ServiceError::Db(db::DbError::TaskVersionConflict {
                expected: expected_version,
                actual: task.version,
            }));
        }
        validate_repositories_belong_to_project_in_tx(
            &mut transaction,
            &task.project_id,
            &repositories,
        )
        .await?;

        let has_execution: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM execution WHERE task_id = ?)")
                .bind(task_id)
                .fetch_one(&mut *transaction)
                .await?;
        if has_execution {
            return Err(ServiceError::TaskRepositoryReplanRequired);
        }

        let selected_repository_ids: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT repo_id FROM work_unit_repository WHERE task_id = ?",
        )
        .bind(task_id)
        .fetch_all(&mut *transaction)
        .await?;
        if selected_repository_ids.iter().any(|repo_id| {
            !repositories.iter().any(|repository| {
                repository.repo_id.as_str() == repo_id.as_str()
                    && matches!(
                        repository.role,
                        db::TaskRepositoryRole::Primary | db::TaskRepositoryRole::Affected
                    )
            })
        }) {
            return Err(ServiceError::TaskRepositoryReplanRequired);
        }

        let task = TaskRepo::replace_repositories_in_tx(
            &*self.db,
            &mut transaction,
            task_id,
            expected_version,
            &repositories,
            &now,
        )
        .await?;
        transaction.commit().await?;
        let repositories = repositories
            .into_iter()
            .map(|repository| api_types::TaskRepositoryInput {
                repo_id: repository.repo_id,
                role: match repository.role {
                    TaskRepositoryRole::Primary => api_types::TaskRepositoryRole::Primary,
                    TaskRepositoryRole::Affected => api_types::TaskRepositoryRole::Affected,
                    TaskRepositoryRole::Context => api_types::TaskRepositoryRole::Context,
                },
            })
            .collect();
        self.event_bus.publish(ForgeEvent {
            event_type: events::TASK_REPOSITORIES_CHANGED_EVENT.to_owned(),
            entity_id: task.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskRepositoriesChanged {
                task_id: task.id.clone(),
                task_version: task.version,
                repositories,
            },
        });
        Ok(task)
    }
}
