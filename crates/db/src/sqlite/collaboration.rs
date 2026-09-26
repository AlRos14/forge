use super::*;
use crate::{
    Artifact, CollaborationRepo, CollaborationTarget, CollaborationTargetKind, CollaborationWrite,
    CreateArtifact, CreateDecision, CreateHandoff, CreateMessage, CreateProposal, Decision,
    DecisionOutcome, Handoff, Message, Proposal, ProposalTarget, TransitionHandoff,
};

#[derive(Debug, Serialize, Deserialize)]
struct CollaborationCursor {
    created_at: String,
    id: String,
}

fn decode_collaboration_cursor(cursor: &Option<String>) -> Result<Option<CollaborationCursor>> {
    let Some(cursor) = cursor else {
        return Ok(None);
    };
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| DbError::InvalidCursor)?;
    let decoded: CollaborationCursor =
        serde_json::from_slice(&bytes).map_err(|_| DbError::InvalidCursor)?;
    if decoded.created_at.is_empty() || decoded.id.is_empty() {
        return Err(DbError::InvalidCursor);
    }
    Ok(Some(decoded))
}

fn encode_collaboration_cursor(created_at: &str, id: &str) -> Result<String> {
    let bytes = serde_json::to_vec(&CollaborationCursor {
        created_at: created_at.to_owned(),
        id: id.to_owned(),
    })
    .map_err(|_| DbError::InvalidCursor)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn actor_from_columns(kind: String, id: String, exists: i64) -> Result<ActorRef> {
    if exists != 1 {
        return Err(DbError::Check(
            "collaboration record contains a dangling ActorRef".to_owned(),
        ));
    }
    let kind = parse_enum::<ActorKind>(kind)?;
    if id.trim().is_empty() {
        return Err(DbError::Check(
            "collaboration record contains an invalid ActorRef".to_owned(),
        ));
    }
    Ok(match kind {
        ActorKind::Human => ActorRef::Human(id),
        ActorKind::Agent => ActorRef::Agent(id),
    })
}

fn target_from_columns(
    kind: String,
    actor_kind: Option<String>,
    actor_id: Option<String>,
    role_id: Option<String>,
    actor_exists: i64,
    role_exists: i64,
) -> Result<CollaborationTarget> {
    match parse_enum::<CollaborationTargetKind>(kind)? {
        CollaborationTargetKind::Actor => Ok(CollaborationTarget::Actor(actor_from_columns(
            actor_kind.ok_or_else(|| DbError::Check("actor target kind is missing".to_owned()))?,
            actor_id.ok_or_else(|| DbError::Check("actor target id is missing".to_owned()))?,
            actor_exists,
        )?)),
        CollaborationTargetKind::Role => {
            if role_exists != 1 {
                return Err(DbError::Check(
                    "collaboration role target is missing or cross-Task".to_owned(),
                ));
            }
            Ok(CollaborationTarget::Role(role_id.ok_or_else(|| {
                DbError::Check("role target id is missing".to_owned())
            })?))
        }
        CollaborationTargetKind::Task => Ok(CollaborationTarget::Task),
    }
}

fn target_columns(
    target: &CollaborationTarget,
) -> (&'static str, Option<String>, Option<String>, Option<String>) {
    match target {
        CollaborationTarget::Actor(actor) => (
            "actor",
            Some(actor.kind().to_string()),
            Some(actor.id().to_owned()),
            None,
        ),
        CollaborationTarget::Role(role_id) => ("role", None, None, Some(role_id.clone())),
        CollaborationTarget::Task => ("task", None, None, None),
    }
}

fn artifact_select() -> &'static str {
    "SELECT a.*,
            p.execution_id AS producer_execution_id,
            p.task_id AS producer_task_id,
            e.task_id AS execution_task_id,
            e.actor_kind AS producer_actor_kind,
            e.actor_id AS producer_actor_id,
            CASE
                WHEN e.actor_kind = 'human' AND EXISTS (
                    SELECT 1 FROM user u WHERE u.id = e.actor_id
                ) THEN 1
                WHEN e.actor_kind = 'agent' AND EXISTS (
                    SELECT 1 FROM agent_identity ai WHERE ai.id = e.actor_id
                ) THEN 1
                ELSE 0
            END AS producer_actor_exists
     FROM artifact a
     LEFT JOIN artifact_execution_producer p ON p.artifact_id = a.id
     LEFT JOIN execution e ON e.id = p.execution_id"
}

