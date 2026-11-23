use super::*;
use crate::TaskRepository;
use sha2::{Digest, Sha256};

async fn sync_legacy_task_repository_catalog_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    task_id: &str,
    repositories: &[TaskRepositorySetRevisionRepo],
    created_at: &str,
) -> Result<()> {
    let task_repo_id: Option<String> = sqlx::query_scalar("SELECT repo_id FROM task WHERE id = ?")
        .bind(task_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(DbError::NotFound)?;
    let requested_primary = repositories
        .iter()
        .find(|repository| repository.role == TaskRepositoryRole::Primary)
        .map(|repository| repository.repo_id.as_str());
    if task_repo_id.as_deref() != requested_primary {
        return Err(DbError::TaskRepositoryReplanPrimaryImmutable);
    }

    let existing: Vec<String> = sqlx::query_scalar(
        "SELECT repo_id FROM task_repository WHERE task_id = ? ORDER BY repo_id",
    )
    .bind(task_id)
    .fetch_all(&mut **tx)
    .await?;
    let max_position: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(position), 0) FROM task_repository WHERE task_id = ?",
    )
    .bind(task_id)
    .fetch_one(&mut **tx)
    .await?;
    let catalog_count = i64::try_from(existing.len())
        .map_err(|_| DbError::Check("Task repository catalog is too large".to_owned()))?;
    let requested_count = i64::try_from(repositories.len())
        .map_err(|_| DbError::Check("Task repository revision is too large".to_owned()))?;
    let offset = max_position
        .checked_add(catalog_count)
        .and_then(|value| value.checked_add(requested_count))
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| DbError::Check("Task repository position overflow".to_owned()))?;
    sqlx::query(
        "UPDATE task_repository SET position = position + ?
         WHERE task_id = ? AND role != 'primary'",
    )
    .bind(offset)
    .bind(task_id)
    .execute(&mut **tx)
    .await?;

    let mut temporary_position = max_position
        .checked_add(offset)
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| DbError::Check("Task repository position overflow".to_owned()))?;
    for repository in repositories {
        if !existing
            .iter()
            .any(|repo_id| repo_id == &repository.repo_id)
        {
            sqlx::query(
                "INSERT INTO task_repository (task_id, repo_id, role, position, created_at)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(task_id)
            .bind(&repository.repo_id)
            .bind(repository.role.to_string())
            .bind(temporary_position)
            .bind(created_at)
            .execute(&mut **tx)
            .await?;
            temporary_position = temporary_position
                .checked_add(1)
                .ok_or_else(|| DbError::Check("Task repository position overflow".to_owned()))?;
        }
    }

    for repository in repositories {
        sqlx::query(
            "UPDATE task_repository SET role = ?, position = ?
             WHERE task_id = ? AND repo_id = ?",
        )
        .bind(repository.role.to_string())
        .bind(repository.position)
        .bind(task_id)
        .bind(&repository.repo_id)
        .execute(&mut **tx)
        .await?;
    }

    let retained = repositories
        .iter()
        .map(|repository| repository.repo_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let mut retired = existing
        .into_iter()
        .filter(|repo_id| !retained.contains(repo_id.as_str()))
        .collect::<Vec<_>>();
    retired.sort();
    let active_non_primary = i64::try_from(
        repositories
            .iter()
            .filter(|repository| repository.role != TaskRepositoryRole::Primary)
            .count(),
    )
    .map_err(|_| DbError::Check("Task repository revision is too large".to_owned()))?;
    let mut position = active_non_primary
        .checked_add(1)
        .ok_or_else(|| DbError::Check("Task repository position overflow".to_owned()))?;
    for repo_id in retired {
        sqlx::query(
            "UPDATE task_repository SET role = 'context', position = ?
             WHERE task_id = ? AND repo_id = ?",
        )
        .bind(position)
        .bind(task_id)
        .bind(repo_id)
        .execute(&mut **tx)
        .await?;
        position = position
            .checked_add(1)
            .ok_or_else(|| DbError::Check("Task repository position overflow".to_owned()))?;
    }
    Ok(())
}

fn map_revision_member(row: SqliteRow) -> Result<TaskRepositorySetRevisionRepo> {
    Ok(TaskRepositorySetRevisionRepo {
        task_id: row.try_get("task_id")?,
        revision: row.try_get("revision")?,
        repo_id: row.try_get("repo_id")?,
        role: parse_enum(row.try_get::<String, _>("role")?)?,
        position: row.try_get("position")?,
    })
}

async fn load_revision_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    task_id: &str,
    revision: i64,
) -> Result<Option<TaskRepositorySetRevision>> {
    let Some(row) = sqlx::query(
        "SELECT task_id, revision, digest, state, created_at, source_kind,
                source_ref, decision_id, human_decision_request_id
         FROM task_repository_set_revision WHERE task_id = ? AND revision = ?",
    )
    .bind(task_id)
    .bind(revision)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(None);
    };
    let mut header = TaskRepositorySetRevision {
        task_id: row.try_get("task_id")?,
        revision: row.try_get("revision")?,
        digest: row.try_get("digest")?,
        state: parse_enum(row.try_get::<String, _>("state")?)?,
        created_at: row.try_get("created_at")?,
        source_kind: row.try_get("source_kind")?,
        source_ref: row.try_get("source_ref")?,
        decision_id: row.try_get("decision_id")?,
        human_decision_request_id: row.try_get("human_decision_request_id")?,
        repositories: Vec::new(),
    };
    let rows = sqlx::query(
        "SELECT task_id, revision, repo_id, role, position
         FROM task_repository_set_revision_repo
         WHERE task_id = ? AND revision = ?
         ORDER BY position, repo_id",
    )
    .bind(task_id)
    .bind(revision)
    .fetch_all(&mut **tx)
    .await?;
    header.repositories = rows
        .into_iter()
        .map(map_revision_member)
        .collect::<Result<Vec<_>>>()?;
    Ok(Some(header))
}

