//! Optional project DAG planning; cognitive proposals never grant execution authority.
use crate::{Result, ServiceError, TaskService};
use db::{
    new_uuid_v4, now_rfc3339, DomainEventRepo, ProjectCopilotRepo, ProjectCopilotRunGrant, SqliteDb,
};
use events::EventBus;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, Transaction};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};
mod types;
pub use types::*;
mod batches;
mod graph;
mod scheduler;
#[cfg(test)]
mod tests;

#[derive(Clone)]
pub struct ProjectPlanService {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
}

enum ProjectPlanEventActor<'a> {
    Human(&'a str),
    Copilot(&'a ProjectCopilotRunGrant),
}

fn invalid(message: impl Into<String>) -> ServiceError {
    ServiceError::invalid_operation(message)
}
fn json_text<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|e| invalid(e.to_string()))
}
fn decode<T: DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(value).map_err(|e| invalid(e.to_string()))
}
fn digest<T: Serialize>(value: &T) -> Result<String> {
    Ok(hex::encode(Sha256::digest(json_text(value)?.as_bytes())))
}
fn key_valid(key: &str) -> Result<()> {
    if key.trim().is_empty() || key.len() > 256 {
        return Err(invalid("idempotency key must contain 1 to 256 bytes"));
    }
    Ok(())
}

pub(crate) async fn authorize_project_write_in_tx(
    db: &SqliteDb,
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    user_id: &str,
    grant: Option<&ProjectCopilotRunGrant>,
) -> Result<()> {
    let allowed: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM project p JOIN project_member caller ON caller.project_id=p.id AND caller.user_id=? AND caller.role IN ('owner','admin','member') WHERE p.id=? AND p.owner_id IS NOT NULL AND EXISTS(SELECT 1 FROM project_member owner_membership WHERE owner_membership.project_id=p.id AND owner_membership.user_id=p.owner_id AND owner_membership.role='owner'))").bind(user_id).bind(project_id).fetch_one(&mut **tx).await?;
    if !allowed {
        return Err(ServiceError::AuthorizationDenied {
            message: "Current Project write membership and canonical owner required".into(),
        });
    }
    if let Some(grant) = grant {
        if grant.context_task_id.is_some()
            || grant.project_id != project_id
            || grant.user_id != user_id
        {
            return Err(db::DbError::ProjectCopilotRunAuthority.into());
        }
        ProjectCopilotRepo::authorize_run_in_tx(
            db,
            tx,
            grant,
            "project",
            project_id,
            "agent",
            &grant.agent_id,
        )
        .await?;
    }
    Ok(())
}

pub(crate) async fn authorize_project_admin_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    user_id: &str,
) -> Result<()> {
    let allowed: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM project p JOIN project_member caller ON caller.project_id=p.id AND caller.user_id=? AND caller.role IN ('owner','admin') WHERE p.id=? AND p.owner_id IS NOT NULL AND EXISTS(SELECT 1 FROM project_member owner_membership WHERE owner_membership.project_id=p.id AND owner_membership.user_id=p.owner_id AND owner_membership.role='owner'))").bind(user_id).bind(project_id).fetch_one(&mut **tx).await?;
    if !allowed {
        return Err(ServiceError::AuthorizationDenied {
            message: "Current Project owner/admin authority and canonical owner required".into(),
        });
    }
    Ok(())
}