fn map_artifact(row: SqliteRow) -> Result<Artifact> {
    let task_id: String = row.try_get("task_id")?;
    let producer_execution_id: Option<String> = row.try_get("producer_execution_id")?;
    let producer_task_id: Option<String> = row.try_get("producer_task_id")?;
    let execution_task_id: Option<String> = row.try_get("execution_task_id")?;
    let actor_kind: Option<String> = row.try_get("producer_actor_kind")?;
    let actor_id: Option<String> = row.try_get("producer_actor_id")?;
    let actor_exists: i64 = row.try_get("producer_actor_exists")?;
    if producer_task_id.as_deref() != Some(task_id.as_str())
        || execution_task_id.as_deref() != Some(task_id.as_str())
    {
        return Err(DbError::Check(
            "Artifact has no valid same-Task Execution producer".to_owned(),
        ));
    }
    let producer = actor_from_columns(
        actor_kind
            .ok_or_else(|| DbError::Check("Artifact producer ActorRef is missing".to_owned()))?,
        actor_id
            .ok_or_else(|| DbError::Check("Artifact producer actor id is missing".to_owned()))?,
        actor_exists,
    )?;
    Ok(Artifact {
        id: row.try_get("id")?,
        task_id,
        kind: parse_enum(row.try_get("kind")?)?,
        storage_kind: parse_enum(row.try_get("storage_kind")?)?,
        content: row.try_get("content")?,
        content_ref: row.try_get("content_ref")?,
        metadata_json: row.try_get("metadata_json")?,
        digest: row.try_get("digest")?,
        producer_execution_id: producer_execution_id
            .ok_or_else(|| DbError::Check("Artifact producer row is missing".to_owned()))?,
        producer,
        created_at: row.try_get("created_at")?,
    })
}

fn message_select() -> &'static str {
    "SELECT m.*,
            CASE
                WHEN m.sender_actor_kind = 'human' AND EXISTS (
                    SELECT 1 FROM user u WHERE u.id = m.sender_actor_id
                ) THEN 1
                WHEN m.sender_actor_kind = 'agent' AND EXISTS (
                    SELECT 1 FROM agent_identity ai WHERE ai.id = m.sender_actor_id
                ) THEN 1
                ELSE 0
            END AS sender_actor_exists,
            CASE
                WHEN m.target_actor_kind = 'human' AND EXISTS (
                    SELECT 1 FROM user u WHERE u.id = m.target_actor_id
                ) THEN 1
                WHEN m.target_actor_kind = 'agent' AND EXISTS (
                    SELECT 1 FROM agent_identity ai WHERE ai.id = m.target_actor_id
                ) THEN 1
                ELSE 0
            END AS target_actor_exists,
            CASE WHEN EXISTS (
                SELECT 1 FROM task_role tr
                WHERE tr.id = m.target_role_id AND tr.task_id = m.task_id
            ) THEN 1 ELSE 0 END AS target_role_exists
     FROM message m"
}

async fn message_artifact_ids(
    db: &SqliteDb,
    message_id: &str,
    task_id: &str,
) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT artifact_id FROM message_artifact
         WHERE message_id = ? AND task_id = ?
         ORDER BY artifact_id ASC",
    )
    .bind(message_id)
    .bind(task_id)
    .fetch_all(db.pool())
    .await?)
}

async fn map_message(db: &SqliteDb, row: SqliteRow) -> Result<Message> {
    let task_id: String = row.try_get("task_id")?;
    let id: String = row.try_get("id")?;
    let sender = actor_from_columns(
        row.try_get("sender_actor_kind")?,
        row.try_get("sender_actor_id")?,
        row.try_get("sender_actor_exists")?,
    )?;
    let target = target_from_columns(
        row.try_get("target_kind")?,
        row.try_get("target_actor_kind")?,
        row.try_get("target_actor_id")?,
        row.try_get("target_role_id")?,
        row.try_get("target_actor_exists")?,
        row.try_get("target_role_exists")?,
    )?;
    let artifact_ids = message_artifact_ids(db, &id, &task_id).await?;
    Ok(Message {
        id,
        task_id,
        sender,
        target,
        body: row.try_get("body")?,
        artifact_ids,
        created_at: row.try_get("created_at")?,
    })
}

