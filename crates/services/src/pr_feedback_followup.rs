//! Follow-up Task for an approved `propose_task` PR-feedback disposition
//! (plan: coordinator-verification-and-pr-loop, «Congelado 11» §11.8, node P2).
//!
//! The Task is created through the existing Project plan services: a plan
//! proposal (current document + one new node + one edge `current Task -> new
//! Task`) is proposed and applied with deterministic idempotency keys, so the
//! new Task depends on the current one and cannot start before it is `Done`.
//! The current PR is never touched. `reject` records nothing and creates
//! nothing. Authority is the human answer already enforced by
//! `HumanDecisionService::answer` (`authority_effect = scope` requires a
//! Project owner/admin); Copilot has no entry point here.

use std::sync::Arc;

use db::{HumanDecisionRepo, HumanDecisionSourceKind, HumanDecisionState, SqliteDb};
use events::EventBus;
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::coordinator_outputs::{
    bind_decision_to_disposition, FollowUpProposal, PROPOSAL_OPTION_APPROVE, PROPOSAL_OPTION_REJECT,
};
use crate::project_plan::{
    ProjectPlanDocument, ProjectPlanEdge, ProjectPlanNode, ProjectPlanService,
};
use crate::{Result, ServiceError};

pub const CODE_FOLLOWUP_NOT_APPROVED: &str = "pr_feedback.followup_not_approved";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowUpOutcome {
    /// The follow-up Task exists and depends on the current Task. `replayed`
    /// is `true` when it already existed (idempotent replay).
    Created { task_id: String, replayed: bool },
    /// The human rejected the proposal; nothing was created.
    Rejected,
}

#[derive(Clone)]
pub struct FollowUpTaskService {
    db: Arc<SqliteDb>,
    plans: ProjectPlanService,
}

/// `sha256("followup|" + disposition_id)` (contract §11.8).
pub fn followup_idempotency_key(disposition_id: &str) -> String {
    hex::encode(Sha256::digest(
        format!("followup|{disposition_id}").as_bytes(),
    ))
}

fn followup_apply_key(disposition_id: &str) -> String {
    hex::encode(Sha256::digest(
        format!("followup|apply|{disposition_id}").as_bytes(),
    ))
}

fn followup_node_key(disposition_id: &str) -> String {
    format!("followup:{disposition_id}")
}

/// Current plan document + the follow-up node + the edge
/// `current Task -> follow-up`. Pure. The current Task is added as a plan node
/// (`@task:<id>`) when the document does not map it yet.
pub fn build_followup_document(
    mut document: ProjectPlanDocument,
    current_task_id: &str,
    current_task_title: &str,
    disposition_id: &str,
    proposal: &FollowUpProposal,
) -> ProjectPlanDocument {
    let current_key = match document
        .nodes
        .iter()
        .find(|node| node.task_id.as_deref() == Some(current_task_id))
    {
        Some(node) => node.key.clone(),
        None => {
            let mut key = format!("@task:{current_task_id}");
            while document.nodes.iter().any(|node| node.key == key) {
                key.push('_');
            }
            document.nodes.push(ProjectPlanNode {
                key: key.clone(),
                task_id: Some(current_task_id.to_owned()),
                title: current_task_title.to_owned(),
                description: None,
                acceptance_criteria: Vec::new(),
                phase: None,
                estimate_min_minutes: None,
                estimate_max_minutes: None,
                source_ids: Vec::new(),
            });
            key
        }
    };
    let new_key = followup_node_key(disposition_id);
    document.nodes.push(ProjectPlanNode {
        key: new_key.clone(),
        task_id: None,
        title: proposal.title.clone(),
        description: Some(proposal.description.clone()),
        acceptance_criteria: proposal.acceptance_criteria.clone(),
        phase: None,
        estimate_min_minutes: None,
        estimate_max_minutes: None,
        source_ids: Vec::new(),
    });
    document.edges.push(ProjectPlanEdge {
        predecessor_key: current_key,
        successor_key: new_key,
        inferred: false,
        source_ids: Vec::new(),
    });
    document
}