impl ProjectPlanService {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self { db, event_bus }
    }
    pub(super) async fn event_with_payload(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        project_id: &str,
        user_id: &str,
        kind: &str,
        entity_id: &str,
        payload: serde_json::Value,
    ) -> Result<db::DomainEvent> {
        self.event_for_actor(
            tx,
            project_id,
            ProjectPlanEventActor::Human(user_id),
            kind,
            entity_id,
            payload,
        )
        .await
    }
    async fn event_for_actor(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        project_id: &str,
        actor: ProjectPlanEventActor<'_>,
        kind: &str,
        entity_id: &str,
        payload: serde_json::Value,
    ) -> Result<db::DomainEvent> {
        let id = new_uuid_v4();
        let (actor_type, actor_id, correlation_id) = match actor {
            ProjectPlanEventActor::Human(user_id) => {
                ("human", Some(user_id.to_owned()), id.clone())
            }
            ProjectPlanEventActor::Copilot(grant) => {
                ("agent", Some(grant.agent_id.clone()), grant.run_id.clone())
            }
        };
        Ok(DomainEventRepo::append_event_in_tx(
            &*self.db,
            tx,
            &db::CreateDomainEvent {
                id,
                event_type: kind.into(),
                entity_type: "project_plan".into(),
                entity_id: entity_id.into(),
                actor_type: actor_type.into(),
                actor_id,
                scope_type: "project".into(),
                scope_id: project_id.into(),
                correlation_id,
                causation_id: None,
                causation_depth: 0,
                dedupe_key: None,
                payload_json: payload.to_string(),
                created_at: now_rfc3339(),
            },
        )
        .await?)
    }
    pub(super) async fn system_event_with_payload(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        project_id: &str,
        kind: &str,
        entity_id: &str,
        payload: serde_json::Value,
    ) -> Result<db::DomainEvent> {
        let id = new_uuid_v4();
        Ok(DomainEventRepo::append_event_in_tx(
            &*self.db,
            tx,
            &db::CreateDomainEvent {
                id: id.clone(),
                event_type: kind.into(),
                entity_type: "project_plan".into(),
                entity_id: entity_id.into(),
                actor_type: "system".into(),
                actor_id: None,
                scope_type: "project".into(),
                scope_id: project_id.into(),
                correlation_id: id,
                causation_id: None,
                causation_depth: 0,
                dedupe_key: None,
                payload_json: payload.to_string(),
                created_at: now_rfc3339(),
            },
        )
        .await?)
    }
    fn publish(&self, event: &db::DomainEvent) {
        crate::DomainEventService::publish_committed_hint(&self.event_bus, event);
    }
    async fn version_in_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        project_id: &str,
        expected: i64,
    ) -> Result<i64> {
        let actual: i64 = sqlx::query_scalar("SELECT version FROM project_plan WHERE project_id=?")
            .bind(project_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", project_id.to_owned()))?;
        if actual != expected {
            return Err(db::DbError::VersionConflict.into());
        }
        Ok(actual)
    }
    pub async fn propose(
        &self,
        project_id: &str,
        user_id: &str,
        expected_version: i64,
        document: ProjectPlanDocument,
        idempotency_key: &str,
    ) -> Result<ProjectPlanProposal> {
        self.propose_inner(
            project_id,
            user_id,
            None,
            expected_version,
            document,
            idempotency_key,
        )
        .await
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn propose_from_copilot(
        &self,
        project_id: &str,
        user_id: &str,
        run_id: &str,
        expected_version: i64,
        document: ProjectPlanDocument,
        idempotency_key: &str,
    ) -> Result<ProjectPlanProposal> {
        let grant = ProjectCopilotRepo::authorize_run(&*self.db, run_id).await?;
        self.propose_inner(
            project_id,
            user_id,
            Some(&grant),
            expected_version,
            document,
            idempotency_key,
        )
        .await
    }
    #[allow(clippy::too_many_arguments)]
    async fn propose_inner(
        &self,
        project_id: &str,
        user_id: &str,
        grant: Option<&ProjectCopilotRunGrant>,
        expected_version: i64,
        document: ProjectPlanDocument,
        idempotency_key: &str,
    ) -> Result<ProjectPlanProposal> {
        key_valid(idempotency_key)?;
        validate_document(&document)?;
        let document_json = json_text(&document)?;
        let document_digest = digest(&document)?;
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        authorize_project_write_in_tx(&self.db, &mut tx, project_id, user_id, grant).await?;
        if let Some(row) = sqlx::query(
            "SELECT * FROM project_plan_proposal WHERE project_id=? AND idempotency_key=?",
        )
        .bind(project_id)
        .bind(idempotency_key)
        .fetch_optional(&mut *tx)
        .await?
        {
            let existing = proposal_row(row)?;
            if existing.digest != document_digest || existing.base_version != expected_version {
                return Err(db::DbError::IdempotencyConflict.into());
            }
            tx.commit().await?;
            return Ok(existing);
        }
        self.version_in_tx(&mut tx, project_id, expected_version)
            .await?;
        let current_document_json: String =
            sqlx::query_scalar("SELECT document_json FROM project_plan WHERE project_id=?")
                .bind(project_id)
                .fetch_one(&mut *tx)
                .await?;
        let current_document: ProjectPlanDocument = decode(&current_document_json)?;
        for node in &document.nodes {
            if let Some(id)=&node.task_id { ensure_task_in_tx(&mut tx,project_id,id).await?; }
            else if current_document.nodes.iter().any(|current|current.key==node.key && current.task_id.is_some())
                || sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM project_plan_proposal_task WHERE project_id=? AND node_key=?)").bind(project_id).bind(&node.key).fetch_one(&mut *tx).await? {
                return Err(invalid(format!("Plan node key {} already has Task identity; include its exact task_id or choose a new key",node.key)));
            }
        }
        let proposal = ProjectPlanProposal {
            id: new_uuid_v4(),
            project_id: project_id.into(),
            base_version: expected_version,
            document,
            digest: document_digest,
            state: "proposed".into(),
            applied_version: None,
            created_at: now_rfc3339(),
        };
        sqlx::query("INSERT INTO project_plan_proposal(id,project_id,base_version,document_json,digest,state,requested_by,idempotency_key,created_at) VALUES(?,?,?,?,?,'proposed',?,?,?)").bind(&proposal.id).bind(project_id).bind(expected_version).bind(document_json).bind(&proposal.digest).bind(user_id).bind(idempotency_key).bind(&proposal.created_at).execute(&mut *tx).await?;
        // Copilot is the event actor; retain the human requester separately in
        // the durable event payload, using the authenticated caller already
        // bound to the proposal row and authorized Copilot run.
        let event_payload = if grant.is_some() {
            json!({"project_id":project_id,"requested_by":user_id})
        } else {
            json!({"project_id":project_id})
        };
        let event = self
            .event_for_actor(
                &mut tx,
                project_id,
                grant
                    .map(ProjectPlanEventActor::Copilot)
                    .unwrap_or(ProjectPlanEventActor::Human(user_id)),
                "project.plan_proposed",
                &proposal.id,
                event_payload,
            )
            .await?;
        tx.commit().await?;
        self.publish(&event);
        Ok(proposal)
    }
    pub async fn apply(
        &self,
        project_id: &str,
        user_id: &str,
        proposal_id: &str,
        expected_version: i64,
        idempotency_key: &str,
    ) -> Result<ProjectPlanGraph> {
        self.apply_inner(
            project_id,
            user_id,
            None,
            proposal_id,
            expected_version,
            idempotency_key,
        )
        .await
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn apply_from_copilot(
        &self,
        project_id: &str,
        user_id: &str,
        run_id: &str,
        proposal_id: &str,
        expected_version: i64,
        idempotency_key: &str,
    ) -> Result<ProjectPlanGraph> {
        let grant = ProjectCopilotRepo::authorize_run(&*self.db, run_id).await?;
        self.apply_inner(
            project_id,
            user_id,
            Some(&grant),
            proposal_id,
            expected_version,
            idempotency_key,
        )
        .await
    }
    #[allow(clippy::too_many_arguments)]
    async fn apply_inner(
        &self,
        project_id: &str,
        user_id: &str,
        grant: Option<&ProjectCopilotRunGrant>,
        proposal_id: &str,
        expected_version: i64,
        idempotency_key: &str,
    ) -> Result<ProjectPlanGraph> {
        key_valid(idempotency_key)?;
        let request_digest = digest(&("apply", user_id, proposal_id, expected_version))?;
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        authorize_project_write_in_tx(&self.db, &mut tx, project_id, user_id, grant).await?;
        if let Some((stored, _)) = receipt(&mut tx, project_id, idempotency_key).await? {
            if stored != request_digest {
                return Err(db::DbError::IdempotencyConflict.into());
            }
            tx.commit().await?;
            return self.graph(project_id).await;
        }
        self.version_in_tx(&mut tx, project_id, expected_version)
            .await?;
        let row = sqlx::query("SELECT * FROM project_plan_proposal WHERE id=? AND project_id=?")
            .bind(proposal_id)
            .bind(project_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("project plan proposal", proposal_id.to_owned())
            })?;
        let proposal = proposal_row(row)?;
        if proposal.base_version != expected_version || proposal.state != "proposed" {
            return Err(db::DbError::VersionConflict.into());
        }
        tx.commit().await?;
        // Recoverable import stage: each Task and stable key commit together via the
        // ordinary Task service. No graph or batch can reference a partial import.
        let mut document = proposal.document;
        let imported_node_keys = document
            .nodes
            .iter()
            .filter(|node| node.task_id.is_none())
            .map(|node| node.key.clone())
            .collect::<BTreeSet<_>>();
        let task_service = TaskService::new(self.db.clone(), self.event_bus.clone());
        for node in &mut document.nodes {
            if node.task_id.is_none() {
                let description = if node.acceptance_criteria.is_empty() {
                    node.description.clone()
                } else {
                    Some(format!(
                        "{}\n\nAcceptance criteria:\n{}",
                        node.description.as_deref().unwrap_or(""),
                        node.acceptance_criteria
                            .iter()
                            .map(|v| format!("- {v}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    ))
                };
                let import_key = format!("proposal:{proposal_id}:node:{}", node.key);
                let task = task_service
                    .create_project_plan_task(
                        project_id,
                        user_id,
                        &import_key,
                        &node.title,
                        description,
                        grant,
                    )
                    .await?;
                node.task_id = Some(task.id);
            }
        }
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        authorize_project_write_in_tx(&self.db, &mut tx, project_id, user_id, grant).await?;
        if let Some((stored, _)) = receipt(&mut tx, project_id, idempotency_key).await? {
            if stored != request_digest {
                return Err(db::DbError::IdempotencyConflict.into());
            }
            tx.commit().await?;
            return self.graph(project_id).await;
        }
        self.version_in_tx(&mut tx, project_id, expected_version)
            .await?;
        let mut ids = BTreeMap::new();
        let mut unique = BTreeSet::new();
        for node in &document.nodes {
            let id = node
                .task_id
                .as_ref()
                .ok_or_else(|| invalid("Unmaterialized node"))?;
            ensure_task_in_tx(&mut tx, project_id, id).await?;
            if !unique.insert(id.clone()) {
                return Err(invalid("Two plan nodes reference the same Task"));
            }
            ids.insert(node.key.clone(), id.clone());
        }
        for node in &document.nodes {
            let task_id = &ids[&node.key];
            let import_key = imported_node_keys
                .contains(&node.key)
                .then(|| format!("proposal:{proposal_id}:node:{}", node.key));
            let existing: Option<String> = sqlx::query_scalar(
                "SELECT task_id FROM project_plan_proposal_task WHERE proposal_id=? AND node_key=?",
            )
            .bind(proposal_id)
            .bind(&node.key)
            .fetch_optional(&mut *tx)
            .await?;
            if existing
                .as_deref()
                .is_some_and(|existing| existing != task_id)
            {
                return Err(db::DbError::IdempotencyConflict.into());
            }
            if existing.is_none() {
                sqlx::query("INSERT INTO project_plan_proposal_task(proposal_id,project_id,node_key,task_id,import_key,created_at) VALUES(?,?,?,?,?,?)")
                    .bind(proposal_id).bind(project_id).bind(&node.key).bind(task_id).bind(import_key).bind(now_rfc3339()).execute(&mut *tx).await?;
            }
        }
        // Replace only dependencies previously owned by the project plan. Manual
        // Task edges are preserved and participate in the cycle check below.
        let desired_edges = document
            .edges
            .iter()
            .map(|edge| {
                (
                    ids[&edge.successor_key].clone(),
                    ids[&edge.predecessor_key].clone(),
                )
            })
            .collect::<BTreeSet<_>>();
        let owned = sqlx::query(
            "SELECT task_id,depends_on_id FROM project_plan_owned_dependency WHERE project_id=?",
        )
        .bind(project_id)
        .fetch_all(&mut *tx)
        .await?;
        for row in owned {
            let task_id: String = row.try_get("task_id")?;
            let depends_on_id: String = row.try_get("depends_on_id")?;
            if !desired_edges.contains(&(task_id.clone(), depends_on_id.clone())) {
                sqlx::query(
                    "DELETE FROM project_plan_owned_dependency WHERE task_id=? AND depends_on_id=?",
                )
                .bind(&task_id)
                .bind(&depends_on_id)
                .execute(&mut *tx)
                .await?;
                let manual:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM project_plan_manual_dependency WHERE task_id=? AND depends_on_id=?)").bind(&task_id).bind(&depends_on_id).fetch_one(&mut *tx).await?;
                if !manual {
                    sqlx::query("DELETE FROM task_dependency WHERE task_id=? AND depends_on_id=?")
                        .bind(&task_id)
                        .bind(&depends_on_id)
                        .execute(&mut *tx)
                        .await?;
                }
            }
        }
        for edge in &document.edges {
            let predecessor = &ids[&edge.predecessor_key];
            let successor = &ids[&edge.successor_key];
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM task_dependency WHERE task_id=? AND depends_on_id=?)",
            )
            .bind(successor)
            .bind(predecessor)
            .fetch_one(&mut *tx)
            .await?;
            if !exists {
                sqlx::query(
                    "INSERT INTO task_dependency(task_id,depends_on_id,created_at) VALUES(?,?,?)",
                )
                .bind(successor)
                .bind(predecessor)
                .bind(now_rfc3339())
                .execute(&mut *tx)
                .await
                .map_err(dependency_error)?;
                // The insert trigger marks new edges as manual for direct callers;
                // this insertion is owned by the plan instead.
                sqlx::query("DELETE FROM project_plan_manual_dependency WHERE task_id=? AND depends_on_id=?").bind(successor).bind(predecessor).execute(&mut *tx).await?;
            }
            sqlx::query("INSERT INTO project_plan_owned_dependency(project_id,task_id,depends_on_id,predecessor_key,successor_key,edge_json) VALUES(?,?,?,?,?,?) ON CONFLICT(task_id,depends_on_id) DO UPDATE SET project_id=excluded.project_id,predecessor_key=excluded.predecessor_key,successor_key=excluded.successor_key,edge_json=excluded.edge_json")
                .bind(project_id).bind(successor).bind(predecessor).bind(&edge.predecessor_key).bind(&edge.successor_key).bind(json_text(edge)?).execute(&mut *tx).await?;
        }
        sqlx::query("UPDATE project_plan SET document_json=?,version=version+1,updated_at=? WHERE project_id=?").bind(json_text(&document)?).bind(now_rfc3339()).bind(project_id).execute(&mut *tx).await?;
        let version: i64 =
            sqlx::query_scalar("SELECT version FROM project_plan WHERE project_id=?")
                .bind(project_id)
                .fetch_one(&mut *tx)
                .await?;
        sqlx::query("UPDATE project_plan_proposal SET state='applied',applied_version=? WHERE id=? AND state='proposed'").bind(version).bind(proposal_id).execute(&mut *tx).await?;
        put_receipt(
            &mut tx,
            project_id,
            idempotency_key,
            &request_digest,
            &json!({"version":version}),
        )
        .await?;
        let event = self
            .event_for_actor(
                &mut tx,
                project_id,
                grant
                    .map(ProjectPlanEventActor::Copilot)
                    .unwrap_or(ProjectPlanEventActor::Human(user_id)),
                "project.plan_applied",
                proposal_id,
                json!({"project_id":project_id,"graph_version":version}),
            )
            .await?;
        tx.commit().await?;
        self.publish(&event);
        self.graph(project_id).await
    }
    pub async fn add_dependency(
        &self,
        task_id: &str,
        depends_on_id: &str,
        user_id: &str,
    ) -> Result<()> {
        self.change_dependency(task_id, depends_on_id, user_id, true, None, None)
            .await
    }
    pub async fn remove_dependency(
        &self,
        task_id: &str,
        depends_on_id: &str,
        user_id: &str,
    ) -> Result<()> {
        self.change_dependency(task_id, depends_on_id, user_id, false, None, None)
            .await
    }
    pub async fn add_dependency_versioned(
        &self,
        task_id: &str,
        depends_on_id: &str,
        user_id: &str,
        expected_graph_version: i64,
        idempotency_key: &str,
    ) -> Result<()> {
        self.change_dependency(
            task_id,
            depends_on_id,
            user_id,
            true,
            Some(expected_graph_version),
            Some(idempotency_key),
        )
        .await
    }
    pub async fn remove_dependency_versioned(
        &self,
        task_id: &str,
        depends_on_id: &str,
        user_id: &str,
        expected_graph_version: i64,
        idempotency_key: &str,
    ) -> Result<()> {
        self.change_dependency(
            task_id,
            depends_on_id,
            user_id,
            false,
            Some(expected_graph_version),
            Some(idempotency_key),
        )
        .await
    }
    async fn change_dependency(
        &self,
        task_id: &str,
        depends_on_id: &str,
        user_id: &str,
        add: bool,
        expected_version: Option<i64>,
        idempotency_key: Option<&str>,
    ) -> Result<()> {
        if let Some(key) = idempotency_key {
            key_valid(key)?;
        }
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let project_id: String =
            sqlx::query_scalar("SELECT project_id FROM task WHERE id=? AND deleted_at IS NULL")
                .bind(task_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        authorize_project_write_in_tx(&self.db, &mut tx, &project_id, user_id, None).await?;
        ensure_task_in_tx(&mut tx, &project_id, depends_on_id).await?;
        let operation = if add {
            "add_dependency"
        } else {
            "remove_dependency"
        };
        let request_digest =
            digest(&(operation, task_id, depends_on_id, user_id, expected_version))?;
        if let Some(key) = idempotency_key {
            if let Some((stored, _)) = receipt(&mut tx, &project_id, key).await? {
                if stored != request_digest {
                    return Err(db::DbError::IdempotencyConflict.into());
                }
                tx.commit().await?;
                return Ok(());
            }
        }
        if let Some(expected) = expected_version {
            self.version_in_tx(&mut tx, &project_id, expected).await?;
        }
        let before_version: i64 =
            sqlx::query_scalar("SELECT version FROM project_plan WHERE project_id=?")
                .bind(&project_id)
                .fetch_one(&mut *tx)
                .await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM task_dependency WHERE task_id=? AND depends_on_id=?)",
        )
        .bind(task_id)
        .bind(depends_on_id)
        .fetch_one(&mut *tx)
        .await?;
        let manual_before:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM project_plan_manual_dependency WHERE task_id=? AND depends_on_id=?)").bind(task_id).bind(depends_on_id).fetch_one(&mut *tx).await?;
        let mut document_changed = false;
        if add {
            if !exists {
                sqlx::query(
                    "INSERT INTO task_dependency(task_id,depends_on_id,created_at) VALUES(?,?,?)",
                )
                .bind(task_id)
                .bind(depends_on_id)
                .bind(now_rfc3339())
                .execute(&mut *tx)
                .await
                .map_err(dependency_error)?;
            }
            sqlx::query("INSERT OR IGNORE INTO project_plan_manual_dependency(task_id,depends_on_id) VALUES(?,?)").bind(task_id).bind(depends_on_id).execute(&mut *tx).await?;
        } else {
            let plan_doc_json: String =
                sqlx::query_scalar("SELECT document_json FROM project_plan WHERE project_id=?")
                    .bind(&project_id)
                    .fetch_one(&mut *tx)
                    .await?;
            let mut document: ProjectPlanDocument = decode(&plan_doc_json)?;
            let id_by_key = document
                .nodes
                .iter()
                .filter_map(|node| {
                    node.task_id
                        .as_ref()
                        .map(|id| (node.key.clone(), id.clone()))
                })
                .collect::<BTreeMap<_, _>>();
            let before = document.edges.len();
            document.edges.retain(|edge| {
                !(id_by_key
                    .get(&edge.successor_key)
                    .is_some_and(|id| id == task_id)
                    && id_by_key
                        .get(&edge.predecessor_key)
                        .is_some_and(|id| id == depends_on_id))
            });
            document_changed = document.edges.len() != before;
            sqlx::query(
                "DELETE FROM project_plan_owned_dependency WHERE task_id=? AND depends_on_id=?",
            )
            .bind(task_id)
            .bind(depends_on_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "DELETE FROM project_plan_manual_dependency WHERE task_id=? AND depends_on_id=?",
            )
            .bind(task_id)
            .bind(depends_on_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query("DELETE FROM task_dependency WHERE task_id=? AND depends_on_id=?")
                .bind(task_id)
                .bind(depends_on_id)
                .execute(&mut *tx)
                .await?;
            if document_changed {
                sqlx::query("UPDATE project_plan SET document_json=?,version=version+1,updated_at=? WHERE project_id=?").bind(json_text(&document)?).bind(now_rfc3339()).bind(&project_id).execute(&mut *tx).await?;
            }
        }
        if add && exists && !manual_before {
            sqlx::query(
                "UPDATE project_plan SET version=version+1,updated_at=? WHERE project_id=?",
            )
            .bind(now_rfc3339())
            .bind(&project_id)
            .execute(&mut *tx)
            .await?;
        }
        let after_version: i64 =
            sqlx::query_scalar("SELECT version FROM project_plan WHERE project_id=?")
                .bind(&project_id)
                .fetch_one(&mut *tx)
                .await?;
        let mutated = if add {
            !exists || !manual_before
        } else {
            exists || document_changed
        };
        if let Some(key) = idempotency_key {
            put_receipt(
                &mut tx,
                &project_id,
                key,
                &request_digest,
                &json!({"graph_version":after_version}),
            )
            .await?;
        }
        let event = if mutated || document_changed {
            Some(
                self.event_with_payload(
                    &mut tx,
                    &project_id,
                    user_id,
                    "project.plan_dependencies_changed",
                    task_id,
                    json!({"project_id":project_id,"graph_version":after_version}),
                )
                .await?,
            )
        } else {
            None
        };
        if after_version < before_version {
            return Err(invalid("Project graph version moved backwards"));
        }
        tx.commit().await?;
        if let Some(event) = event {
            self.publish(&event);
        }
        Ok(())
    }
}
async fn ensure_task_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    task_id: &str,
) -> Result<()> {
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM task WHERE id=? AND project_id=? AND deleted_at IS NULL)",
    )
    .bind(task_id)
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await?;
    if !valid {
        return Err(ServiceError::not_found("task", task_id.to_owned()));
    }
    Ok(())
}
fn dependency_error(error: sqlx::Error) -> ServiceError {
    if let sqlx::Error::Database(db) = &error {
        match db.message() {
            "FORGE_PROJECT_DEPENDENCY_CYCLE" => return db::DbError::CycleDetected.into(),
            "FORGE_PROJECT_DEPENDENCY_INVALID" => {
                return invalid("Dependencies require distinct same-Project Tasks")
            }
            _ => (),
        }
    }
    error.into()
}
async fn receipt(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    key: &str,
) -> Result<Option<(String, String)>> {
    Ok(sqlx::query_as("SELECT request_digest,result_json FROM project_plan_receipt WHERE project_id=? AND idempotency_key=?").bind(project_id).bind(key).fetch_optional(&mut **tx).await?)
}
async fn put_receipt<T: Serialize>(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    key: &str,
    request_digest: &str,
    result: &T,
) -> Result<()> {
    sqlx::query("INSERT INTO project_plan_receipt(project_id,idempotency_key,request_digest,result_json,created_at) VALUES(?,?,?,?,?)").bind(project_id).bind(key).bind(request_digest).bind(json_text(result)?).bind(now_rfc3339()).execute(&mut **tx).await?;
    Ok(())
}
fn proposal_row(row: sqlx::sqlite::SqliteRow) -> Result<ProjectPlanProposal> {
    Ok(ProjectPlanProposal {
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        base_version: row.try_get("base_version")?,
        document: decode(row.try_get::<&str, _>("document_json")?)?,
        digest: row.try_get("digest")?,
        state: row.try_get("state")?,
        applied_version: row.try_get("applied_version")?,
        created_at: row.try_get("created_at")?,
    })
}
fn validate_document(document: &ProjectPlanDocument) -> Result<()> {
    if document.nodes.len() > 1000
        || document.edges.len() > 10000
        || json_text(document)?.len() > 2 * 1024 * 1024
    {
        return Err(invalid("Project plan exceeds bounded document limits"));
    }
    let sources = document
        .sources
        .iter()
        .map(|s| s.id.as_str())
        .collect::<BTreeSet<_>>();
    if sources.len() != document.sources.len()
        || document
            .sources
            .iter()
            .any(|s| s.id.trim().is_empty() || s.label.trim().is_empty())
    {
        return Err(invalid(
            "Sources require unique nonempty identifiers and labels",
        ));
    }
    let mut keys = BTreeSet::new();
    for node in &document.nodes {
        if node.key.trim().is_empty()
            || node.key.len() > 256
            || node.title.trim().is_empty()
            || !keys.insert(node.key.clone())
        {
            return Err(invalid("Plan nodes require unique stable keys and titles"));
        }
        if node
            .source_ids
            .iter()
            .any(|id| !sources.contains(id.as_str()))
        {
            return Err(invalid("Node references an unknown source"));
        }
        match (node.estimate_min_minutes, node.estimate_max_minutes) {
            (None, None) => (),
            (Some(a), Some(b)) if a >= 0 && b >= a => (),
            _ => {
                return Err(invalid(
                    "Estimates require an ordered nonnegative interval or no interval",
                ))
            }
        }
    }
    let mut edges = BTreeSet::new();
    let mut indegree = keys
        .iter()
        .map(|k| (k.clone(), 0_usize))
        .collect::<BTreeMap<_, _>>();
    let mut successors: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for edge in &document.edges {
        if edge.predecessor_key == edge.successor_key
            || !keys.contains(&edge.predecessor_key)
            || !keys.contains(&edge.successor_key)
            || !edges.insert((edge.predecessor_key.clone(), edge.successor_key.clone()))
        {
            return Err(invalid(
                "Edges require distinct existing keys without duplicates",
            ));
        }
        if edge
            .source_ids
            .iter()
            .any(|id| !sources.contains(id.as_str()))
        {
            return Err(invalid("Edge references an unknown source"));
        }
        *indegree
            .get_mut(&edge.successor_key)
            .ok_or_else(|| invalid("Missing successor"))? += 1;
        successors
            .entry(edge.predecessor_key.clone())
            .or_default()
            .push(edge.successor_key.clone());
    }
    let mut queue = indegree
        .iter()
        .filter(|(_, v)| **v == 0)
        .map(|(k, _)| k.clone())
        .collect::<VecDeque<_>>();
    let mut visited = 0;
    while let Some(key) = queue.pop_front() {
        visited += 1;
        for next in successors.get(&key).into_iter().flatten() {
            let degree = indegree
                .get_mut(next)
                .ok_or_else(|| invalid("Missing node"))?;
            *degree -= 1;
            if *degree == 0 {
                queue.push_back(next.clone());
            }
        }
    }
    if visited != keys.len() {
        return Err(db::DbError::CycleDetected.into());
    }
    Ok(())
}