fn handoff_select() -> &'static str {
    "SELECT h.*,
            CASE
                WHEN h.created_by_actor_kind = 'human' AND EXISTS (
                    SELECT 1 FROM user u WHERE u.id = h.created_by_actor_id
                ) THEN 1
                WHEN h.created_by_actor_kind = 'agent' AND EXISTS (
                    SELECT 1 FROM agent_identity ai WHERE ai.id = h.created_by_actor_id
                ) THEN 1
                ELSE 0
            END AS creator_actor_exists,
            CASE
                WHEN h.target_actor_kind = 'human' AND EXISTS (
                    SELECT 1 FROM user u WHERE u.id = h.target_actor_id
                ) THEN 1
                WHEN h.target_actor_kind = 'agent' AND EXISTS (
                    SELECT 1 FROM agent_identity ai WHERE ai.id = h.target_actor_id
                ) THEN 1
                ELSE 0
            END AS target_actor_exists,
            CASE WHEN EXISTS (
                SELECT 1 FROM task_role tr
                WHERE tr.id = h.target_role_id AND tr.task_id = h.task_id
            ) THEN 1 ELSE 0 END AS target_role_exists,
            CASE WHEN h.source_role_id IS NULL OR EXISTS (
                SELECT 1 FROM task_role tr
                WHERE tr.id = h.source_role_id AND tr.task_id = h.task_id
            ) THEN 1 ELSE 0 END AS source_role_exists,
            CASE WHEN h.parent_execution_id IS NULL OR EXISTS (
                SELECT 1 FROM execution e
                WHERE e.id = h.parent_execution_id AND e.task_id = h.task_id
            ) THEN 1 ELSE 0 END AS parent_execution_exists
     FROM handoff h"
}

async fn handoff_artifact_ids(
    db: &SqliteDb,
    handoff_id: &str,
    task_id: &str,
) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT artifact_id FROM handoff_artifact
         WHERE handoff_id = ? AND task_id = ?
         ORDER BY artifact_id ASC",
    )
    .bind(handoff_id)
    .bind(task_id)
    .fetch_all(db.pool())
    .await?)
}

async fn map_handoff(db: &SqliteDb, row: SqliteRow) -> Result<Handoff> {
    let task_id: String = row.try_get("task_id")?;
    let id: String = row.try_get("id")?;
    let source_role_exists: i64 = row.try_get("source_role_exists")?;
    let parent_execution_exists: i64 = row.try_get("parent_execution_exists")?;
    if source_role_exists != 1 || parent_execution_exists != 1 {
        return Err(DbError::Check(
            "Handoff role or Execution reference is missing or cross-Task".to_owned(),
        ));
    }
    let created_by = actor_from_columns(
        row.try_get("created_by_actor_kind")?,
        row.try_get("created_by_actor_id")?,
        row.try_get("creator_actor_exists")?,
    )?;
    let target = target_from_columns(
        row.try_get("target_kind")?,
        row.try_get("target_actor_kind")?,
        row.try_get("target_actor_id")?,
        row.try_get("target_role_id")?,
        row.try_get("target_actor_exists")?,
        row.try_get("target_role_exists")?,
    )?;
    let artifact_ids = handoff_artifact_ids(db, &id, &task_id).await?;
    Ok(Handoff {
        id,
        task_id,
        created_by,
        source_role_id: row.try_get("source_role_id")?,
        target,
        intent: parse_enum(row.try_get("intent")?)?,
        parent_execution_id: row.try_get("parent_execution_id")?,
        expected_policy_ref: row.try_get("expected_policy_ref")?,
        status: parse_enum(row.try_get("status")?)?,
        version: row.try_get("version")?,
        artifact_ids,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn proposal_select() -> &'static str {
    "SELECT p.*,
            CASE
                WHEN p.proposer_actor_kind = 'human' AND EXISTS (
                    SELECT 1 FROM user u WHERE u.id = p.proposer_actor_id
                ) THEN 1
                WHEN p.proposer_actor_kind = 'agent' AND EXISTS (
                    SELECT 1 FROM agent_identity ai WHERE ai.id = p.proposer_actor_id
                ) THEN 1
                ELSE 0
            END AS proposer_actor_exists,
            CASE p.target_kind
                WHEN 'task' THEN CASE WHEN p.target_id = p.task_id THEN 1 ELSE 0 END
                WHEN 'execution' THEN CASE WHEN EXISTS (
                    SELECT 1 FROM execution e
                    WHERE e.id = p.target_id AND e.task_id = p.task_id
                ) THEN 1 ELSE 0 END
                WHEN 'workspace' THEN CASE WHEN EXISTS (
                    SELECT 1 FROM workspace w
                    WHERE w.id = p.target_id AND w.task_id = p.task_id
                ) THEN 1 ELSE 0 END
                ELSE 0
            END AS target_exists,
            CASE WHEN p.supersedes_proposal_id IS NULL OR EXISTS (
                SELECT 1 FROM proposal prior
                WHERE prior.id = p.supersedes_proposal_id
                  AND prior.task_id = p.task_id
                  AND prior.status = 'superseded'
            ) THEN 1 ELSE 0 END AS supersedes_exists
     FROM proposal p"
}

async fn proposal_artifact_ids(
    db: &SqliteDb,
    proposal_id: &str,
    task_id: &str,
) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT artifact_id FROM proposal_artifact
         WHERE proposal_id = ? AND task_id = ?
         ORDER BY artifact_id ASC",
    )
    .bind(proposal_id)
    .bind(task_id)
    .fetch_all(db.pool())
    .await?)
}

