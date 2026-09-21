use super::*;
use crate::{
    canonical_task_role_name, new_uuid_v4, now_rfc3339, AssigneeKind, CreateTaskRoleAssignment,
    CreateTransitionLog, TaskRoleAssignment, TaskRoleAssignmentRepo, TransitionLog,
    TransitionLogRepo,
};
use std::str::FromStr;

fn map_task_role_assignment_row(
    row: SqliteRow,
) -> std::result::Result<TaskRoleAssignment, DbError> {
    let assignee_type = row
        .get::<Option<String>, _>(3)
        .map(|value| AssigneeKind::from_str(&value).map_err(|_| DbError::InvalidTransition))
        .transpose()?;
    Ok(TaskRoleAssignment {
        id: row.get(0),
        task_id: row.get(1),
        role_name: row.get(2),
        assignee_type,
        assignee_id: row.get(4),
        created_at: row.get(5),
        updated_at: row.get(6),
    })
}

fn map_transition_log_row(row: SqliteRow) -> TransitionLog {
    TransitionLog {
        id: row.get(0),
        task_id: row.get(1),
        from_state: row.get(2),
        to_state: row.get(3),
        trigger_name: row.get(4),
        triggered_by: row.get(5),
        trigger_reason: row.get(6),
        hook_results_json: row.get(7),
        rejection: row.get::<i64, _>(8) != 0,
        created_at: row.get(9),
    }
}

fn map_workflow_sqlx_error(error: sqlx::Error) -> DbError {
    match error {
        sqlx::Error::RowNotFound => DbError::NotFound,
        other => DbError::Sqlx(other),
    }
}

