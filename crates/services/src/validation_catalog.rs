//! Versioned validation check catalog service (plan
//! coordinator-verification-and-pr-loop, node C1).
//!
//! Definitions are immutable rows owned by a Project (optionally one Repo). Only
//! an explicit Project owner/admin membership may publish or retire a version;
//! agents and the coordinator never write the catalog. A task pins exact
//! definition digests in its coordination envelope (v2) and every later lookup
//! is by exact `(check_key, definition_digest)`, never by "latest".

use std::sync::Arc;

use db::{
    new_uuid_v4, now_rfc3339, Page, PageRequest, ProjectRepo, PublishValidationCheckDefinition,
    RepoRepo, SqliteDb, ValidationCatalogRepo, ValidationCheckDefinition, ValidationCheckState,
};
use sqlx::Row;

use crate::{project_authority, Result, ServiceError};

const MAX_COMMAND_BYTES: usize = 8192;
const MAX_TIMEOUT_SECONDS: i64 = 3600;
const ENV_POLICY_CAPTURED_DEFAULT: &str = "captured_default";

pub(crate) fn validation_error(code: &'static str, message: impl Into<String>) -> ServiceError {
    ServiceError::Coordination {
        code,
        message: message.into(),
    }
}

/// Slug accepted by the catalog and by envelope pins: `[a-z0-9_-]{1,64}`.
pub(crate) fn is_valid_check_key(check_key: &str) -> bool {
    (1..=64).contains(&check_key.len())
        && check_key.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

/// Input of a new catalog version. `env_policy` defaults to `captured_default`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishCheckInput {
    pub repo_id: Option<String>,
    pub check_key: String,
    pub command: String,
    pub timeout_seconds: i64,
    pub env_policy: Option<String>,
}

#[derive(Clone)]
pub struct ValidationCatalogService {
    db: Arc<SqliteDb>,
}

impl ValidationCatalogService {
    pub fn new(db: Arc<SqliteDb>) -> Self {
        Self { db }
    }