async fn map_proposal(db: &SqliteDb, row: SqliteRow) -> Result<Proposal> {
    let task_id: String = row.try_get("task_id")?;
    let id: String = row.try_get("id")?;
    if row.try_get::<i64, _>("target_exists")? != 1
        || row.try_get::<i64, _>("supersedes_exists")? != 1
    {
        return Err(DbError::Check(
            "Proposal target or supersedes reference is missing or cross-Task".to_owned(),
        ));
    }
    let proposer = actor_from_columns(
        row.try_get("proposer_actor_kind")?,
        row.try_get("proposer_actor_id")?,
        row.try_get("proposer_actor_exists")?,
    )?;
    let target = ProposalTarget {
        kind: parse_enum(row.try_get("target_kind")?)?,
        id: row.try_get("target_id")?,
    };
    let artifact_ids = proposal_artifact_ids(db, &id, &task_id).await?;
    Ok(Proposal {
        id,
        task_id,
        proposer,
        target,
        action: row.try_get("action")?,
        reason: row.try_get("reason")?,
        target_version: row.try_get("target_version")?,
        target_digest: row.try_get("target_digest")?,
        required_policy_ref: row.try_get("required_policy_ref")?,
        required_policy_version: row.try_get("required_policy_version")?,
        required_policy_digest: row.try_get("required_policy_digest")?,
        content_version: row.try_get("content_version")?,
        status: parse_enum(row.try_get("status")?)?,
        supersedes_proposal_id: row.try_get("supersedes_proposal_id")?,
        artifact_ids,
        created_at: row.try_get("created_at")?,
    })
}

async fn decision_actors(db: &SqliteDb, decision_id: &str, task_id: &str) -> Result<Vec<ActorRef>> {
    let rows = sqlx::query(
        "SELECT da.actor_kind, da.actor_id, da.task_id,
                CASE
                    WHEN da.actor_kind = 'human' AND EXISTS (
                        SELECT 1 FROM user u WHERE u.id = da.actor_id
                    ) THEN 1
                    WHEN da.actor_kind = 'agent' AND EXISTS (
                        SELECT 1 FROM agent_identity ai WHERE ai.id = da.actor_id
                    ) THEN 1
                    ELSE 0
                END AS actor_exists
         FROM decision_actor da
         WHERE da.decision_id = ? AND da.task_id = ?
         ORDER BY da.actor_kind ASC, da.actor_id ASC",
    )
    .bind(decision_id)
    .bind(task_id)
    .fetch_all(db.pool())
    .await?;
    if rows.is_empty() {
        return Err(DbError::Check(
            "Decision has no decider ActorRef".to_owned(),
        ));
    }
    rows.into_iter()
        .map(|row| {
            let actor_task_id: String = row.try_get("task_id")?;
            if actor_task_id != task_id {
                return Err(DbError::Check(
                    "Decision actor is attached to a different Task".to_owned(),
                ));
            }
            actor_from_columns(
                row.try_get("actor_kind")?,
                row.try_get("actor_id")?,
                row.try_get("actor_exists")?,
            )
        })
        .collect()
}

fn decision_select() -> &'static str {
    "SELECT d.*,
            p.task_id AS proposal_task_id,
            p.content_version AS current_proposal_version
     FROM decision d
     LEFT JOIN proposal p ON p.id = d.proposal_id"
}

async fn map_decision(db: &SqliteDb, row: SqliteRow) -> Result<Decision> {
    let task_id: String = row.try_get("task_id")?;
    let id: String = row.try_get("id")?;
    if row
        .try_get::<Option<String>, _>("proposal_task_id")?
        .as_deref()
        != Some(task_id.as_str())
        || row.try_get::<Option<i64>, _>("current_proposal_version")?
            != Some(row.try_get("proposal_version")?)
    {
        return Err(DbError::Check(
            "Decision Proposal reference is missing, cross-Task, or stale".to_owned(),
        ));
    }
    let actors = decision_actors(db, &id, &task_id).await?;
    Ok(Decision {
        id,
        task_id,
        proposal_id: row.try_get("proposal_id")?,
        proposal_version: row.try_get("proposal_version")?,
        outcome: parse_enum(row.try_get("outcome")?)?,
        rationale: row.try_get("rationale")?,
        policy_ref: row.try_get("policy_ref")?,
        policy_version: row.try_get("policy_version")?,
        policy_digest: row.try_get("policy_digest")?,
        actors,
        created_at: row.try_get("created_at")?,
    })
}

