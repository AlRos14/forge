use std::sync::Arc;

use api_types::ActorRef;
use db::{
    new_uuid_v4, now_rfc3339, CreateProjectMember, Project, ProjectMember, ProjectMemberRepo,
    ProjectRepo, SqliteDb,
};
use events::EventBus;
use sqlx::Row;

use crate::{project_actor_scope, Result, ServiceError};

/// Project ownership invariant (INV-040):
///
/// `project.owner_id` is the canonical owner. When it is non-null, the exact
/// same user has a `project_member` row with role `owner`. Additional `owner`
/// memberships are allowed, but every mutation that removes or demotes the
/// canonical owner transfers canonical identity to another current owner in the
/// same `BEGIN IMMEDIATE` transaction, or fails with `last_owner`.
#[derive(Clone)]
pub struct ProjectMemberService {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
}

impl ProjectMemberService {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self { db, event_bus }
    }

    pub async fn list_members(
        &self,
        project_id: &str,
        caller_user_id: &str,
    ) -> Result<Vec<ProjectMember>> {
        self.check_access(project_id, caller_user_id).await?;
        ProjectMemberRepo::list_members(&*self.db, project_id)
            .await
            .map_err(Into::into)
    }

    pub async fn add_member(
        &self,
        project_id: &str,
        caller_user_id: &str,
        target_user_id: &str,
        role: &str,
    ) -> Result<ProjectMember> {
        let mut transaction = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let project = load_project_in_tx(&self.db, &mut transaction, project_id).await?;
        let caller =
            require_member_in_tx(&self.db, &mut transaction, project_id, caller_user_id).await?;
        let owners = list_owners_in_tx(&self.db, &mut transaction, project_id).await?;
        ensure_canonical_owner_membership(&project, &owners)?;

        if !is_admin_or_owner(&caller.role) {
            return Err(ServiceError::InvalidOperation {
                message: "insufficient_role".to_owned(),
            });
        }
        if caller.role == "admin" && role == "owner" {
            return Err(ServiceError::InvalidOperation {
                message: "not_owner".to_owned(),
            });
        }

        let now = now_rfc3339();
        let member = ProjectMemberRepo::add_member_in_tx(
            &*self.db,
            &mut transaction,
            CreateProjectMember {
                id: new_uuid_v4(),
                project_id: project_id.to_owned(),
                user_id: target_user_id.to_owned(),
                role: role.to_owned(),
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await?;
        transaction.commit().await?;
        Ok(member)
    }

    pub async fn update_role(
        &self,
        project_id: &str,
        caller_user_id: &str,
        target_user_id: &str,
        role: &str,
    ) -> Result<ProjectMember> {
        let mut transaction = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let project = load_project_in_tx(&self.db, &mut transaction, project_id).await?;
        let caller =
            require_member_in_tx(&self.db, &mut transaction, project_id, caller_user_id).await?;
        let target =
            require_member_in_tx(&self.db, &mut transaction, project_id, target_user_id).await?;
        let owners = list_owners_in_tx(&self.db, &mut transaction, project_id).await?;
        ensure_canonical_owner_membership(&project, &owners)?;

        if !is_admin_or_owner(&caller.role) {
            return Err(ServiceError::InvalidOperation {
                message: "insufficient_role".to_owned(),
            });
        }
        if caller.role == "admin" && (target.role == "owner" || role == "owner") {
            return Err(ServiceError::InvalidOperation {
                message: "not_owner".to_owned(),
            });
        }

        let now = now_rfc3339();
        if target.role == "owner"
            && role != "owner"
            && project.owner_id.as_deref() == Some(target.user_id.as_str())
        {
            let replacement = next_owner(&owners, target_user_id)
                .ok_or_else(|| ServiceError::Conflict("last_owner".to_owned()))?;
            ProjectRepo::set_owner_in_tx(
                &*self.db,
                &mut transaction,
                project_id,
                &replacement.user_id,
                &now,
            )
            .await?;
        }

        let member = ProjectMemberRepo::update_member_role_in_tx(
            &*self.db,
            &mut transaction,
            project_id,
            target_user_id,
            role,
            &now,
        )
        .await?;
        transaction.commit().await?;
        Ok(member)
    }

    pub async fn remove_member(
        &self,
        project_id: &str,
        caller_user_id: &str,
        target_user_id: &str,
    ) -> Result<()> {
        let mut transaction = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let project = load_project_in_tx(&self.db, &mut transaction, project_id).await?;
        let caller =
            require_member_in_tx(&self.db, &mut transaction, project_id, caller_user_id).await?;
        let target =
            require_member_in_tx(&self.db, &mut transaction, project_id, target_user_id).await?;
        let owners = list_owners_in_tx(&self.db, &mut transaction, project_id).await?;
        ensure_canonical_owner_membership(&project, &owners)?;

        if !is_admin_or_owner(&caller.role) {
            return Err(ServiceError::InvalidOperation {
                message: "insufficient_role".to_owned(),
            });
        }
        if caller.role == "admin" && target.role == "owner" {
            return Err(ServiceError::InvalidOperation {
                message: "not_owner".to_owned(),
            });
        }

        let now = now_rfc3339();
        let mut canonical_owner_id = project.owner_id.clone();
        if target.role == "owner" && project.owner_id.as_deref() == Some(target.user_id.as_str()) {
            let replacement = next_owner(&owners, target_user_id)
                .ok_or_else(|| ServiceError::Conflict("last_owner".to_owned()))?;
            ProjectRepo::set_owner_in_tx(
                &*self.db,
                &mut transaction,
                project_id,
                &replacement.user_id,
                &now,
            )
            .await?;
            canonical_owner_id = Some(replacement.user_id.clone());
        }

        ProjectMemberRepo::remove_member_in_tx(
            &*self.db,
            &mut transaction,
            project_id,
            target_user_id,
        )
        .await?;

        let account_agents = sqlx::query(
            "SELECT id FROM agent_current WHERE visibility = 'account' AND owner_id = ? ORDER BY id",
        )
        .bind(target_user_id)
        .fetch_all(&mut *transaction)
        .await?;
        let mut affected_actors = vec![ActorRef::Human(target_user_id.to_owned())];
        affected_actors.extend(
            account_agents
                .into_iter()
                .map(|row| row.try_get::<String, _>("id").map(ActorRef::Agent))
                .collect::<std::result::Result<Vec<_>, _>>()?,
        );
        let effects = project_actor_scope::reconcile_project_actor_memberships_in_tx(
            &mut transaction,
            project_id,
            canonical_owner_id.as_deref(),
            &affected_actors,
            &now,
        )
        .await?;
        transaction.commit().await?;
        project_actor_scope::publish_scope_revocation_events(&self.event_bus, &effects);
        Ok(())
    }

    pub async fn get_own_membership(
        &self,
        project_id: &str,
        caller_user_id: &str,
    ) -> Result<ProjectMember> {
        self.require_member(project_id, caller_user_id).await
    }

    pub async fn check_access(&self, project_id: &str, user_id: &str) -> Result<ProjectMember> {
        self.require_member(project_id, user_id).await
    }

    async fn require_member(&self, project_id: &str, user_id: &str) -> Result<ProjectMember> {
        ProjectMemberRepo::get_member(&*self.db, project_id, user_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", project_id.to_owned()))
    }
}

async fn load_project_in_tx(
    db: &SqliteDb,
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    project_id: &str,
) -> Result<Project> {
    ProjectRepo::get_by_id_in_tx(db, transaction, project_id)
        .await?
        .ok_or_else(|| ServiceError::not_found("project", project_id.to_owned()))
}

async fn require_member_in_tx(
    db: &SqliteDb,
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    project_id: &str,
    user_id: &str,
) -> Result<ProjectMember> {
    ProjectMemberRepo::get_member_in_tx(db, transaction, project_id, user_id)
        .await?
        .ok_or_else(|| ServiceError::not_found("project", project_id.to_owned()))
}

async fn list_owners_in_tx(
    db: &SqliteDb,
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    project_id: &str,
) -> Result<Vec<ProjectMember>> {
    let members = ProjectMemberRepo::list_members_in_tx(db, transaction, project_id).await?;
    Ok(members
        .into_iter()
        .filter(|member| member.role == "owner")
        .collect())
}

fn ensure_canonical_owner_membership(project: &Project, owners: &[ProjectMember]) -> Result<()> {
    if let Some(owner_id) = project.owner_id.as_deref() {
        if !owners
            .iter()
            .any(|member| member.user_id == owner_id && member.role == "owner")
        {
            return Err(ServiceError::Conflict(
                "project_owner_membership_missing".to_owned(),
            ));
        }
    }
    Ok(())
}

fn next_owner<'a>(
    owners: &'a [ProjectMember],
    excluding_user_id: &str,
) -> Option<&'a ProjectMember> {
    owners
        .iter()
        .find(|member| member.user_id != excluding_user_id)
}