impl FollowUpTaskService {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self {
            plans: ProjectPlanService::new(Arc::clone(&db), event_bus),
            db,
        }
    }

    /// Apply the answer of a `propose_task` decision. Idempotent by decision
    /// id: the disposition's `followup_task_id` is set once, and the plan
    /// proposal/apply use deterministic keys derived from the disposition.
    pub async fn apply_approved(&self, human_decision_id: &str) -> Result<FollowUpOutcome> {
        let decision = HumanDecisionRepo::get_human_decision_request(&*self.db, human_decision_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("human decision", human_decision_id))?;
        if decision.source_kind != HumanDecisionSourceKind::ExternalFeedback
            || decision.state != HumanDecisionState::Answered
        {
            return Err(not_approved(
                "only an answered external_feedback decision creates a follow-up Task",
            ));
        }
        match decision.answered_option_id.as_deref() {
            Some(PROPOSAL_OPTION_REJECT) => return Ok(FollowUpOutcome::Rejected),
            Some(PROPOSAL_OPTION_APPROVE) => {}
            _ => {
                return Err(not_approved(
                    "the decision was not answered with the approve option",
                ))
            }
        }
        let user_id = decision
            .answered_by_user_id
            .clone()
            .ok_or_else(|| not_approved("the decision has no answering user"))?;

        let row = sqlx::query(
            "SELECT id, proposal_json, human_decision_id, followup_task_id
             FROM task_external_feedback_disposition
             WHERE task_id = ? AND feedback_item_id = ? AND disposition = 'propose_task'
               AND (human_decision_id = ? OR human_decision_id IS NULL)
             ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(&decision.task_id)
        .bind(&decision.source_id)
        .bind(&decision.id)
        .fetch_optional(self.db.pool())
        .await?
        .ok_or_else(|| {
            ServiceError::not_found("propose_task disposition", decision.source_id.clone())
        })?;
        let disposition_id: String = row.try_get("id")?;
        let proposal_json: Option<String> = row.try_get("proposal_json")?;
        let followup_task_id: Option<String> = row.try_get("followup_task_id")?;
        if let Some(task_id) = followup_task_id {
            return Ok(FollowUpOutcome::Created {
                task_id,
                replayed: true,
            });
        }
        let proposal: FollowUpProposal = serde_json::from_str(
            proposal_json
                .as_deref()
                .ok_or_else(|| not_approved("the disposition has no stored proposal"))?,
        )
        .map_err(|error| ServiceError::invalid_operation(format!("stored proposal: {error}")))?;
        proposal.validate().map_err(ServiceError::from)?;
        bind_decision_to_disposition(&self.db, &disposition_id, &decision.id).await?;

        let (task_id, replayed) = self
            .create_in_plan(&decision.task_id, &user_id, &disposition_id, &proposal)
            .await?;
        let updated = sqlx::query(
            "UPDATE task_external_feedback_disposition SET followup_task_id = ?
             WHERE id = ? AND followup_task_id IS NULL",
        )
        .bind(&task_id)
        .bind(&disposition_id)
        .execute(self.db.pool())
        .await?
        .rows_affected();
        if updated == 0 {
            let stored: Option<String> = sqlx::query_scalar(
                "SELECT followup_task_id FROM task_external_feedback_disposition WHERE id = ?",
            )
            .bind(&disposition_id)
            .fetch_one(self.db.pool())
            .await?;
            if stored.as_deref() != Some(task_id.as_str()) {
                return Err(db::DbError::IdempotencyConflict.into());
            }
        }
        Ok(FollowUpOutcome::Created { task_id, replayed })
    }

    /// Propose + apply the plan change; returns `(follow-up task id,
    /// replayed)`. Recoverable at every step: an existing proposal under the
    /// deterministic key is resumed instead of recreated.
    pub(crate) async fn create_in_plan(
        &self,
        current_task_id: &str,
        user_id: &str,
        disposition_id: &str,
        proposal: &FollowUpProposal,
    ) -> Result<(String, bool)> {
        let (project_id, title): (String, String) = {
            let row = sqlx::query(
                "SELECT project_id, title FROM task WHERE id = ? AND deleted_at IS NULL",
            )
            .bind(current_task_id)
            .fetch_optional(self.db.pool())
            .await?
            .ok_or_else(|| ServiceError::not_found("task", current_task_id.to_owned()))?;
            (row.try_get("project_id")?, row.try_get("title")?)
        };
        let propose_key = followup_idempotency_key(disposition_id);
        let apply_key = followup_apply_key(disposition_id);
        let node_key = followup_node_key(disposition_id);

        let existing = sqlx::query(
            "SELECT id, base_version, state FROM project_plan_proposal
             WHERE project_id = ? AND idempotency_key = ?",
        )
        .bind(&project_id)
        .bind(&propose_key)
        .fetch_optional(self.db.pool())
        .await?;
        let (proposal_id, base_version, state, was_existing) = match existing {
            Some(row) => (
                row.try_get::<String, _>("id")?,
                row.try_get::<i64, _>("base_version")?,
                row.try_get::<String, _>("state")?,
                true,
            ),
            None => {
                let plan_row = sqlx::query(
                    "SELECT version, document_json FROM project_plan WHERE project_id = ?",
                )
                .bind(&project_id)
                .fetch_optional(self.db.pool())
                .await?
                .ok_or_else(|| ServiceError::not_found("project", project_id.clone()))?;
                let version: i64 = plan_row.try_get("version")?;
                let document_json: String = plan_row.try_get("document_json")?;
                let document: ProjectPlanDocument =
                    serde_json::from_str(&document_json).map_err(|error| {
                        ServiceError::invalid_operation(format!("project plan document: {error}"))
                    })?;
                let document = build_followup_document(
                    document,
                    current_task_id,
                    &title,
                    disposition_id,
                    proposal,
                );
                let created = self
                    .plans
                    .propose(&project_id, user_id, version, document, &propose_key)
                    .await?;
                (created.id, created.base_version, created.state, false)
            }
        };
        if state != "applied" {
            self.plans
                .apply(&project_id, user_id, &proposal_id, base_version, &apply_key)
                .await?;
        }
        let task_id: String = sqlx::query_scalar(
            "SELECT task_id FROM project_plan_proposal_task WHERE proposal_id = ? AND node_key = ?",
        )
        .bind(&proposal_id)
        .bind(&node_key)
        .fetch_optional(self.db.pool())
        .await?
        .ok_or_else(|| {
            ServiceError::invalid_operation("the applied plan did not record the follow-up Task")
        })?;
        Ok((task_id, was_existing && state == "applied"))
    }
}