pub(super) async fn list_current_repositories_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    task_id: &str,
) -> Result<Vec<TaskRepository>> {
    let has_revision_schema: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM sqlite_master
             WHERE type = 'table' AND name = 'task_repository_set_revision'
         ) AND EXISTS(
             SELECT 1 FROM sqlite_master
             WHERE type = 'table' AND name = 'task_repository_set_revision_repo'
         )",
    )
    .fetch_one(&mut **tx)
    .await?;
    if !has_revision_schema {
        return sqlx::query(
            "SELECT task_id, repo_id, role, position, created_at
             FROM task_repository WHERE task_id = ? ORDER BY position, repo_id",
        )
        .bind(task_id)
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .map(|row| {
            Ok(TaskRepository {
                task_id: row.try_get("task_id")?,
                repo_id: row.try_get("repo_id")?,
                role: parse_enum(row.try_get::<String, _>("role")?)?,
                position: row.try_get("position")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect();
    }
    ensure_initial_current_revision_in_tx(tx, task_id).await?;
    let rows = sqlx::query(
        "SELECT member.task_id, member.repo_id, member.role, member.position,
                COALESCE(catalog.created_at, revision.created_at) AS created_at
         FROM task_repository_set_revision revision
         JOIN task_repository_set_revision_repo member
           ON member.task_id = revision.task_id AND member.revision = revision.revision
         LEFT JOIN task_repository catalog
           ON catalog.task_id = member.task_id AND catalog.repo_id = member.repo_id
         WHERE revision.task_id = ? AND revision.state = 'current'
         ORDER BY member.position, member.repo_id",
    )
    .bind(task_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(TaskRepository {
                task_id: row.try_get("task_id")?,
                repo_id: row.try_get("repo_id")?,
                role: parse_enum(row.try_get::<String, _>("role")?)?,
                position: row.try_get("position")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect()
}

fn repository_set_digest(rows: &[TaskRepositorySetRevisionRepo]) -> Result<String> {
    // Struct field order and the explicit SQL order form the canonical JSON
    // representation used for new revisions.
    let canonical: Vec<(&str, String, i64)> = rows
        .iter()
        .map(|row| (row.repo_id.as_str(), row.role.to_string(), row.position))
        .collect();
    let bytes = serde_json::to_vec(&canonical).map_err(|error| {
        DbError::Check(format!(
            "cannot serialize Task repository revision: {error}"
        ))
    })?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn requested_revision_members(
    task_id: &str,
    repositories: &[TaskRepositoryWrite],
) -> Result<Vec<TaskRepositorySetRevisionRepo>> {
    let primary_count = repositories
        .iter()
        .filter(|repository| repository.role == TaskRepositoryRole::Primary)
        .count();
    let mut ids = std::collections::HashSet::with_capacity(repositories.len());
    if (!repositories.is_empty() && primary_count != 1)
        || repositories
            .iter()
            .any(|repository| !ids.insert(repository.repo_id.as_str()))
    {
        return Err(DbError::Check(
            "Task repository revision requires unique members and exactly one primary".to_owned(),
        ));
    }
    let mut next_position = 1_i64;
    let mut members = Vec::with_capacity(repositories.len());
    for repository in repositories {
        let position = if repository.role == TaskRepositoryRole::Primary {
            0
        } else {
            let position = next_position;
            next_position = next_position.checked_add(1).ok_or_else(|| {
                DbError::Check("Task repository revision position overflow".to_owned())
            })?;
            position
        };
        members.push(TaskRepositorySetRevisionRepo {
            task_id: task_id.to_owned(),
            revision: 0,
            repo_id: repository.repo_id.clone(),
            role: repository.role,
            position,
        });
    }
    members.sort_by_key(|member| (member.position, member.repo_id.clone()));
    Ok(members)
}

async fn create_revision_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    input: CreateTaskRepositorySetRevision,
) -> Result<TaskRepositorySetRevision> {
    if input.source_kind.trim().is_empty() {
        return Err(DbError::Check(
            "Task repository revision source kind is required".to_owned(),
        ));
    }
    let current = ensure_initial_current_revision_in_tx(tx, &input.task_id).await?;
    if current.revision != input.expected_current_revision
        || current.digest != input.expected_current_digest
    {
        return Err(DbError::VersionConflict);
    }
    let mut members = requested_revision_members(&input.task_id, &input.repositories)?;
    let desired = members
        .iter()
        .map(|member| (member.repo_id.clone(), member.role, member.position))
        .collect::<Vec<_>>();
    let existing = current
        .repositories
        .iter()
        .map(|member| (member.repo_id.clone(), member.role, member.position))
        .collect::<Vec<_>>();
    if desired == existing && !input.force_new_revision {
        return Ok(current);
    }
    let task_project_id: String = sqlx::query_scalar("SELECT project_id FROM task WHERE id = ?")
        .bind(&input.task_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(DbError::NotFound)?;
    for member in &members {
        let repo_project_id: Option<String> =
            sqlx::query_scalar("SELECT project_id FROM repo WHERE id = ?")
                .bind(&member.repo_id)
                .fetch_optional(&mut **tx)
                .await?;
        if repo_project_id.as_deref() != Some(task_project_id.as_str()) {
            return Err(DbError::Check(
                "Task repository revision member must belong to its Project".to_owned(),
            ));
        }
    }
    sync_legacy_task_repository_catalog_in_tx(tx, &input.task_id, &members, &input.created_at)
        .await?;
    let revision = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(revision), 0) + 1
         FROM task_repository_set_revision WHERE task_id = ?",
    )
    .bind(&input.task_id)
    .fetch_one(&mut **tx)
    .await?;
    let digest_members = members
        .iter()
        .map(|member| TaskRepositorySetRevisionRepo {
            revision,
            ..member.clone()
        })
        .collect::<Vec<_>>();
    let digest = repository_set_digest(&digest_members)?;
    sqlx::query(
        "UPDATE task_repository_set_revision SET state = 'superseded'
         WHERE task_id = ? AND revision = ? AND state = 'current'",
    )
    .bind(&input.task_id)
    .bind(current.revision)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO task_repository_set_revision (
             task_id, revision, digest, state, created_at, source_kind, source_ref,
             decision_id, human_decision_request_id
         ) VALUES (?, ?, ?, 'building', ?, ?, ?, ?, ?)",
    )
    .bind(&input.task_id)
    .bind(revision)
    .bind(&digest)
    .bind(&input.created_at)
    .bind(&input.source_kind)
    .bind(input.source_ref.as_deref())
    .bind(input.decision_id.as_deref())
    .bind(input.human_decision_request_id.as_deref())
    .execute(&mut **tx)
    .await
    .map_err(check_error)?;
    for member in &mut members {
        sqlx::query(
            "INSERT INTO task_repository_set_revision_repo (
                 task_id, revision, repo_id, role, position
             ) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&input.task_id)
        .bind(revision)
        .bind(&member.repo_id)
        .bind(member.role.to_string())
        .bind(member.position)
        .execute(&mut **tx)
        .await
        .map_err(check_error)?;
    }
    sqlx::query(
        "UPDATE task_repository_set_revision SET state = 'current'
         WHERE task_id = ? AND revision = ? AND state = 'building'",
    )
    .bind(&input.task_id)
    .bind(revision)
    .execute(&mut **tx)
    .await?;
    load_revision_in_tx(tx, &input.task_id, revision)
        .await?
        .ok_or(DbError::NotFound)
}

pub(super) async fn pin_work_unit_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    work_unit_id: &str,
    task_id: &str,
    revision: i64,
) -> Result<()> {
    let current: Option<i64> = sqlx::query_scalar(
        "SELECT revision FROM task_repository_set_revision
         WHERE task_id = ? AND state = 'current'",
    )
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    if current != Some(revision) {
        return Err(DbError::VersionConflict);
    }
    let work_unit_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM work_unit WHERE id = ? AND task_id = ?)")
            .bind(work_unit_id)
            .bind(task_id)
            .fetch_one(&mut **tx)
            .await?;
    if !work_unit_exists {
        return Err(DbError::NotFound);
    }
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT revision FROM work_unit_repository_set_revision
         WHERE work_unit_id = ? AND task_id = ?",
    )
    .bind(work_unit_id)
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    match existing {
        Some(existing) if existing == revision => return Ok(()),
        Some(_) => return Err(DbError::VersionConflict),
        None => {}
    }
    sqlx::query(
        "INSERT INTO work_unit_repository_set_revision (work_unit_id, task_id, revision)
         VALUES (?, ?, ?)",
    )
    .bind(work_unit_id)
    .bind(task_id)
    .bind(revision)
    .execute(&mut **tx)
    .await
    .map_err(check_error)?;
    Ok(())
}

