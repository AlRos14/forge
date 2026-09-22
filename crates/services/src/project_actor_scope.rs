use std::collections::HashSet;

use api_types::ActorRef;
use db::{
    canonical_task_role_name, new_uuid_v4, now_rfc3339, AgentRepo, ProjectAgentBindingRepo,
    ProjectMemberRepo, SqliteDb, UserRepo,
};
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
            let is_project_member = ProjectMemberRepo::get_member(db, &project.id, user_id)
                .await?
                .is_some();
            Ok(project.owner_id.as_deref() == Some(user_id) || is_project_member)
        }
        ActorRef::Agent(agent_id) => {
            if agent_id == "human" {
                return Ok(false);
            }
            let Some(agent) = AgentRepo::get_by_id(db, agent_id).await? else {
                return Ok(false);
            };
            let owner_is_project_actor = match agent.owner_id.as_deref() {
                Some(owner_id) if project.owner_id.as_deref() == Some(owner_id) => true,
                Some(owner_id) => ProjectMemberRepo::get_member(db, &project.id, owner_id)
                    .await?
                    .is_some(),
                None => false,
            };
            let binding =
                ProjectAgentBindingRepo::get_active_project_binding(db, &project.id).await?;
            let has_active_binding = binding.as_ref().is_some_and(|binding| {
                binding.state == "active" && binding.identity_id.as_deref() == Some(agent_id)
            });
            Ok(agent_is_valid_from_sources(
                &agent.visibility,
                owner_is_project_actor,
                has_active_binding,
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
    project_owner_id: Option<&str>,
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
            Ok(project_owner_id == Some(user_id) || member_exists != 0)
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
                Some(owner_id) if project_owner_id == Some(owner_id) => true,
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
            let has_active_binding: i64 = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM project_agent_binding
                    WHERE project_id = ? AND identity_id = ? AND state = 'active'
                )",
            )
            .bind(project_id)
            .bind(agent_id)
            .fetch_one(&mut **transaction)
            .await?;
            Ok(agent_is_valid_from_sources(
                &visibility,
                owner_is_project_actor,
                has_active_binding != 0,
            ))
        }
    }
}

fn agent_is_valid_from_sources(
    visibility: &str,
    owner_is_project_actor: bool,
    has_active_binding: bool,
) -> bool {
    visibility == "global"
        || (visibility == "account" && owner_is_project_actor)
        || has_active_binding
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
            "SELECT DISTINCT tr.task_id, tr.id AS task_role_id, tr.role
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
            let canonical_role: String = role.try_get("role")?;
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
            sync_legacy_role_projection_in_tx(
                transaction,
                &task_id,
                &task_role_id,
                &canonical_role,
                None,
                None,
            )
            .await?;
        }
    }
    Ok(effects)
}

/// The single compatibility projection algorithm used by membership writes
/// and scope revocation. It never selects a representative for authority.
pub(crate) async fn sync_legacy_role_projection_in_tx(
    transaction: &mut Transaction<'_, Sqlite>,
    task_id: &str,
    task_role_id: &str,
    canonical_role: &str,
    fallback_role: Option<&str>,
    preferred_projection_role: Option<&str>,
) -> Result<()> {
    let member = sqlx::query(
        "SELECT actor_kind, actor_id, created_at
         FROM role_membership
         WHERE task_role_id = ? AND status = 'active'
         ORDER BY CASE actor_kind WHEN 'agent' THEN 0 ELSE 1 END,
                  created_at, id
         LIMIT 1",
    )
    .bind(task_role_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let mut projection_roles = sqlx::query(
        "SELECT role_name
         FROM task_role_assignment
         WHERE task_id = ?
         ORDER BY role_name",
    )
    .bind(task_id)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .filter_map(|row| {
        let role_name: String = row.try_get("role_name").ok()?;
        (canonical_task_role_name(&role_name).as_deref() == Some(canonical_role))
            .then_some(role_name)
    })
    .collect::<Vec<_>>();
    if projection_roles.is_empty() {
        if let Some(preferred) = preferred_projection_role {
            if canonical_task_role_name(preferred).as_deref() == Some(canonical_role) {
                projection_roles.push(preferred.to_owned());
            }
        }
    }
    if projection_roles.is_empty() {
        if let Some(role) = fallback_role {
            projection_roles.push(role.to_owned());
        }
    }
    if projection_roles.is_empty() && canonical_role != "implementer" {
        return Ok(());
    }

    let now = now_rfc3339();
    let Some(member) = member else {
        if canonical_role == "implementer" {
            sqlx::query(
                "UPDATE task
                 SET assignee_type = NULL, assignee_id = NULL, updated_at = ?
                 WHERE id = ?",
            )
            .bind(now)
            .bind(task_id)
            .execute(&mut **transaction)
            .await?;
        }
        for projection_role in projection_roles {
            sqlx::query("DELETE FROM task_role_assignment WHERE task_id = ? AND role_name = ?")
                .bind(task_id)
                .bind(projection_role)
                .execute(&mut **transaction)
                .await?;
        }
        return Ok(());
    };
    let actor_kind: String = member.try_get("actor_kind")?;
    let actor_id: String = member.try_get("actor_id")?;
    let member_created_at: String = member.try_get("created_at")?;
    let assignee_type = if actor_kind == "agent" {
        "agent"
    } else {
        "user"
    };
    if canonical_role == "implementer" {
        sqlx::query(
            "UPDATE task
             SET assignee_type = ?, assignee_id = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(assignee_type)
        .bind(&actor_id)
        .bind(&now)
        .bind(task_id)
        .execute(&mut **transaction)
        .await?;
    }
    if projection_roles.is_empty() {
        return Ok(());
    }
    for projection_role in projection_roles {
        sqlx::query(
            "INSERT INTO task_role_assignment
                (id, task_id, role_name, assignee_type, assignee_id, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(task_id, role_name) DO UPDATE SET
                assignee_type = excluded.assignee_type,
                assignee_id = excluded.assignee_id,
                updated_at = excluded.updated_at",
        )
        .bind(new_uuid_v4())
        .bind(task_id)
        .bind(projection_role)
        .bind(assignee_type)
        .bind(&actor_id)
        .bind(&member_created_at)
        .bind(&now)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}