#[async_trait]
impl TaskRoleAssignmentRepo for SqliteDb {
    async fn assign(
        &self,
        input: CreateTaskRoleAssignment,
    ) -> std::result::Result<TaskRoleAssignment, DbError> {
        let mut transaction = self.pool.begin().await.map_err(map_workflow_sqlx_error)?;
        sqlx::query(
            "INSERT INTO task_role_assignment (id, task_id, role_name, assignee_type, assignee_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(task_id, role_name) DO UPDATE SET assignee_type = excluded.assignee_type, assignee_id = excluded.assignee_id, updated_at = excluded.updated_at",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(&input.role_name)
        .bind(input.assignee_type.as_ref().map(ToString::to_string))
        .bind(input.assignee_id.as_deref())
        .bind(&input.created_at)
        .bind(&input.updated_at)
        .execute(&mut *transaction)
        .await
        .map_err(map_workflow_sqlx_error)?;

        let actor_is_valid = match (input.assignee_type.as_ref(), input.assignee_id.as_deref()) {
            (Some(AssigneeKind::Agent), Some(actor_id)) => sqlx::query_scalar::<_, i64>(
                "SELECT EXISTS(SELECT 1 FROM agent_identity WHERE id = ?)",
            )
            .bind(actor_id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(map_workflow_sqlx_error)?
                != 0,
            (Some(AssigneeKind::User), Some(actor_id)) if actor_id != "human" => {
                sqlx::query_scalar::<_, i64>(
                    "SELECT EXISTS(
                        SELECT 1
                        FROM user u
                        JOIN task t ON t.id = ?
                        JOIN project p ON p.id = t.project_id
                        WHERE u.id = ?
                          AND (p.owner_id = u.id OR EXISTS(
                              SELECT 1 FROM project_member pm
                              WHERE pm.project_id = p.id AND pm.user_id = u.id
                          ))
                    )",
                )
                .bind(&input.task_id)
                .bind(actor_id)
                .fetch_one(&mut *transaction)
                .await
                .map_err(map_workflow_sqlx_error)?
                    != 0
            }
            (None, None) => true,
            _ => false,
        };

        // Once the replacement model exists, an invalid legacy write must not
        // leave a contradictory singleton row behind.  Returning while this
        // transaction is open rolls the compatibility INSERT back.  Pre-V088
        // fixtures still retain the old repository's permissive sentinel
        // behavior because they have no replacement TaskRole to protect.
        if !actor_is_valid {
            if let Some(role) = canonical_task_role_name(&input.role_name) {
                let replacement_exists: i64 = sqlx::query_scalar(
                    "SELECT EXISTS(
                        SELECT 1 FROM task_role WHERE task_id = ? AND role = ?
                    )",
                )
                .bind(&input.task_id)
                .bind(role)
                .fetch_one(&mut *transaction)
                .await
                .map_err(map_workflow_sqlx_error)?;
                if replacement_exists != 0 {
                    return Err(DbError::InvalidTransition);
                }
            }
        }

        // This repository is retained as a bounded singleton compatibility
        // writer for existing internal callers and fixtures.  It updates the
        // new authority in the same transaction; it never creates a second
        // independent source of truth.  The multi-member API uses
        // RoleMembershipRepo directly.
        if actor_is_valid {
            if let Some(role) = canonical_task_role_name(&input.role_name) {
                let task_role_id = sqlx::query_scalar::<_, String>(
                    "SELECT id FROM task_role WHERE task_id = ? AND role = ?",
                )
                .bind(&input.task_id)
                .bind(&role)
                .fetch_optional(&mut *transaction)
                .await
                .map_err(map_workflow_sqlx_error)?;
                // A low-level compatibility call must not create the
                // replacement model implicitly. Service-level assignment
                // paths create the TaskRole and membership; fixtures and
                // pre-V088 data may continue to use this bounded legacy
                // writer until a replacement row already exists.
                if let Some(task_role_id) = task_role_id {
                    let current_member_count: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM role_membership
                     WHERE task_role_id = ? AND status IN ('active', 'suspended')",
                )
                .bind(&task_role_id)
                .fetch_one(&mut *transaction)
                .await
                .map_err(map_workflow_sqlx_error)?;

                let same_active = match (
                    input.assignee_type.as_ref(),
                    input.assignee_id.as_deref(),
                ) {
                    (Some(assignee_type), Some(assignee_id)) => {
                        let actor_kind = match assignee_type {
                            AssigneeKind::Agent => "agent",
                            AssigneeKind::User => "human",
                        };
                        sqlx::query_scalar::<_, i64>(
                            "SELECT EXISTS(
                                SELECT 1 FROM role_membership
                                WHERE task_role_id = ?
                                  AND actor_kind = ?
                                  AND actor_id = ?
                                  AND status = 'active'
                            )",
                        )
                        .bind(&task_role_id)
                        .bind(actor_kind)
                        .bind(assignee_id)
                        .fetch_one(&mut *transaction)
                        .await
                        .map_err(map_workflow_sqlx_error)?
                            != 0
                    }
                    _ => false,
                };
                if current_member_count > 1 && !same_active {
                    return Err(DbError::VersionConflict);
                }

                if !same_active {
                    sqlx::query(
                        "UPDATE role_membership
                         SET status = 'ended', ended_at = ?, updated_at = ?, version = version + 1
                         WHERE task_role_id = ? AND status IN ('active', 'suspended')",
                    )
                    .bind(&input.updated_at)
                    .bind(&input.updated_at)
                    .bind(&task_role_id)
                    .execute(&mut *transaction)
                    .await
                    .map_err(map_workflow_sqlx_error)?;
                }

                if !same_active {
                    if let (Some(assignee_type), Some(assignee_id)) = (
                        input.assignee_type.as_ref(),
                        input.assignee_id.as_deref(),
                    ) {
                        let actor_kind = match assignee_type {
                            AssigneeKind::Agent => "agent",
                            AssigneeKind::User => "human",
                        };
                        sqlx::query(
                            "INSERT INTO role_membership
                                (id, task_role_id, actor_kind, actor_id, status, version, created_at, updated_at, ended_at)
                             VALUES (?, ?, ?, ?, 'active', 1, ?, ?, NULL)",
                        )
                        .bind(new_uuid_v4())
                        .bind(&task_role_id)
                        .bind(actor_kind)
                        .bind(assignee_id)
                        .bind(&input.created_at)
                        .bind(&input.updated_at)
                        .execute(&mut *transaction)
                        .await
                        .map_err(map_workflow_sqlx_error)?;
                    }
                }

                if role == "implementer" {
                    match (input.assignee_type.as_ref(), input.assignee_id.as_deref()) {
                        (Some(assignee_type), Some(assignee_id)) => {
                            let assignee_type = assignee_type.to_string();
                            sqlx::query(
                                "UPDATE task_role_assignment
                                 SET assignee_type = ?, assignee_id = ?, updated_at = ?
                                 WHERE task_id = ?
                                   AND role_name IN ('implementer', 'coder', 'worker', 'assignee', 'executor')",
                            )
                            .bind(&assignee_type)
                            .bind(assignee_id)
                            .bind(&input.updated_at)
                            .bind(&input.task_id)
                            .execute(&mut *transaction)
                            .await
                            .map_err(map_workflow_sqlx_error)?;
                            sqlx::query(
                                "UPDATE task
                                 SET assignee_type = ?, assignee_id = ?, updated_at = ?
                                 WHERE id = ?",
                            )
                            .bind(assignee_type)
                            .bind(assignee_id)
                            .bind(&input.updated_at)
                            .bind(&input.task_id)
                            .execute(&mut *transaction)
                            .await
                            .map_err(map_workflow_sqlx_error)?;
                        }
                        _ => {
                            sqlx::query(
                                "UPDATE task_role_assignment
                                 SET assignee_type = NULL, assignee_id = NULL, updated_at = ?
                                 WHERE task_id = ?
                                   AND role_name IN ('implementer', 'coder', 'worker', 'assignee', 'executor')",
                            )
                            .bind(&input.updated_at)
                            .bind(&input.task_id)
                            .execute(&mut *transaction)
                            .await
                            .map_err(map_workflow_sqlx_error)?;
                            sqlx::query(
                                "UPDATE task
                                 SET assignee_type = NULL, assignee_id = NULL, updated_at = ?
                                 WHERE id = ?",
                            )
                            .bind(&input.updated_at)
                            .bind(&input.task_id)
                            .execute(&mut *transaction)
                            .await
                            .map_err(map_workflow_sqlx_error)?;
                        }
                    }
                }
                }
            }
        }

        transaction.commit().await.map_err(map_workflow_sqlx_error)?;

        let row = sqlx::query(
            "SELECT id, task_id, role_name, assignee_type, assignee_id, created_at, updated_at FROM task_role_assignment WHERE task_id = ? AND role_name = ?",
        )
        .bind(&input.task_id)
        .bind(&input.role_name)
        .fetch_one(&self.pool)
        .await
        .map_err(map_workflow_sqlx_error)?;

        map_task_role_assignment_row(row)
    }

    async fn get_by_task_and_role(
        &self,
        task_id: &str,
        role_name: &str,
    ) -> std::result::Result<Option<TaskRoleAssignment>, DbError> {
        match sqlx::query(
            "SELECT id, task_id, role_name, assignee_type, assignee_id, created_at, updated_at FROM task_role_assignment WHERE task_id = ? AND role_name = ?",
        )
        .bind(task_id)
        .bind(role_name)
        .fetch_one(&self.pool)
        .await
        {
            Ok(row) => map_task_role_assignment_row(row).map(Some),
            Err(sqlx::Error::RowNotFound) => Ok(None),
            Err(error) => Err(map_workflow_sqlx_error(error)),
        }
    }

    async fn list_by_task(
        &self,
        task_id: &str,
    ) -> std::result::Result<Vec<TaskRoleAssignment>, DbError> {
        let rows = sqlx::query(
            "SELECT id, task_id, role_name, assignee_type, assignee_id, created_at, updated_at FROM task_role_assignment WHERE task_id = ? ORDER BY role_name",
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await
        .map_err(map_workflow_sqlx_error)?;

        rows.into_iter().map(map_task_role_assignment_row).collect()
    }

    async fn remove(&self, task_id: &str, role_name: &str) -> std::result::Result<(), DbError> {
        let mut transaction = self.pool.begin().await.map_err(map_workflow_sqlx_error)?;
        if let Some(role) = canonical_task_role_name(role_name) {
            if let Some(task_role_id) = sqlx::query_scalar::<_, String>(
                "SELECT id FROM task_role WHERE task_id = ? AND role = ?",
            )
            .bind(task_id)
            .bind(&role)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(map_workflow_sqlx_error)?
            {
                let current_member_count: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM role_membership
                     WHERE task_role_id = ? AND status IN ('active', 'suspended')",
                )
                .bind(task_role_id)
                .fetch_one(&mut *transaction)
                .await
                .map_err(map_workflow_sqlx_error)?;
                if current_member_count > 1 {
                    return Err(DbError::VersionConflict);
                }
            }
        }
        sqlx::query("DELETE FROM task_role_assignment WHERE task_id = ? AND role_name = ?")
            .bind(task_id)
            .bind(role_name)
            .execute(&mut *transaction)
            .await
            .map_err(map_workflow_sqlx_error)?;
        if let Some(role) = canonical_task_role_name(role_name) {
            sqlx::query(
                "UPDATE role_membership
                 SET status = 'ended', ended_at = ?, updated_at = ?, version = version + 1
                 WHERE task_role_id IN (SELECT id FROM task_role WHERE task_id = ? AND role = ?)
                   AND status IN ('active', 'suspended')",
            )
            .bind(now_rfc3339())
            .bind(now_rfc3339())
            .bind(task_id)
            .bind(role)
            .execute(&mut *transaction)
            .await
            .map_err(map_workflow_sqlx_error)?;
            if role == "implementer" {
                sqlx::query(
                    "DELETE FROM task_role_assignment
                     WHERE task_id = ? AND role_name IN ('implementer', 'coder', 'worker', 'assignee', 'executor')",
                )
                .bind(task_id)
                .execute(&mut *transaction)
                .await
                .map_err(map_workflow_sqlx_error)?;
                sqlx::query(
                    "UPDATE task SET assignee_type = NULL, assignee_id = NULL, updated_at = ?
                     WHERE id = ?",
                )
                .bind(now_rfc3339())
                .bind(task_id)
                .execute(&mut *transaction)
                .await
                .map_err(map_workflow_sqlx_error)?;
            }
        }
        transaction.commit().await.map_err(map_workflow_sqlx_error)?;
        Ok(())
    }
}