    async fn authorize(
        &self,
        project_id: &str,
        user_id: &str,
        administer: bool,
    ) -> Result<db::Project> {
        let project = ProjectRepo::get_by_id(&*self.db, project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", project_id.to_owned()))?;
        if !project_authority::user_can_read_project(&self.db, &project, user_id).await? {
            return Err(ServiceError::not_found("project", project_id.to_owned()));
        }
        if administer
            && !project_authority::user_can_administer_project(&self.db, &project, user_id).await?
        {
            return Err(ServiceError::AuthorizationDenied {
                message: "only a Project owner or admin may change the validation catalog"
                    .to_owned(),
            });
        }
        Ok(project)
    }

    async fn require_repo_in_project(&self, project_id: &str, repo_id: &str) -> Result<()> {
        match RepoRepo::get_by_id(&*self.db, repo_id).await? {
            Some(repo) if repo.project_id == project_id => Ok(()),
            _ => Err(ServiceError::not_found("repo", repo_id.to_owned())),
        }
    }

    /// Publish the next exact version of `(project, repo, check_key)`. The
    /// previous active version is retired in the same transaction; existing
    /// envelope pins keep naming the old digest and become stale.
    pub async fn create_version(
        &self,
        project_id: &str,
        user_id: &str,
        input: PublishCheckInput,
    ) -> Result<ValidationCheckDefinition> {
        self.authorize(project_id, user_id, true).await?;
        if !is_valid_check_key(&input.check_key) {
            return Err(ServiceError::invalid_operation(
                "check_key must be a slug of 1 to 64 [a-z0-9_-] bytes",
            ));
        }
        if input.command.trim().is_empty() || input.command.len() > MAX_COMMAND_BYTES {
            return Err(ServiceError::invalid_operation(
                "validation command must contain between 1 and 8192 bytes",
            ));
        }
        if !(1..=MAX_TIMEOUT_SECONDS).contains(&input.timeout_seconds) {
            return Err(ServiceError::invalid_operation(
                "timeout_seconds must be between 1 and 3600",
            ));
        }
        let env_policy = input
            .env_policy
            .unwrap_or_else(|| ENV_POLICY_CAPTURED_DEFAULT.to_owned());
        if env_policy != ENV_POLICY_CAPTURED_DEFAULT {
            return Err(ServiceError::invalid_operation(
                "env_policy must be captured_default",
            ));
        }
        if let Some(repo_id) = input.repo_id.as_deref() {
            self.require_repo_in_project(project_id, repo_id).await?;
        }
        let now = now_rfc3339();
        Ok(ValidationCatalogRepo::publish_validation_check_definition(
            &*self.db,
            PublishValidationCheckDefinition {
                id: new_uuid_v4(),
                project_id: project_id.to_owned(),
                repo_id: input.repo_id,
                check_key: input.check_key,
                command: input.command,
                timeout_seconds: input.timeout_seconds,
                env_policy,
                created_by: user_id.to_owned(),
                created_at: now,
            },
        )
        .await?)
    }

    /// Retire one active definition. A definition that is already retired is a
    /// typed version conflict.
    pub async fn retire(
        &self,
        project_id: &str,
        user_id: &str,
        definition_id: &str,
    ) -> Result<ValidationCheckDefinition> {
        self.authorize(project_id, user_id, true).await?;
        let definition = self.get_in_project(project_id, definition_id).await?;
        Ok(ValidationCatalogRepo::retire_validation_check_definition(
            &*self.db,
            &definition.id,
            ValidationCheckState::Active,
            &now_rfc3339(),
        )
        .await?)
    }

    pub async fn get(
        &self,
        project_id: &str,
        user_id: &str,
        definition_id: &str,
    ) -> Result<ValidationCheckDefinition> {
        self.authorize(project_id, user_id, false).await?;
        self.get_in_project(project_id, definition_id).await
    }

    /// Explicit-state listing; there is no implicit "latest" read.
    pub async fn list(
        &self,
        project_id: &str,
        user_id: &str,
        repo_id: Option<&str>,
        state: Option<ValidationCheckState>,
        page: &PageRequest,
    ) -> Result<Page<ValidationCheckDefinition>> {
        self.authorize(project_id, user_id, false).await?;
        Ok(ValidationCatalogRepo::list_validation_check_definitions(
            &*self.db, project_id, repo_id, state, page,
        )
        .await?)
    }

    async fn get_in_project(
        &self,
        project_id: &str,
        definition_id: &str,
    ) -> Result<ValidationCheckDefinition> {
        ValidationCatalogRepo::get_validation_check_definition(&*self.db, definition_id)
            .await?
            .filter(|definition| definition.project_id == project_id)
            .ok_or_else(|| {
                ServiceError::not_found("validation check definition", definition_id.to_owned())
            })
    }
}

/// Resolve one exact pin inside a Task scope (`repo_id` of the Task, else the
/// Project-wide definition). Errors are typed:
/// `validation.check_unknown` (no such key/digest in scope) and
/// `validation.check_stale_pin` (retired, or the active version has another digest).
pub async fn resolve_pin(
    db: &SqliteDb,
    project_id: &str,
    repo_id: Option<&str>,
    check_key: &str,
    definition_digest: &str,
) -> Result<ValidationCheckDefinition> {
    let mut active_with_other_digest = false;
    let scopes: Vec<Option<&str>> = match repo_id {
        Some(repo_id) => vec![Some(repo_id), None],
        None => vec![None],
    };
    for scope in scopes {
        let Some(active) = ValidationCatalogRepo::get_active_validation_check_definition(
            db, project_id, scope, check_key,
        )
        .await?
        else {
            continue;
        };
        if active.definition_digest == definition_digest {
            return Ok(active);
        }
        active_with_other_digest = true;
    }
    if active_with_other_digest {
        return Err(validation_error(
            "validation.check_stale_pin",
            format!("check {check_key} has a newer active version than the pinned digest"),
        ));
    }
    let retired: Option<i64> = sqlx::query(
        "SELECT 1 FROM validation_check_definition
         WHERE project_id = ? AND check_key = ? AND definition_digest = ?
           AND (repo_id IS NULL OR repo_id IS ?) AND state = 'retired' LIMIT 1",
    )
    .bind(project_id)
    .bind(check_key)
    .bind(definition_digest)
    .bind(repo_id)
    .fetch_optional(db.pool())
    .await?
    .map(|row| row.try_get::<i64, _>(0))
    .transpose()?;
    if retired.is_some() {
        return Err(validation_error(
            "validation.check_stale_pin",
            format!("check {check_key} is retired"),
        ));
    }
    Err(validation_error(
        "validation.check_unknown",
        format!("check {check_key} with the pinned digest does not exist in this scope"),
    ))
}

#[cfg(test)]
pub(crate) mod test_support {
    use db::{
        create_sqlite_pool, run_migrations, CreateProject, CreateRepo, CreateTask,
        ProjectMemberRepo, ProjectRepo, RepoRepo, SqliteDb, TaskLifecycleState, TaskRepo, User,
        UserRepo,
    };