fn is_admin_or_owner(role: &str) -> bool {
    role == "owner" || role == "admin"
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::{create_sqlite_pool, run_migrations, SqliteDb};
    use events::EventBus;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    async fn test_service() -> ProjectMemberService {
        let pool = create_sqlite_pool("sqlite::memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();
        ProjectMemberService::new(Arc::new(SqliteDb::new(pool)), Arc::new(EventBus::new(16)))
    }

    struct FileDb {
        path: PathBuf,
        service: ProjectMemberService,
    }

    impl Drop for FileDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
            let _ = std::fs::remove_file(format!("{}-wal", self.path.display()));
            let _ = std::fs::remove_file(format!("{}-shm", self.path.display()));
        }
    }

    async fn file_service() -> FileDb {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "forge-project-member-{}-{nanos}.db",
            std::process::id()
        ));
        let pool = create_sqlite_pool(&format!("sqlite://{}", path.display()))
            .await
            .unwrap();
        run_migrations(&pool).await.unwrap();
        let service =
            ProjectMemberService::new(Arc::new(SqliteDb::new(pool)), Arc::new(EventBus::new(16)));
        FileDb { path, service }
    }

    async fn seed_user(db: &SqliteDb, user_id: &str, email: &str) {
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO user (id, email, password_hash, is_admin, created_at, updated_at) VALUES (?, ?, ?, 0, ?, ?)",
        )
        .bind(user_id)
        .bind(email)
        .bind("pw")
        .bind(&now)
        .bind(&now)
        .execute(db.pool())
        .await
        .unwrap();
    }

    async fn seed_project(db: &SqliteDb, project_id: &str, owner_id: Option<&str>) {
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO project (id, name, settings, owner_id, created_at, updated_at) VALUES (?, 'p', '{}', ?, ?, ?)",
        )
        .bind(project_id)
        .bind(owner_id)
        .bind(&now)
        .bind(&now)
        .execute(db.pool())
        .await
        .unwrap();
    }

    async fn add_member(db: &SqliteDb, project_id: &str, user_id: &str, role: &str) {
        let now = now_rfc3339();
        ProjectMemberRepo::add_member(
            db,
            CreateProjectMember {
                id: new_uuid_v4(),
                project_id: project_id.to_owned(),
                user_id: user_id.to_owned(),
                role: role.to_owned(),
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .unwrap();
    }

    async fn seed_project_with_owners(
        db: &SqliteDb,
        project_id: &str,
        owner_id: &str,
        extra_owners: &[&str],
    ) {
        seed_project(db, project_id, Some(owner_id)).await;
        add_member(db, project_id, owner_id, "owner").await;
        for extra in extra_owners {
            add_member(db, project_id, extra, "owner").await;
        }
    }

    #[tokio::test]
    async fn admin_cannot_assign_owner() {
        let svc = test_service().await;
        let db = &svc.db;
        seed_user(db, "u_owner", "owner@test.com").await;
        seed_user(db, "u_admin", "admin@test.com").await;
        seed_user(db, "u_new", "new@test.com").await;
        seed_project(db, "p1", Some("u_owner")).await;
        add_member(db, "p1", "u_owner", "owner").await;
        add_member(db, "p1", "u_admin", "admin").await;

        let result = svc.add_member("p1", "u_admin", "u_new", "owner").await;
        assert!(matches!(
            result,
            Err(ServiceError::InvalidOperation { message }) if message == "not_owner"
        ));
    }

    #[tokio::test]
    async fn prevents_removing_last_owner() {
        let svc = test_service().await;
        let db = &svc.db;
        seed_user(db, "u_owner", "owner@test.com").await;
        seed_user(db, "u_admin", "admin@test.com").await;
        seed_project(db, "p1", Some("u_owner")).await;
        add_member(db, "p1", "u_owner", "owner").await;
        add_member(db, "p1", "u_admin", "admin").await;

        let result = svc.remove_member("p1", "u_owner", "u_owner").await;
        assert!(matches!(
            result,
            Err(ServiceError::Conflict(message)) if message == "last_owner"
        ));
    }

    #[tokio::test]
    async fn check_access_requires_membership() {
        let svc = test_service().await;
        let db = &svc.db;
        seed_user(db, "u_owner", "owner@test.com").await;
        seed_user(db, "u_other", "other@test.com").await;
        seed_project(db, "p1", Some("u_owner")).await;
        add_member(db, "p1", "u_owner", "owner").await;

        let ok = svc.check_access("p1", "u_owner").await;
        assert!(ok.is_ok());

        let denied = svc.check_access("p1", "u_other").await;
        assert!(matches!(denied, Err(ServiceError::NotFound { .. })));
    }

    #[tokio::test]
    async fn canonical_owner_without_membership_fails_closed() {
        let svc = test_service().await;
        let db = &svc.db;
        seed_user(db, "u_owner", "owner@test.com").await;
        seed_user(db, "u_admin", "admin@test.com").await;
        seed_project(db, "p1", Some("u_owner")).await;
        add_member(db, "p1", "u_admin", "admin").await;

        let result = svc.add_member("p1", "u_admin", "u_owner", "member").await;
        assert!(matches!(
            result,
            Err(ServiceError::Conflict(message)) if message == "project_owner_membership_missing"
        ));
    }

    #[tokio::test]
    async fn removing_canonical_owner_transfers_to_remaining_owner() {
        let svc = test_service().await;
        let db = &svc.db;
        seed_user(db, "u_a", "a@test.com").await;
        seed_user(db, "u_b", "b@test.com").await;
        seed_project_with_owners(db, "p1", "u_a", &["u_b"]).await;

        svc.remove_member("p1", "u_a", "u_a").await.unwrap();

        let project = ProjectRepo::get_by_id(&**db, "p1").await.unwrap().unwrap();
        assert_eq!(project.owner_id.as_deref(), Some("u_b"));
        assert!(ProjectMemberRepo::get_member(&**db, "p1", "u_a")
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            ProjectMemberRepo::get_member(&**db, "p1", "u_b")
                .await
                .unwrap()
                .unwrap()
                .role,
            "owner"
        );
    }

    #[tokio::test]
    async fn demoting_canonical_owner_transfers_to_remaining_owner() {
        let svc = test_service().await;
        let db = &svc.db;
        seed_user(db, "u_a", "a@test.com").await;
        seed_user(db, "u_b", "b@test.com").await;
        seed_project_with_owners(db, "p1", "u_a", &["u_b"]).await;

        svc.update_role("p1", "u_a", "u_a", "admin").await.unwrap();

        let project = ProjectRepo::get_by_id(&**db, "p1").await.unwrap().unwrap();
        assert_eq!(project.owner_id.as_deref(), Some("u_b"));
        assert_eq!(
            ProjectMemberRepo::get_member(&**db, "p1", "u_a")
                .await
                .unwrap()
                .unwrap()
                .role,
            "admin"
        );
    }

    #[tokio::test]
    async fn concurrent_owner_removals_leave_one_owner_and_one_winner() {
        let harness = file_service().await;
        let db: &SqliteDb = harness.service.db.as_ref();
        for user in ["u_a", "u_b", "u_admin"] {
            seed_user(db, user, &format!("{user}@test.com")).await;
        }
        seed_project_with_owners(db, "p1", "u_a", &["u_b"]).await;
        add_member(db, "p1", "u_admin", "admin").await;

        let first = harness.service.clone();
        let second = harness.service.clone();
        let (left, right) = tokio::join!(
            first.remove_member("p1", "u_a", "u_a"),
            second.remove_member("p1", "u_b", "u_b"),
        );

        let successes = [&left, &right]
            .iter()
            .filter(|result| result.is_ok())
            .count();
        let conflicts = [&left, &right]
            .iter()
            .filter(|result| {
                matches!(result, Err(ServiceError::Conflict(message)) if message == "last_owner")
            })
            .count();
        assert_eq!(successes, 1, "exactly one owner removal wins");
        assert_eq!(conflicts, 1, "the other removal is the last-owner conflict");

        let owners = ProjectMemberRepo::list_members(db, "p1")
            .await
            .unwrap()
            .into_iter()
            .filter(|member| member.role == "owner")
            .collect::<Vec<_>>();
        assert_eq!(owners.len(), 1);
        let project = ProjectRepo::get_by_id(db, "p1").await.unwrap().unwrap();
        assert_eq!(
            project.owner_id.as_deref(),
            Some(owners[0].user_id.as_str())
        );
    }

    #[tokio::test]
    async fn concurrent_demotion_and_removal_leave_one_owner() {
        let harness = file_service().await;
        let db: &SqliteDb = harness.service.db.as_ref();
        for user in ["u_a", "u_b"] {
            seed_user(db, user, &format!("{user}@test.com")).await;
        }
        seed_project_with_owners(db, "p1", "u_a", &["u_b"]).await;

        let first = harness.service.clone();
        let second = harness.service.clone();
        let (left, right) = tokio::join!(
            first.update_role("p1", "u_a", "u_a", "admin"),
            second.remove_member("p1", "u_b", "u_b"),
        );

        assert!(left.is_ok() || right.is_ok());
        assert!(
            matches!(left, Err(ServiceError::Conflict(ref message)) if message == "last_owner")
                || matches!(right, Err(ServiceError::Conflict(ref message)) if message == "last_owner")
        );

        let owners = ProjectMemberRepo::list_members(db, "p1")
            .await
            .unwrap()
            .into_iter()
            .filter(|member| member.role == "owner")
            .collect::<Vec<_>>();
        assert_eq!(owners.len(), 1);
        let project = ProjectRepo::get_by_id(db, "p1").await.unwrap().unwrap();
        assert_eq!(
            project.owner_id.as_deref(),
            Some(owners[0].user_id.as_str())
        );
    }

    #[tokio::test]
    async fn concurrent_removals_on_distinct_projects_both_succeed() {
        let harness = file_service().await;
        let db: &SqliteDb = harness.service.db.as_ref();
        for user in ["u_a", "u_b", "u_c", "u_d"] {
            seed_user(db, user, &format!("{user}@test.com")).await;
        }
        seed_project_with_owners(db, "p1", "u_a", &["u_b"]).await;
        seed_project_with_owners(db, "p2", "u_c", &["u_d"]).await;

        let first = harness.service.clone();
        let second = harness.service.clone();
        let (left, right) = tokio::join!(
            first.remove_member("p1", "u_a", "u_a"),
            second.remove_member("p2", "u_c", "u_c"),
        );
        left.unwrap();
        right.unwrap();

        let p1 = ProjectRepo::get_by_id(db, "p1").await.unwrap().unwrap();
        let p2 = ProjectRepo::get_by_id(db, "p2").await.unwrap().unwrap();
        assert_eq!(p1.owner_id.as_deref(), Some("u_b"));
        assert_eq!(p2.owner_id.as_deref(), Some("u_d"));
    }

    #[tokio::test]
    async fn caller_demoted_while_waiting_for_the_writer_is_rejected() {
        let harness = file_service().await;
        let db: &SqliteDb = harness.service.db.as_ref();
        for user in ["u_a", "u_b", "u_admin"] {
            seed_user(db, user, &format!("{user}@test.com")).await;
        }
        seed_project_with_owners(db, "p1", "u_a", &["u_b"]).await;
        add_member(db, "p1", "u_admin", "admin").await;

        let mut tx = harness
            .service
            .db
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .unwrap();
        sqlx::query("UPDATE project_member SET role = 'member' WHERE project_id = 'p1' AND user_id = 'u_admin'")
            .execute(&mut *tx)
            .await
            .unwrap();

        let service = harness.service.clone();
        let waiting =
            tokio::spawn(async move { service.remove_member("p1", "u_admin", "u_b").await });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        tx.commit().await.unwrap();

        let result = waiting.await.expect("waiting task joins");
        assert!(matches!(
            result,
            Err(ServiceError::InvalidOperation { message }) if message == "insufficient_role"
        ));
        assert!(ProjectMemberRepo::get_member(db, "p1", "u_b")
            .await
            .unwrap()
            .is_some());
        let project = ProjectRepo::get_by_id(db, "p1").await.unwrap().unwrap();
        assert_eq!(project.owner_id.as_deref(), Some("u_a"));
    }

    #[tokio::test]
    async fn reopened_sqlite_keeps_the_single_canonical_owner_result() {
        let harness = file_service().await;
        let db: &SqliteDb = harness.service.db.as_ref();
        seed_user(db, "u_a", "a@test.com").await;
        seed_user(db, "u_b", "b@test.com").await;
        seed_project_with_owners(db, "p1", "u_a", &["u_b"]).await;

        let first = harness.service.clone();
        let second = harness.service.clone();
        let (left, right) = tokio::join!(
            first.remove_member("p1", "u_a", "u_a"),
            second.remove_member("p1", "u_b", "u_b"),
        );
        assert!(left.is_ok() ^ right.is_ok());

        harness.service.db.pool().close().await;
        let pool = create_sqlite_pool(&format!("sqlite://{}", harness.path.display()))
            .await
            .unwrap();
        run_migrations(&pool).await.unwrap();
        let db = SqliteDb::new(pool);
        let owners = ProjectMemberRepo::list_members(&db, "p1")
            .await
            .unwrap()
            .into_iter()
            .filter(|member| member.role == "owner")
            .collect::<Vec<_>>();
        assert_eq!(owners.len(), 1);
        let project = ProjectRepo::get_by_id(&db, "p1").await.unwrap().unwrap();
        assert_eq!(
            project.owner_id.as_deref(),
            Some(owners[0].user_id.as_str())
        );
    }

    #[tokio::test]
    async fn failed_last_owner_removal_rolls_back_without_partial_changes() {
        let svc = test_service().await;
        let db = &svc.db;
        seed_user(db, "u_a", "a@test.com").await;
        seed_project_with_owners(db, "p1", "u_a", &[]).await;

        let before = ProjectRepo::get_by_id(&**db, "p1").await.unwrap().unwrap();
        let result = svc.remove_member("p1", "u_a", "u_a").await;
        assert!(matches!(
            result,
            Err(ServiceError::Conflict(message)) if message == "last_owner"
        ));

        let after = ProjectRepo::get_by_id(&**db, "p1").await.unwrap().unwrap();
        assert_eq!(after.owner_id, before.owner_id);
        assert_eq!(after.version, before.version);
        assert!(ProjectMemberRepo::get_member(&**db, "p1", "u_a")
            .await
            .unwrap()
            .is_some());
    }
}