pub(super) async fn ensure_initial_current_revision_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    task_id: &str,
) -> Result<TaskRepositorySetRevision> {
    if let Some(revision) = sqlx::query_scalar::<_, i64>(
        "SELECT revision FROM task_repository_set_revision
         WHERE task_id = ? AND state = 'current'",
    )
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?
    {
        return load_revision_in_tx(tx, task_id, revision)
            .await?
            .ok_or(DbError::NotFound);
    }

    let historical_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task_repository_set_revision WHERE task_id = ?")
            .bind(task_id)
            .fetch_one(&mut **tx)
            .await?;
    if historical_count != 0 {
        return Err(DbError::InvalidTransition);
    }
    let task_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM task WHERE id = ?)")
        .bind(task_id)
        .fetch_one(&mut **tx)
        .await?;
    if !task_exists {
        return Err(DbError::NotFound);
    }

    let member_rows = sqlx::query(
        "SELECT task_id, 1 AS revision, repo_id, role, position
         FROM task_repository WHERE task_id = ? ORDER BY position, repo_id",
    )
    .bind(task_id)
    .fetch_all(&mut **tx)
    .await?;
    let members = member_rows
        .into_iter()
        .map(map_revision_member)
        .collect::<Result<Vec<_>>>()?;
    let digest = repository_set_digest(&members)?;
    let created_at = crate::now_rfc3339();
    sqlx::query(
        "INSERT INTO task_repository_set_revision (
             task_id, revision, digest, state, created_at, source_kind, source_ref,
             decision_id, human_decision_request_id
         ) VALUES (?, 1, ?, 'building', ?, 'task_create', NULL, NULL, NULL)",
    )
    .bind(task_id)
    .bind(&digest)
    .bind(&created_at)
    .execute(&mut **tx)
    .await?;
    for member in &members {
        sqlx::query(
            "INSERT INTO task_repository_set_revision_repo (
                 task_id, revision, repo_id, role, position
             ) VALUES (?, 1, ?, ?, ?)",
        )
        .bind(task_id)
        .bind(&member.repo_id)
        .bind(member.role.to_string())
        .bind(member.position)
        .execute(&mut **tx)
        .await?;
    }
    sqlx::query(
        "UPDATE task_repository_set_revision SET state = 'current'
         WHERE task_id = ? AND revision = 1 AND state = 'building'",
    )
    .bind(task_id)
    .execute(&mut **tx)
    .await?;
    load_revision_in_tx(tx, task_id, 1)
        .await?
        .ok_or(DbError::NotFound)
}