#[async_trait]
impl TransitionLogRepo for SqliteDb {
    async fn insert(
        &self,
        input: CreateTransitionLog,
    ) -> std::result::Result<TransitionLog, DbError> {
        sqlx::query(
            "INSERT INTO transition_log (id, task_id, from_state, to_state, trigger_name, triggered_by, trigger_reason, hook_results_json, rejection, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(&input.from_state)
        .bind(&input.to_state)
        .bind(input.trigger_name.as_deref())
        .bind(&input.triggered_by)
        .bind(&input.trigger_reason)
        .bind(input.hook_results_json.as_deref())
        .bind(if input.rejection { 1_i64 } else { 0_i64 })
        .bind(&input.created_at)
        .execute(&self.pool)
        .await
        .map_err(map_workflow_sqlx_error)?;

        let row = sqlx::query(
            "SELECT id, task_id, from_state, to_state, trigger_name, triggered_by, trigger_reason, hook_results_json, rejection, created_at FROM transition_log WHERE id = ?",
        )
        .bind(&input.id)
        .fetch_one(&self.pool)
        .await
        .map_err(map_workflow_sqlx_error)?;

        Ok(map_transition_log_row(row))
    }

    async fn insert_recovery_marker(
        &self,
        task_id: &str,
        current_state: &str,
        action_kind: &str,
        triggered_by: &str,
        reason: &str,
    ) -> std::result::Result<TransitionLog, DbError> {
        self.insert(CreateTransitionLog {
            id: new_uuid_v4(),
            task_id: task_id.to_owned(),
            from_state: current_state.to_owned(),
            to_state: current_state.to_owned(),
            trigger_name: Some(action_kind.to_owned()),
            triggered_by: triggered_by.to_owned(),
            trigger_reason: reason.to_owned(),
            hook_results_json: None,
            rejection: false,
            created_at: now_rfc3339(),
        })
        .await
    }

    async fn list_by_task(
        &self,
        task_id: &str,
    ) -> std::result::Result<Vec<TransitionLog>, DbError> {
        let rows = sqlx::query(
            "SELECT id, task_id, from_state, to_state, trigger_name, triggered_by, trigger_reason, hook_results_json, rejection, created_at FROM transition_log WHERE task_id = ? ORDER BY created_at",
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await
        .map_err(map_workflow_sqlx_error)?;

        Ok(rows.into_iter().map(map_transition_log_row).collect())
    }

    async fn count_gate_rejections(
        &self,
        task_id: &str,
        gate_state: &str,
    ) -> std::result::Result<i64, DbError> {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM transition_log WHERE task_id = ? AND from_state = ? AND rejection = 1 AND (NOT EXISTS (SELECT 1 FROM transition_log t2 WHERE t2.task_id = ? AND t2.from_state = ? AND t2.to_state = ? AND t2.trigger_name = 'reset_retry_window') OR created_at > (SELECT MAX(created_at) FROM transition_log t2 WHERE t2.task_id = ? AND t2.from_state = ? AND t2.to_state = ? AND t2.trigger_name = 'reset_retry_window'))",
        )
        .bind(task_id)
        .bind(gate_state)
        .bind(task_id)
        .bind(gate_state)
        .bind(gate_state)
        .bind(task_id)
        .bind(gate_state)
        .bind(gate_state)
        .fetch_one(&self.pool)
        .await
        .map_err(map_workflow_sqlx_error)
    }

    async fn count_to_state_since(
        &self,
        task_id: &str,
        to_state: &str,
        since: Option<&str>,
    ) -> std::result::Result<i64, DbError> {
        let mut query =
            sqlx::QueryBuilder::new("SELECT COUNT(*) FROM transition_log WHERE task_id = ");
        query
            .push_bind(task_id)
            .push(" AND to_state = ")
            .push_bind(to_state);
        if let Some(since) = since {
            query.push(" AND created_at >= ").push_bind(since);
        }
        query
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_err(map_workflow_sqlx_error)
    }

    async fn update_hook_results(
        &self,
        id: &str,
        hook_results_json: &str,
    ) -> std::result::Result<(), DbError> {
        let result = sqlx::query("UPDATE transition_log SET hook_results_json = ? WHERE id = ?")
            .bind(hook_results_json)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(map_workflow_sqlx_error)?;

        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }

        Ok(())
    }
}