fn not_approved(message: &str) -> ServiceError {
    ServiceError::Coordination {
        code: CODE_FOLLOWUP_NOT_APPROVED,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::{
        create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, CreateProject, CreateTask,
        ProjectRepo, TaskLifecycleState, TaskRepo, User, UserRepo,
    };

    fn proposal() -> FollowUpProposal {
        FollowUpProposal {
            title: "Support X".to_owned(),
            description: "Add support for X.".to_owned(),
            acceptance_criteria: vec!["X works".to_owned()],
        }
    }

    #[test]
    fn pr_feedback_followup_key_is_deterministic_per_disposition() {
        let first = followup_idempotency_key("d1");
        assert_eq!(first.len(), 64);
        assert_eq!(first, followup_idempotency_key("d1"));
        assert_ne!(first, followup_idempotency_key("d2"));
        assert_ne!(first, followup_apply_key("d1"));
    }

    #[test]
    fn pr_feedback_followup_document_depends_on_current_task() {
        let document = build_followup_document(
            ProjectPlanDocument::default(),
            "task-1",
            "Current",
            "d1",
            &proposal(),
        );
        assert_eq!(document.nodes.len(), 2);
        let current = &document.nodes[0];
        assert_eq!(current.task_id.as_deref(), Some("task-1"));
        let follow_up = &document.nodes[1];
        assert!(follow_up.task_id.is_none());
        assert_eq!(follow_up.key, "followup:d1");
        assert_eq!(follow_up.acceptance_criteria, vec!["X works".to_owned()]);
        assert_eq!(document.edges.len(), 1);
        assert_eq!(document.edges[0].predecessor_key, current.key);
        assert_eq!(document.edges[0].successor_key, "followup:d1");
    }

    #[test]
    fn pr_feedback_followup_document_reuses_an_existing_current_node() {
        let existing = ProjectPlanDocument {
            nodes: vec![ProjectPlanNode {
                key: "plan-a".to_owned(),
                task_id: Some("task-1".to_owned()),
                title: "Current".to_owned(),
                description: None,
                acceptance_criteria: Vec::new(),
                phase: None,
                estimate_min_minutes: None,
                estimate_max_minutes: None,
                source_ids: Vec::new(),
            }],
            edges: Vec::new(),
            sources: Vec::new(),
        };
        let document = build_followup_document(existing, "task-1", "Current", "d1", &proposal());
        assert_eq!(document.nodes.len(), 2);
        assert_eq!(document.edges[0].predecessor_key, "plan-a");
    }

    struct Fixture {
        db: Arc<SqliteDb>,
        service: FollowUpTaskService,
        owner_id: String,
        project_id: String,
        task_id: String,
    }

    async fn fixture() -> Fixture {
        let pool = create_sqlite_pool("sqlite::memory:")
            .await
            .expect("pool creates");
        run_migrations(&pool).await.expect("migrations run");
        let db = Arc::new(SqliteDb::new(pool));
        let now = now_rfc3339();
        let owner_id = new_uuid_v4();
        UserRepo::create_user(
            db.as_ref(),
            &User {
                id: owner_id.clone(),
                email: format!("{owner_id}@example.test"),
                password_hash: "test-hash".to_owned(),
                display_name: Some("Owner".to_owned()),
                is_admin: false,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("owner creates");
        let project_id = new_uuid_v4();
        ProjectRepo::create(
            db.as_ref(),
            CreateProject {
                id: project_id.clone(),
                name: "Follow-up fixture".to_owned(),
                settings: "{}".to_owned(),
                primary_repo_id: None,
                owner_id: Some(owner_id.clone()),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("project creates");
        let task_id = new_uuid_v4();
        TaskRepo::create(
            db.as_ref(),
            CreateTask {
                repositories: Vec::new(),
                id: task_id.clone(),
                project_id: project_id.clone(),
                repo_id: None,
                parent_task_id: None,
                title: "Current Task".to_owned(),
                description: None,
                lifecycle_state: TaskLifecycleState::Backlog,
                is_automation: false,
                priority: 0,
                subtask_order: None,
                merge_config: None,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("task creates");
        let service = FollowUpTaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
        Fixture {
            db,
            service,
            owner_id,
            project_id,
            task_id,
        }
    }

    #[tokio::test]
    async fn pr_feedback_followup_creates_dependent_task_once() {
        let fixture = fixture().await;
        let (task_id, replayed) = fixture
            .service
            .create_in_plan(
                &fixture.task_id,
                &fixture.owner_id,
                "disposition-1",
                &proposal(),
            )
            .await
            .expect("follow-up Task is created through the plan services");
        assert!(!replayed);
        assert_ne!(task_id, fixture.task_id);
        let depends: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM task_dependency WHERE task_id = ? AND depends_on_id = ?)",
        )
        .bind(&task_id)
        .bind(&fixture.task_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("dependency query runs");
        assert!(depends, "the follow-up depends on the current Task");

        let (again, replayed) = fixture
            .service
            .create_in_plan(
                &fixture.task_id,
                &fixture.owner_id,
                "disposition-1",
                &proposal(),
            )
            .await
            .expect("replay resumes the applied proposal");
        assert_eq!(again, task_id);
        assert!(replayed);
        let tasks: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task WHERE project_id = ? AND deleted_at IS NULL",
        )
        .bind(&fixture.project_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("count runs");
        assert_eq!(tasks, 2, "no duplicate Task on replay");
    }

    #[tokio::test]
    async fn pr_feedback_followup_requires_an_answered_approval() {
        let fixture = fixture().await;
        let error = fixture
            .service
            .apply_approved("missing-decision")
            .await
            .expect_err("an unknown decision creates nothing");
        assert!(matches!(error, ServiceError::NotFound { .. }));
    }
}
