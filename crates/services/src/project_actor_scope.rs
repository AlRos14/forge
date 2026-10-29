use std::collections::HashSet;

use api_types::ActorRef;
use db::{AgentRepo, ProjectMemberRepo, SqliteDb, UserRepo};
use events::{event_timestamp, EventBus, EventContext, ForgeEvent};
use sqlx::{Row, Sqlite, Transaction};

use crate::Result;

/// The domain predicate shared by TaskRole creation and scope revocation.
/// Runtime status is deliberately not consulted here: a paused or busy Agent
/// may still be a valid current Project Actor.
pub(crate) async fn actor_is_valid_for_project(
    db: &SqliteDb,
    project: &db::Project,
    actor: &ActorRef,
) -> Result<bool> {
    match actor {
        ActorRef::Human(user_id) => {
            if user_id == "human" || UserRepo::get_user_by_id(db, user_id).await?.is_none() {
                return Ok(false);
            }
            Ok(ProjectMemberRepo::get_member(db, &project.id, user_id)
                .await?
                .is_some())
        }
        ActorRef::Agent(agent_id) => {
            if agent_id == "human" {
                return Ok(false);
            }
            let Some(agent) = AgentRepo::get_by_id(db, agent_id).await? else {
                return Ok(false);
            };
            let owner_is_project_actor = match agent.owner_id.as_deref() {
                Some(owner_id) => ProjectMemberRepo::get_member(db, &project.id, owner_id)
                    .await?
                    .is_some(),
                None => false,
            };
            Ok(agent_is_valid_from_sources(
                &agent.visibility,
                owner_is_project_actor,
            ))
        }
    }
}

/// The transaction variant evaluates the same predicate against the state
/// visible after the scope mutation. It intentionally uses direct SQLite
/// reads so the caller's transaction remains the sole authority boundary.
pub(crate) async fn actor_is_valid_for_project_in_tx(
    transaction: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    _project_owner_id: Option<&str>,
    actor: &ActorRef,
) -> Result<bool> {
    match actor {
        ActorRef::Human(user_id) => {
            if user_id == "human" {
                return Ok(false);
            }
            let user_exists: i64 =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM user WHERE id = ?)")
                    .bind(user_id)
                    .fetch_one(&mut **transaction)
                    .await?;
            if user_exists == 0 {
                return Ok(false);
            }
            let member_exists: i64 = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM project_member WHERE project_id = ? AND user_id = ?
                )",
            )
            .bind(project_id)
            .bind(user_id)
            .fetch_one(&mut **transaction)
            .await?;
            Ok(member_exists != 0)
        }
        ActorRef::Agent(agent_id) => {
            if agent_id == "human" {
                return Ok(false);
            }
            let Some(agent) =
                sqlx::query("SELECT visibility, owner_id FROM agent_current WHERE id = ?")
                    .bind(agent_id)
                    .fetch_optional(&mut **transaction)
                    .await?
            else {
                return Ok(false);
            };
            let visibility: String = agent.try_get("visibility")?;
            let owner_id: Option<String> = agent.try_get("owner_id")?;
            let owner_is_project_actor = match owner_id.as_deref() {
                Some(owner_id) => {
                    let member_exists: i64 = sqlx::query_scalar(
                        "SELECT EXISTS(
                            SELECT 1 FROM project_member
                            WHERE project_id = ? AND user_id = ?
                        )",
                    )
                    .bind(project_id)
                    .bind(owner_id)
                    .fetch_one(&mut **transaction)
                    .await?;
                    member_exists != 0
                }
                None => false,
            };
            Ok(agent_is_valid_from_sources(
                &visibility,
                owner_is_project_actor,
            ))
        }
    }
}

fn agent_is_valid_from_sources(visibility: &str, owner_is_project_actor: bool) -> bool {
    visibility == "global" || (visibility == "account" && owner_is_project_actor)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScopeRevocationEffect {
    pub project_id: String,
    pub task_id: String,
}

/// Publish task projections after the transaction that produced them commits.
pub(crate) fn publish_scope_revocation_events(
    event_bus: &EventBus,
    effects: &[ScopeRevocationEffect],
) {
    for effect in effects {
        event_bus.publish(ForgeEvent {
            event_type: "task.updated".to_owned(),
            entity_id: effect.task_id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskUpdated {
                project_id: effect.project_id.clone(),
            },
        });
    }
}

/// End current memberships for actors whose Project scope disappeared and
/// rebuild the existing singular compatibility projection from survivors.
/// The caller owns the transaction and decides when to commit/publish.
pub(crate) async fn reconcile_project_actor_memberships_in_tx(
    transaction: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    project_owner_id: Option<&str>,
    actors: &[ActorRef],
    now: &str,
) -> Result<Vec<ScopeRevocationEffect>> {
    let mut seen = HashSet::new();
    let mut affected_task_ids = HashSet::new();
    let mut effects = Vec::new();
    for actor in actors {
        let key = match actor {
            ActorRef::Human(id) => ("human", id.as_str()),
            ActorRef::Agent(id) => ("agent", id.as_str()),
        };
        if !seen.insert(key) {
            continue;
        }
        if actor_is_valid_for_project_in_tx(transaction, project_id, project_owner_id, actor)
            .await?
        {
            continue;
        }

        let (actor_kind, actor_id) = match actor {
            ActorRef::Human(id) => ("human", id.as_str()),
            ActorRef::Agent(id) => ("agent", id.as_str()),
        };
        let affected_roles = sqlx::query(
            "SELECT DISTINCT tr.task_id, tr.id AS task_role_id
             FROM task AS t
             JOIN task_role AS tr ON tr.task_id = t.id
             JOIN role_membership AS rm ON rm.task_role_id = tr.id
             WHERE t.project_id = ?
               AND rm.actor_kind = ?
               AND rm.actor_id = ?
               AND rm.status IN ('active', 'suspended')
             ORDER BY tr.task_id, tr.role, tr.id",
        )
        .bind(project_id)
        .bind(actor_kind)
        .bind(actor_id)
        .fetch_all(&mut **transaction)
        .await?;

        for role in affected_roles {
            let task_id: String = role.try_get("task_id")?;
            let task_role_id: String = role.try_get("task_role_id")?;
            if affected_task_ids.insert(task_id.clone()) {
                effects.push(ScopeRevocationEffect {
                    project_id: project_id.to_owned(),
                    task_id: task_id.clone(),
                });
            }
            sqlx::query(
                "UPDATE role_membership
                 SET status = 'ended', ended_at = ?, updated_at = ?, version = version + 1
                 WHERE task_role_id = ? AND actor_kind = ? AND actor_id = ?
                   AND status IN ('active', 'suspended')",
            )
            .bind(now)
            .bind(now)
            .bind(&task_role_id)
            .bind(actor_kind)
            .bind(actor_id)
            .execute(&mut **transaction)
            .await?;
        }
    }
    Ok(effects)
}