    pub(crate) struct Seed {
        pub db: std::sync::Arc<SqliteDb>,
        pub project_id: String,
        pub repo_id: String,
        pub owner_id: String,
    }

    pub(crate) async fn sqlite_db() -> SqliteDb {
        let pool = create_sqlite_pool("sqlite::memory:")
            .await
            .expect("pool creates");
        run_migrations(&pool).await.expect("migrations run");
        SqliteDb::new(pool)
    }

    pub(crate) async fn seed_user(db: &SqliteDb, is_admin: bool) -> String {
        let id = db::new_uuid_v4();
        let now = db::now_rfc3339();
        UserRepo::create_user(
            db,
            &User {
                id: id.clone(),
                email: format!("user-{id}@example.com"),
                password_hash: "test".to_owned(),
                display_name: None,
                is_admin,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("user creates");
        id
    }

    /// Project owned by a fresh user (owner membership), with one Repo.
    pub(crate) async fn seed_project(local_path: &str) -> Seed {
        let db = std::sync::Arc::new(sqlite_db().await);
        let owner_id = seed_user(&db, false).await;
        let now = db::now_rfc3339();
        let project_id = db::new_uuid_v4();
        ProjectRepo::create_with_owner_membership(
            &*db,
            CreateProject {
                id: project_id.clone(),
                name: "catalog".to_owned(),
                settings: "{}".to_owned(),
                primary_repo_id: None,
                owner_id: Some(owner_id.clone()),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
            &db::new_uuid_v4(),
        )
        .await
        .expect("project creates");
        let repo_id = db::new_uuid_v4();
        RepoRepo::create(
            &*db,
            CreateRepo {
                id: repo_id.clone(),
                project_id: project_id.clone(),
                name: "repo".to_owned(),
                remote_url: local_path.to_owned(),
                local_path: Some(local_path.to_owned()),
                work_mode: db::WorkMode::DirectMerge,
                default_branch: "main".to_owned(),
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("repo creates");
        Seed {
            db,
            project_id,
            repo_id,
            owner_id,
        }
    }

    pub(crate) async fn add_member(db: &SqliteDb, project_id: &str, user_id: &str, role: &str) {
        let now = db::now_rfc3339();
        ProjectMemberRepo::add_member(
            db,
            db::CreateProjectMember {
                id: db::new_uuid_v4(),
                project_id: project_id.to_owned(),
                user_id: user_id.to_owned(),
                role: role.to_owned(),
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("member adds");
    }

    pub(crate) async fn seed_task(db: &SqliteDb, project_id: &str, repo_id: &str) -> db::Task {
        TaskRepo::create(
            db,
            CreateTask {
                id: db::new_uuid_v4(),
                project_id: project_id.to_owned(),
                repo_id: Some(repo_id.to_owned()),
                parent_task_id: None,
                subtask_order: None,
                title: "catalog task".to_owned(),
                description: None,
                lifecycle_state: TaskLifecycleState::Active,
                is_automation: false,
                priority: 0,
                merge_config: None,
                created_at: db::now_rfc3339(),
                updated_at: db::now_rfc3339(),
            },
        )
        .await
        .expect("task creates")
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    fn input(key: &str, command: &str) -> PublishCheckInput {
        PublishCheckInput {
            repo_id: None,
            check_key: key.to_owned(),
            command: command.to_owned(),
            timeout_seconds: 60,
            env_policy: None,
        }
    }

    #[tokio::test]
    async fn validation_catalog_publish_requires_owner_or_admin_membership() {
        let seed = seed_project("/tmp/forge-catalog-auth").await;
        let service = ValidationCatalogService::new(Arc::clone(&seed.db));
        let member = seed_user(&seed.db, false).await;
        add_member(&seed.db, &seed.project_id, &member, "member").await;
        let admin = seed_user(&seed.db, false).await;
        add_member(&seed.db, &seed.project_id, &admin, "admin").await;
        let stranger = seed_user(&seed.db, false).await;

        let denied = service
            .create_version(&seed.project_id, &member, input("tests", "true"))
            .await
            .expect_err("a plain member cannot publish");
        assert!(matches!(denied, ServiceError::AuthorizationDenied { .. }));
        let hidden = service
            .create_version(&seed.project_id, &stranger, input("tests", "true"))
            .await
            .expect_err("a non-member sees no Project");
        assert!(matches!(hidden, ServiceError::NotFound { .. }));

        let owner_version = service
            .create_version(&seed.project_id, &seed.owner_id, input("tests", "true"))
            .await
            .expect("owner publishes");
        assert_eq!(owner_version.version, 1);
        let admin_version = service
            .create_version(&seed.project_id, &admin, input("lint", "true"))
            .await
            .expect("admin publishes");
        assert_eq!(admin_version.state, ValidationCheckState::Active);
        assert_eq!(admin_version.created_by, admin);
    }

    #[tokio::test]
    async fn validation_catalog_new_version_retires_previous_and_changes_digest() {
        let seed = seed_project("/tmp/forge-catalog-versions").await;
        let service = ValidationCatalogService::new(Arc::clone(&seed.db));
        let first = service
            .create_version(
                &seed.project_id,
                &seed.owner_id,
                input("tests", "cargo test"),
            )
            .await
            .expect("v1");
        let second = service
            .create_version(
                &seed.project_id,
                &seed.owner_id,
                input("tests", "cargo test -q"),
            )
            .await
            .expect("v2");
        assert_eq!((first.version, second.version), (1, 2));
        assert_ne!(first.definition_digest, second.definition_digest);
        let first_after = service
            .get(&seed.project_id, &seed.owner_id, &first.id)
            .await
            .expect("v1 history is kept");
        assert_eq!(first_after.state, ValidationCheckState::Retired);

        let active = service
            .list(
                &seed.project_id,
                &seed.owner_id,
                None,
                Some(ValidationCheckState::Active),
                &PageRequest {
                    cursor: None,
                    limit: 50,
                    include_total: true,
                    sort_by: db::SortBy::CreatedAt,
                    sort_order: db::SortOrder::Desc,
                },
            )
            .await
            .expect("active list");
        assert_eq!(active.items.len(), 1);
        assert_eq!(active.items[0].id, second.id);
    }

    #[tokio::test]
    async fn validation_catalog_retire_is_typed_and_validates_input() {
        let seed = seed_project("/tmp/forge-catalog-retire").await;
        let service = ValidationCatalogService::new(Arc::clone(&seed.db));
        for bad in [
            input("Bad Key", "true"),
            input("tests", "   "),
            PublishCheckInput {
                timeout_seconds: 3601,
                ..input("tests", "true")
            },
            PublishCheckInput {
                env_policy: Some("inherit".to_owned()),
                ..input("tests", "true")
            },
            PublishCheckInput {
                repo_id: Some("missing-repo".to_owned()),
                ..input("tests", "true")
            },
        ] {
            assert!(service
                .create_version(&seed.project_id, &seed.owner_id, bad)
                .await
                .is_err());
        }
        let definition = service
            .create_version(&seed.project_id, &seed.owner_id, input("tests", "true"))
            .await
            .expect("publish");
        let retired = service
            .retire(&seed.project_id, &seed.owner_id, &definition.id)
            .await
            .expect("retire");
        assert_eq!(retired.state, ValidationCheckState::Retired);
        let again = service
            .retire(&seed.project_id, &seed.owner_id, &definition.id)
            .await
            .expect_err("already retired");
        assert!(matches!(
            again,
            ServiceError::Db(db::DbError::VersionConflict)
        ));
    }

    #[tokio::test]
    async fn validation_catalog_resolve_pin_is_exact_and_typed() {
        let seed = seed_project("/tmp/forge-catalog-pins").await;
        let service = ValidationCatalogService::new(Arc::clone(&seed.db));
        let first = service
            .create_version(
                &seed.project_id,
                &seed.owner_id,
                input("tests", "cargo test"),
            )
            .await
            .expect("v1");
        resolve_pin(
            &seed.db,
            &seed.project_id,
            Some(&seed.repo_id),
            "tests",
            &first.definition_digest,
        )
        .await
        .expect("exact active pin resolves");

        let unknown = resolve_pin(
            &seed.db,
            &seed.project_id,
            Some(&seed.repo_id),
            "missing",
            &first.definition_digest,
        )
        .await
        .expect_err("unknown key");
        assert!(matches!(
            unknown,
            ServiceError::Coordination {
                code: "validation.check_unknown",
                ..
            }
        ));

        let second = service
            .create_version(
                &seed.project_id,
                &seed.owner_id,
                input("tests", "cargo test -q"),
            )
            .await
            .expect("v2");
        let stale = resolve_pin(
            &seed.db,
            &seed.project_id,
            Some(&seed.repo_id),
            "tests",
            &first.definition_digest,
        )
        .await
        .expect_err("a later version never changes what a pin authorizes");
        assert!(matches!(
            stale,
            ServiceError::Coordination {
                code: "validation.check_stale_pin",
                ..
            }
        ));
        service
            .retire(&seed.project_id, &seed.owner_id, &second.id)
            .await
            .expect("retire v2");
        let retired = resolve_pin(
            &seed.db,
            &seed.project_id,
            Some(&seed.repo_id),
            "tests",
            &second.definition_digest,
        )
        .await
        .expect_err("retired pin");
        assert!(matches!(
            retired,
            ServiceError::Coordination {
                code: "validation.check_stale_pin",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn validation_catalog_envelope_validate_pins_resolves_in_task_scope() {
        use crate::task_coordination::{
            CoordinationEnvelope, EnvelopePinContext, EnvelopeValidationCheck,
        };
        let seed = seed_project("/tmp/forge-catalog-envelope").await;
        let service = ValidationCatalogService::new(Arc::clone(&seed.db));
        let definition = service
            .create_version(&seed.project_id, &seed.owner_id, input("tests", "true"))
            .await
            .expect("publish");
        let envelope = CoordinationEnvelope {
            schema_version: 2,
            validation_checks: vec![EnvelopeValidationCheck {
                check_key: "tests".to_owned(),
                definition_digest: definition.definition_digest.clone(),
                trigger: db::EnvelopeCheckTrigger::OnRequest,
            }],
            ..CoordinationEnvelope::default()
        };
        envelope.validate().expect("structure");
        let ctx = EnvelopePinContext {
            db: &seed.db,
            project_id: &seed.project_id,
            repo_id: Some(&seed.repo_id),
        };
        envelope.validate_pins(&ctx).await.expect("exact pin");
        service
            .create_version(&seed.project_id, &seed.owner_id, input("tests", "false"))
            .await
            .expect("v2");
        assert!(envelope.validate_pins(&ctx).await.is_err());
    }
}