async fn get_artifact_row(db: &SqliteDb, id: &str) -> Result<Option<Artifact>> {
    let sql = format!("{} WHERE a.id = ?", artifact_select());
    sqlx::query(&sql)
        .bind(id)
        .fetch_optional(db.pool())
        .await?
        .map(map_artifact)
        .transpose()
}

async fn get_message_row(db: &SqliteDb, id: &str) -> Result<Option<Message>> {
    let sql = format!("{} WHERE m.id = ?", message_select());
    match sqlx::query(&sql).bind(id).fetch_optional(db.pool()).await? {
        Some(row) => map_message(db, row).await.map(Some),
        None => Ok(None),
    }
}

async fn get_handoff_row(db: &SqliteDb, id: &str) -> Result<Option<Handoff>> {
    let sql = format!("{} WHERE h.id = ?", handoff_select());
    match sqlx::query(&sql).bind(id).fetch_optional(db.pool()).await? {
        Some(row) => map_handoff(db, row).await.map(Some),
        None => Ok(None),
    }
}

async fn get_proposal_row(db: &SqliteDb, id: &str) -> Result<Option<Proposal>> {
    let sql = format!("{} WHERE p.id = ?", proposal_select());
    match sqlx::query(&sql).bind(id).fetch_optional(db.pool()).await? {
        Some(row) => map_proposal(db, row).await.map(Some),
        None => Ok(None),
    }
}

async fn get_decision_row(db: &SqliteDb, id: &str) -> Result<Option<Decision>> {
    let sql = format!("{} WHERE d.id = ?", decision_select());
    match sqlx::query(&sql).bind(id).fetch_optional(db.pool()).await? {
        Some(row) => map_decision(db, row).await.map(Some),
        None => Ok(None),
    }
}

fn finish_page<T>(
    mut items: Vec<T>,
    page: &PageRequest,
    cursor_for: impl Fn(&T) -> (&str, &str),
    total_count: Option<i64>,
) -> Result<Page<T>> {
    let page_size = page.limit.clamp(1, 100) as usize;
    let has_more = items.len() > page_size;
    if has_more {
        items.truncate(page_size);
    }
    let next_cursor = if has_more {
        let (created_at, id) = cursor_for(items.last().ok_or(DbError::InvalidCursor)?);
        Some(encode_collaboration_cursor(created_at, id)?)
    } else {
        None
    };
    Ok(Page {
        items,
        next_cursor,
        total_count,
    })
}

async fn total_for_task(db: &SqliteDb, table: &str, task_id: &str) -> Result<i64> {
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE task_id = ?");
    Ok(sqlx::query_scalar(&sql)
        .bind(task_id)
        .fetch_one(db.pool())
        .await?)
}