#[async_trait]
impl TaskRepositorySetRepo for SqliteDb {
    async fn get_current(&self, task_id: &str) -> Result<Option<TaskRepositorySetRevision>> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let has_repository_set_schema: i64 = sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM sqlite_master
                 WHERE type = 'table' AND name = 'task_repository_set_revision'
             ) AND EXISTS (
                 SELECT 1 FROM sqlite_master
                 WHERE type = 'table' AND name = 'task_repository_set_revision_repo'
             )",
        )
        .fetch_one(&mut *tx)
        .await?;
        if has_repository_set_schema == 0 {
            tx.commit().await?;
            return Ok(None);
        }
        let task_exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM task WHERE id = ?)")
                .bind(task_id)
                .fetch_one(&mut *tx)
                .await?;
        if !task_exists {
            tx.commit().await?;
            return Ok(None);
        }
        let revision = ensure_initial_current_revision_in_tx(&mut tx, task_id).await?;
        tx.commit().await?;
        Ok(Some(revision))
    }

    async fn create_revision(
        &self,
        input: CreateTaskRepositorySetRevision,
    ) -> Result<TaskRepositorySetRevision> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let revision = create_revision_in_tx(&mut tx, input).await?;
        tx.commit().await?;
        Ok(revision)
    }

    async fn create_revision_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        input: CreateTaskRepositorySetRevision,
    ) -> Result<TaskRepositorySetRevision> {
        create_revision_in_tx(transaction, input).await
    }

    async fn get_revision(
        &self,
        task_id: &str,
        revision: i64,
    ) -> Result<Option<TaskRepositorySetRevision>> {
        let mut tx = self.pool.begin().await?;
        let result = load_revision_in_tx(&mut tx, task_id, revision).await?;
        tx.commit().await?;
        Ok(result)
    }

    async fn pin_work_unit(&self, work_unit_id: &str, task_id: &str, revision: i64) -> Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        pin_work_unit_in_tx(&mut tx, work_unit_id, task_id, revision).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn pin_work_unit_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        work_unit_id: &str,
        task_id: &str,
        revision: i64,
    ) -> Result<()> {
        pin_work_unit_in_tx(transaction, work_unit_id, task_id, revision).await
    }

    async fn get_for_work_unit(
        &self,
        work_unit_id: &str,
        task_id: &str,
    ) -> Result<Option<TaskRepositorySetRevision>> {
        let mut tx = self.pool.begin().await?;
        let revision: Option<i64> = sqlx::query_scalar(
            "SELECT revision FROM work_unit_repository_set_revision
             WHERE work_unit_id = ? AND task_id = ?",
        )
        .bind(work_unit_id)
        .bind(task_id)
        .fetch_optional(&mut *tx)
        .await?;
        let result = if let Some(revision) = revision {
            load_revision_in_tx(&mut tx, task_id, revision).await?
        } else {
            None
        };
        tx.commit().await?;
        Ok(result)
    }
}