#[async_trait]
impl CollaborationRepo for SqliteDb {
    async fn create_artifact(
        &self,
        input: CreateArtifact,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<Artifact>> {
        let mut tx = self.pool.begin().await?;
        // The producer row is inserted first under a deferred FK. The Artifact
        // trigger refuses a row without this one same-Task Execution producer.
        sqlx::query(
            "INSERT INTO artifact_execution_producer (artifact_id, execution_id, task_id)
             VALUES (?, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.producer_execution_id)
        .bind(&input.task_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO artifact (
                id, task_id, kind, storage_kind, content, content_ref,
                metadata_json, digest, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(input.kind.to_string())
        .bind(input.storage_kind.to_string())
        .bind(input.content)
        .bind(input.content_ref)
        .bind(input.metadata_json)
        .bind(input.digest)
        .bind(input.created_at)
        .execute(&mut *tx)
        .await?;
        let event = DomainEventRepo::append_event_in_tx(self, &mut tx, &event).await?;
        tx.commit().await?;
        let record = get_artifact_row(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        Ok(CollaborationWrite { record, event })
    }

    async fn get_artifact(&self, id: &str) -> Result<Option<Artifact>> {
        get_artifact_row(self, id).await
    }

    async fn list_artifacts(&self, task_id: &str, page: PageRequest) -> Result<Page<Artifact>> {
        let cursor = decode_collaboration_cursor(&page.cursor)?;
        let mut query = format!("{} WHERE a.task_id = ?", artifact_select());
        if cursor.is_some() {
            query.push_str(" AND (a.created_at < ? OR (a.created_at = ? AND a.id < ?))");
        }
        query.push_str(" ORDER BY a.created_at DESC, a.id DESC LIMIT ?");
        let mut statement = sqlx::query(&query).bind(task_id);
        if let Some(cursor) = cursor {
            statement = statement
                .bind(cursor.created_at.clone())
                .bind(cursor.created_at)
                .bind(cursor.id);
        }
        let rows = statement
            .bind(page.limit.clamp(1, 100) + 1)
            .fetch_all(self.pool())
            .await?;
        let items = rows
            .into_iter()
            .map(map_artifact)
            .collect::<Result<Vec<_>>>()?;
        let total = if page.include_total {
            Some(total_for_task(self, "artifact", task_id).await?)
        } else {
            None
        };
        finish_page(items, &page, |item| (&item.created_at, &item.id), total)
    }

    async fn create_message(
        &self,
        input: CreateMessage,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<Message>> {
        let mut tx = self.pool.begin().await?;
        for artifact_id in &input.artifact_ids {
            sqlx::query(
                "INSERT INTO message_artifact (message_id, artifact_id, task_id)
                 VALUES (?, ?, ?)",
            )
            .bind(&input.id)
            .bind(artifact_id)
            .bind(&input.task_id)
            .execute(&mut *tx)
            .await?;
        }
        let (target_kind, target_actor_kind, target_actor_id, target_role_id) =
            target_columns(&input.target);
        sqlx::query(
            "INSERT INTO message (
                id, task_id, sender_actor_kind, sender_actor_id, target_kind,
                target_actor_kind, target_actor_id, target_role_id, body, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(input.sender.kind().to_string())
        .bind(input.sender.id())
        .bind(target_kind)
        .bind(target_actor_kind)
        .bind(target_actor_id)
        .bind(target_role_id)
        .bind(input.body)
        .bind(input.created_at)
        .execute(&mut *tx)
        .await?;
        let event = DomainEventRepo::append_event_in_tx(self, &mut tx, &event).await?;
        tx.commit().await?;
        let record = get_message_row(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        Ok(CollaborationWrite { record, event })
    }

    async fn get_message(&self, id: &str) -> Result<Option<Message>> {
        get_message_row(self, id).await
    }

    async fn list_messages(&self, task_id: &str, page: PageRequest) -> Result<Page<Message>> {
        let cursor = decode_collaboration_cursor(&page.cursor)?;
        let mut query = format!("{} WHERE m.task_id = ?", message_select());
        if cursor.is_some() {
            query.push_str(" AND (m.created_at < ? OR (m.created_at = ? AND m.id < ?))");
        }
        query.push_str(" ORDER BY m.created_at DESC, m.id DESC LIMIT ?");
        let mut statement = sqlx::query(&query).bind(task_id);
        if let Some(cursor) = cursor {
            statement = statement
                .bind(cursor.created_at.clone())
                .bind(cursor.created_at)
                .bind(cursor.id);
        }
        let rows = statement
            .bind(page.limit.clamp(1, 100) + 1)
            .fetch_all(self.pool())
            .await?;
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(map_message(self, row).await?);
        }
        let total = if page.include_total {
            Some(total_for_task(self, "message", task_id).await?)
        } else {
            None
        };
        finish_page(items, &page, |item| (&item.created_at, &item.id), total)
    }

    async fn create_handoff(
        &self,
        input: CreateHandoff,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<Handoff>> {
        let mut tx = self.pool.begin().await?;
        for artifact_id in &input.artifact_ids {
            sqlx::query(
                "INSERT INTO handoff_artifact (handoff_id, artifact_id, task_id)
                 VALUES (?, ?, ?)",
            )
            .bind(&input.id)
            .bind(artifact_id)
            .bind(&input.task_id)
            .execute(&mut *tx)
            .await?;
        }
        let (target_kind, target_actor_kind, target_actor_id, target_role_id) =
            target_columns(&input.target);
        sqlx::query(
            "INSERT INTO handoff (
                id, task_id, created_by_actor_kind, created_by_actor_id, source_role_id,
                target_kind, target_actor_kind, target_actor_id, target_role_id, intent,
                parent_execution_id, expected_policy_ref, status, version, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', 1, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(input.created_by.kind().to_string())
        .bind(input.created_by.id())
        .bind(input.source_role_id)
        .bind(target_kind)
        .bind(target_actor_kind)
        .bind(target_actor_id)
        .bind(target_role_id)
        .bind(input.intent.to_string())
        .bind(input.parent_execution_id)
        .bind(input.expected_policy_ref)
        .bind(&input.created_at)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await?;
        let event = DomainEventRepo::append_event_in_tx(self, &mut tx, &event).await?;
        tx.commit().await?;
        let record = get_handoff_row(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        Ok(CollaborationWrite { record, event })
    }

    async fn get_handoff(&self, id: &str) -> Result<Option<Handoff>> {
        get_handoff_row(self, id).await
    }

    async fn list_handoffs(&self, task_id: &str, page: PageRequest) -> Result<Page<Handoff>> {
        let cursor = decode_collaboration_cursor(&page.cursor)?;
        let mut query = format!("{} WHERE h.task_id = ?", handoff_select());
        if cursor.is_some() {
            query.push_str(" AND (h.created_at < ? OR (h.created_at = ? AND h.id < ?))");
        }
        query.push_str(" ORDER BY h.created_at DESC, h.id DESC LIMIT ?");
        let mut statement = sqlx::query(&query).bind(task_id);
        if let Some(cursor) = cursor {
            statement = statement
                .bind(cursor.created_at.clone())
                .bind(cursor.created_at)
                .bind(cursor.id);
        }
        let rows = statement
            .bind(page.limit.clamp(1, 100) + 1)
            .fetch_all(self.pool())
            .await?;
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(map_handoff(self, row).await?);
        }
        let total = if page.include_total {
            Some(total_for_task(self, "handoff", task_id).await?)
        } else {
            None
        };
        finish_page(items, &page, |item| (&item.created_at, &item.id), total)
    }

    async fn transition_handoff(
        &self,
        input: TransitionHandoff,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<Handoff>> {
        let mut tx = self.pool.begin().await?;
        let update = sqlx::query(
            "UPDATE handoff
             SET status = ?, version = version + 1, updated_at = ?
             WHERE id = ? AND version = ?",
        )
        .bind(input.status.to_string())
        .bind(input.updated_at)
        .bind(&input.id)
        .bind(input.expected_version)
        .execute(&mut *tx)
        .await?;
        if update.rows_affected() == 0 {
            let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM handoff WHERE id = ?")
                .bind(&input.id)
                .fetch_optional(&mut *tx)
                .await?;
            return Err(if exists.is_some() {
                DbError::VersionConflict
            } else {
                DbError::NotFound
            });
        }
        let event = DomainEventRepo::append_event_in_tx(self, &mut tx, &event).await?;
        tx.commit().await?;
        let record = get_handoff_row(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        Ok(CollaborationWrite { record, event })
    }

    async fn create_proposal(
        &self,
        input: CreateProposal,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<Proposal>> {
        let mut tx = self.pool.begin().await?;
        for artifact_id in &input.artifact_ids {
            sqlx::query(
                "INSERT INTO proposal_artifact (proposal_id, artifact_id, task_id)
                 VALUES (?, ?, ?)",
            )
            .bind(&input.id)
            .bind(artifact_id)
            .bind(&input.task_id)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO proposal (
                id, task_id, proposer_actor_kind, proposer_actor_id, target_kind, target_id,
                action, reason, target_version, target_digest, required_policy_ref,
                required_policy_version, required_policy_digest, content_version, status,
                supersedes_proposal_id, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, 'open', ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(input.proposer.kind().to_string())
        .bind(input.proposer.id())
        .bind(input.target.kind.to_string())
        .bind(input.target.id)
        .bind(input.action)
        .bind(input.reason)
        .bind(input.target_version)
        .bind(input.target_digest)
        .bind(input.required_policy_ref)
        .bind(input.required_policy_version)
        .bind(input.required_policy_digest)
        .bind(input.supersedes_proposal_id)
        .bind(input.created_at)
        .execute(&mut *tx)
        .await?;
        let event = DomainEventRepo::append_event_in_tx(self, &mut tx, &event).await?;
        tx.commit().await?;
        let record = get_proposal_row(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        Ok(CollaborationWrite { record, event })
    }

    async fn get_proposal(&self, id: &str) -> Result<Option<Proposal>> {
        get_proposal_row(self, id).await
    }

    async fn list_proposals(&self, task_id: &str, page: PageRequest) -> Result<Page<Proposal>> {
        let cursor = decode_collaboration_cursor(&page.cursor)?;
        let mut query = format!("{} WHERE p.task_id = ?", proposal_select());
        if cursor.is_some() {
            query.push_str(" AND (p.created_at < ? OR (p.created_at = ? AND p.id < ?))");
        }
        query.push_str(" ORDER BY p.created_at DESC, p.id DESC LIMIT ?");
        let mut statement = sqlx::query(&query).bind(task_id);
        if let Some(cursor) = cursor {
            statement = statement
                .bind(cursor.created_at.clone())
                .bind(cursor.created_at)
                .bind(cursor.id);
        }
        let rows = statement
            .bind(page.limit.clamp(1, 100) + 1)
            .fetch_all(self.pool())
            .await?;
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(map_proposal(self, row).await?);
        }
        let total = if page.include_total {
            Some(total_for_task(self, "proposal", task_id).await?)
        } else {
            None
        };
        finish_page(items, &page, |item| (&item.created_at, &item.id), total)
    }

    async fn withdraw_proposal(
        &self,
        id: &str,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<Proposal>> {
        let mut tx = self.pool.begin().await?;
        let update = sqlx::query(
            "UPDATE proposal SET status = 'withdrawn'
             WHERE id = ? AND status = 'open'",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        if update.rows_affected() == 0 {
            let status: Option<String> =
                sqlx::query_scalar("SELECT status FROM proposal WHERE id = ?")
                    .bind(id)
                    .fetch_optional(&mut *tx)
                    .await?;
            return Err(match status {
                None => DbError::NotFound,
                Some(_) => DbError::Check("only an open Proposal can be withdrawn".to_owned()),
            });
        }
        let event = DomainEventRepo::append_event_in_tx(self, &mut tx, &event).await?;
        tx.commit().await?;
        let record = get_proposal_row(self, id).await?.ok_or(DbError::NotFound)?;
        Ok(CollaborationWrite { record, event })
    }

    async fn create_decision(
        &self,
        input: CreateDecision,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<Decision>> {
        if input.actors.is_empty() {
            return Err(DbError::Check(
                "Decision requires at least one decider".to_owned(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        for actor in &input.actors {
            sqlx::query(
                "INSERT INTO decision_actor (decision_id, actor_kind, actor_id, task_id)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(&input.id)
            .bind(actor.kind().to_string())
            .bind(actor.id())
            .bind(&input.task_id)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO decision (
                id, task_id, proposal_id, proposal_version, outcome, rationale,
                policy_ref, policy_version, policy_digest, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(&input.proposal_id)
        .bind(input.proposal_version)
        .bind(input.outcome.to_string())
        .bind(input.rationale)
        .bind(input.policy_ref)
        .bind(input.policy_version)
        .bind(input.policy_digest)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await?;
        let proposal_status = match input.outcome {
            DecisionOutcome::Approve | DecisionOutcome::Reject => "resolved",
            DecisionOutcome::Supersede => "superseded",
        };
        let update = sqlx::query(
            "UPDATE proposal SET status = ?
             WHERE id = ? AND task_id = ? AND content_version = ? AND status = 'open'",
        )
        .bind(proposal_status)
        .bind(&input.proposal_id)
        .bind(&input.task_id)
        .bind(input.proposal_version)
        .execute(&mut *tx)
        .await?;
        if update.rows_affected() == 0 {
            return Err(DbError::Check(
                "Decision Proposal is missing, stale, cross-Task, or already resolved".to_owned(),
            ));
        }
        let event = DomainEventRepo::append_event_in_tx(self, &mut tx, &event).await?;
        tx.commit().await?;
        let record = get_decision_row(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        Ok(CollaborationWrite { record, event })
    }

    async fn get_decision(&self, id: &str) -> Result<Option<Decision>> {
        get_decision_row(self, id).await
    }

    async fn list_decisions(&self, task_id: &str, page: PageRequest) -> Result<Page<Decision>> {
        let cursor = decode_collaboration_cursor(&page.cursor)?;
        let mut query = format!("{} WHERE d.task_id = ?", decision_select());
        if cursor.is_some() {
            query.push_str(" AND (d.created_at < ? OR (d.created_at = ? AND d.id < ?))");
        }
        query.push_str(" ORDER BY d.created_at DESC, d.id DESC LIMIT ?");
        let mut statement = sqlx::query(&query).bind(task_id);
        if let Some(cursor) = cursor {
            statement = statement
                .bind(cursor.created_at.clone())
                .bind(cursor.created_at)
                .bind(cursor.id);
        }
        let rows = statement
            .bind(page.limit.clamp(1, 100) + 1)
            .fetch_all(self.pool())
            .await?;
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(map_decision(self, row).await?);
        }
        let total = if page.include_total {
            Some(total_for_task(self, "decision", task_id).await?)
        } else {
            None
        };
        finish_page(items, &page, |item| (&item.created_at, &item.id), total)
    }
}
