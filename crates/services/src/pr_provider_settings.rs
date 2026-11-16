//! Authority-checked writes of a repo's PR provider credential reference and
//! frozen-at-admission feedback policy (plan nodes A3/P1, «Congelado 6/7»).
//!
//! Both writes advance the config revision; admissions already issued keep
//! the credential and policy they froze, so changing either never widens an
//! existing admission.

use crate::credential_service::CredentialService;
use crate::{Result, ServiceError};
use db::{
    now_rfc3339, PrFeedbackPolicy, PrProviderConfig, PrProviderConfigRepo, PrProviderCredentialRef,
    ProjectRepo, RepoRepo, SqliteDb,
};
use std::sync::Arc;

const ALLOWED_ASSOCIATIONS: [&str; 6] = [
    "OWNER",
    "MEMBER",
    "COLLABORATOR",
    "CONTRIBUTOR",
    "FIRST_TIME_CONTRIBUTOR",
    "NONE",
];
const MAX_TRUSTED_LOGINS: usize = 32;
const MAX_LOGIN_CHARS: usize = 128;

#[derive(Clone)]
pub struct PrProviderSettingsService {
    db: Arc<SqliteDb>,
    credentials: Arc<CredentialService>,
}

impl PrProviderSettingsService {
    pub fn new(db: Arc<SqliteDb>, credentials: Arc<CredentialService>) -> Self {
        Self { db, credentials }
    }

    /// Reference (`Some`) or clear (`None`) the GitHub credential of the
    /// repo's PR provider config. Requires Project owner/admin and that the
    /// caller owns the (configured, `github`) handle.
    pub async fn set_credential(
        &self,
        actor_user_id: &str,
        repo_id: &str,
        credential_id: Option<&str>,
    ) -> Result<PrProviderConfig> {
        let config = self.require_admin_config(actor_user_id, repo_id).await?;
        let reference = match credential_id {
            None => None,
            Some(credential_id) => {
                let handle = self
                    .credentials
                    .require_owned_entry(actor_user_id, credential_id)
                    .await?;
                if handle.provider != "github" || handle.credential_method != "api_key" {
                    return Err(ServiceError::invalid_operation(
                        "credential must be a GitHub API key",
                    ));
                }
                Some(PrProviderCredentialRef {
                    handle_id: handle.id,
                    owner_user_id: handle.owner_user_id,
                })
            }
        };
        Ok(
            PrProviderConfigRepo::set_credential(&*self.db, &config.id, reference, &now_rfc3339())
                .await?,
        )
    }

    /// Replace the feedback policy. Requires Project owner/admin.
    pub async fn set_feedback_policy(
        &self,
        actor_user_id: &str,
        repo_id: &str,
        expected_updated_at: Option<&str>,
        policy: PrFeedbackPolicy,
    ) -> Result<PrProviderConfig> {
        let policy = validate_policy(policy)?;
        let config = self.require_admin_config(actor_user_id, repo_id).await?;
        Ok(PrProviderConfigRepo::set_feedback_policy(
            &*self.db,
            &config.id,
            expected_updated_at,
            &policy.to_json(),
            &now_rfc3339(),
        )
        .await?)
    }

    async fn require_admin_config(
        &self,
        actor_user_id: &str,
        repo_id: &str,
    ) -> Result<PrProviderConfig> {
        let repo = RepoRepo::get_by_id(&*self.db, repo_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("repo", repo_id.to_owned()))?;
        let project = ProjectRepo::get_by_id(&*self.db, &repo.project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", repo.project_id.clone()))?;
        if !crate::project_authority::user_can_administer_project(&self.db, &project, actor_user_id)
            .await?
        {
            return Err(ServiceError::AuthorizationDenied {
                message: "Project owner or admin role is required".to_owned(),
            });
        }
        PrProviderConfigRepo::get_by_repo_id(&*self.db, repo_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("pr_provider_config", repo_id.to_owned()))
    }
}

/// Validate and normalise a policy (schema 1). Associations are deduplicated
/// and logins are trimmed, deduplicated case-insensitively and kept in order.
pub fn validate_policy(mut policy: PrFeedbackPolicy) -> Result<PrFeedbackPolicy> {
    if policy.schema_version != 1 {
        return Err(ServiceError::invalid_operation(
            "feedback policy schema_version must be 1",
        ));
    }
    if !(1..=100).contains(&policy.max_items_per_poll) {
        return Err(ServiceError::invalid_operation(
            "max_items_per_poll must be between 1 and 100",
        ));
    }
    let mut associations: Vec<String> = Vec::new();
    for association in &policy.trusted_author_associations {
        if !ALLOWED_ASSOCIATIONS.contains(&association.as_str()) {
            return Err(ServiceError::invalid_operation(
                "trusted_author_associations contains an unsupported value",
            ));
        }
        if !associations.contains(association) {
            associations.push(association.clone());
        }
    }
    policy.trusted_author_associations = associations;
    let mut logins: Vec<String> = Vec::new();
    for login in &policy.trusted_logins {
        let login = login.trim();
        if login.is_empty() || login.chars().count() > MAX_LOGIN_CHARS {
            return Err(ServiceError::invalid_operation(
                "trusted_logins entries must be 1..=128 characters",
            ));
        }
        if !logins.iter().any(|known| known.eq_ignore_ascii_case(login)) {
            logins.push(login.to_owned());
        }
    }
    if logins.len() > MAX_TRUSTED_LOGINS {
        return Err(ServiceError::invalid_operation(
            "trusted_logins holds at most 32 entries",
        ));
    }
    policy.trusted_logins = logins;
    Ok(policy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential_service::{ConnectScmCredential, Secret};
    use crate::task_integration_operation::tests::fixture;
    use db::CreatePrProviderConfig;
    use tempfile::TempDir;

    struct Setup {
        _temp: TempDir,
        db: Arc<SqliteDb>,
        settings: PrProviderSettingsService,
        credentials: Arc<CredentialService>,
        repo_id: String,
        owner: String,
    }

    async fn setup() -> Setup {
        let temp = TempDir::new().expect("temporary directory");
        let url = format!("sqlite://{}", temp.path().join("settings.db").display());
        let fixture = fixture(&url, temp.path()).await;
        PrProviderConfigRepo::create(
            &*fixture.db,
            CreatePrProviderConfig {
                id: db::new_uuid_v4(),
                repo_id: fixture.repo_id.clone(),
                provider_type: "github".to_owned(),
                base_url: None,
                polling_interval_seconds: 60,
                token_secret_ref: None,
                credential_handle_id: None,
                credential_owner_user_id: None,
                created_at: now_rfc3339(),
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("PR provider config");
        let credentials = Arc::new(CredentialService::new(
            Arc::clone(&fixture.db),
            b"settings-test-key",
        ));
        Setup {
            _temp: temp,
            settings: PrProviderSettingsService::new(
                Arc::clone(&fixture.db),
                Arc::clone(&credentials),
            ),
            db: Arc::clone(&fixture.db),
            credentials,
            repo_id: fixture.repo_id.clone(),
            owner: fixture.user_id.clone(),
        }
    }

    async fn github_handle(setup: &Setup, owner: &str) -> String {
        setup
            .credentials
            .connect_scm_credential(ConnectScmCredential {
                owner_user_id: owner.to_owned(),
                provider: "github".to_owned(),
                label: "pat".to_owned(),
                credential: Secret::new("ghp_settings".to_owned()),
            })
            .await
            .expect("github handle")
            .id
    }

    async fn second_user(setup: &Setup, role: Option<&str>) -> String {
        let id = db::new_uuid_v4();
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO user (id, email, password_hash, display_name, created_at, updated_at)
             VALUES (?, ?, 'x', NULL, ?, ?)",
        )
        .bind(&id)
        .bind(format!("{id}@example.test"))
        .bind(&now)
        .bind(&now)
        .execute(setup.db.pool())
        .await
        .expect("user");
        if let Some(role) = role {
            sqlx::query(
                "INSERT INTO project_member (id, project_id, user_id, role, created_at, updated_at)
                 SELECT ?, project_id, ?, ?, ?, ? FROM repo WHERE id = ?",
            )
            .bind(db::new_uuid_v4())
            .bind(&id)
            .bind(role)
            .bind(&now)
            .bind(&now)
            .bind(&setup.repo_id)
            .execute(setup.db.pool())
            .await
            .expect("membership");
        }
        id
    }

    #[tokio::test]
    async fn scm_credential_reference_requires_admin_and_handle_ownership() {
        let s = setup().await;
        let handle = github_handle(&s, &s.owner).await;
        let config = s
            .settings
            .set_credential(&s.owner, &s.repo_id, Some(&handle))
            .await
            .expect("owner references own handle");
        assert_eq!(
            config.credential_handle_id.as_deref(),
            Some(handle.as_str())
        );
        assert_eq!(
            config.credential_owner_user_id.as_deref(),
            Some(s.owner.as_str())
        );

        // An admin cannot reference a handle owned by someone else.
        let admin = second_user(&s, Some("admin")).await;
        s.settings
            .set_credential(&admin, &s.repo_id, Some(&handle))
            .await
            .expect_err("handle owner must be the caller");
        let admin_handle = github_handle(&s, &admin).await;
        s.settings
            .set_credential(&admin, &s.repo_id, Some(&admin_handle))
            .await
            .expect("admin references own handle");

        // A plain member or outsider cannot change it at all.
        let member = second_user(&s, Some("member")).await;
        let member_handle = github_handle(&s, &member).await;
        s.settings
            .set_credential(&member, &s.repo_id, Some(&member_handle))
            .await
            .expect_err("members cannot administer");
        let outsider = second_user(&s, None).await;
        s.settings
            .set_credential(&outsider, &s.repo_id, None)
            .await
            .expect_err("outsiders cannot administer");

        // Model-provider handles are refused.
        let model = s
            .credentials
            .connect_api_key_credential(crate::credential_service::ConnectApiKeyCredential {
                owner_user_id: s.owner.clone(),
                provider: "openai".to_owned(),
                label: "model".to_owned(),
                credential: Secret::new("sk-model".to_owned()),
                base_url: Some("https://8.8.8.8/v1".to_owned()),
            })
            .await
            .expect("model handle");
        s.settings
            .set_credential(&s.owner, &s.repo_id, Some(&model.id))
            .await
            .expect_err("only GitHub handles may authenticate PR calls");

        let cleared = s
            .settings
            .set_credential(&s.owner, &s.repo_id, None)
            .await
            .expect("owner clears the reference");
        assert_eq!(cleared.credential_handle_id, None);
        assert_eq!(cleared.credential_owner_user_id, None);
    }

    #[tokio::test]
    async fn scm_credential_feedback_policy_is_validated_and_admin_only() {
        let s = setup().await;
        let initial = PrProviderConfigRepo::get_by_repo_id(&*s.db, &s.repo_id)
            .await
            .expect("load initial provider config")
            .expect("provider config exists");
        let mut policy = PrFeedbackPolicy {
            ingest: true,
            trusted_logins: vec![" Alice ".to_owned(), "alice".to_owned(), "bob".to_owned()],
            trusted_author_associations: vec!["OWNER".to_owned(), "OWNER".to_owned()],
            ..PrFeedbackPolicy::default()
        };
        let config = s
            .settings
            .set_feedback_policy(
                &s.owner,
                &s.repo_id,
                Some(&initial.updated_at),
                policy.clone(),
            )
            .await
            .expect("valid policy");
        assert_ne!(config.updated_at, initial.updated_at);
        assert!(matches!(
            s.settings
                .set_feedback_policy(
                    &s.owner,
                    &s.repo_id,
                    Some(&initial.updated_at),
                    PrFeedbackPolicy::default(),
                )
                .await,
            Err(ServiceError::Db(db::DbError::VersionConflict))
        ));
        let stored = PrFeedbackPolicy::from_json(&config.feedback_policy_json).expect("json");
        assert!(stored.ingest);
        assert_eq!(stored.trusted_logins, vec!["Alice", "bob"]);
        assert_eq!(stored.trusted_author_associations, vec!["OWNER"]);

        policy.max_items_per_poll = 0;
        s.settings
            .set_feedback_policy(&s.owner, &s.repo_id, None, policy.clone())
            .await
            .expect_err("cap below 1");
        policy.max_items_per_poll = 101;
        s.settings
            .set_feedback_policy(&s.owner, &s.repo_id, None, policy.clone())
            .await
            .expect_err("cap above 100");
        policy.max_items_per_poll = 10;
        policy.trusted_author_associations = vec!["STRANGER".to_owned()];
        s.settings
            .set_feedback_policy(&s.owner, &s.repo_id, None, policy.clone())
            .await
            .expect_err("unknown association");
        policy.trusted_author_associations = Vec::new();
        policy.trusted_logins = (0..33).map(|i| format!("user{i}")).collect();
        s.settings
            .set_feedback_policy(&s.owner, &s.repo_id, None, policy)
            .await
            .expect_err("too many trusted logins");

        let member = second_user(&s, Some("member")).await;
        s.settings
            .set_feedback_policy(&member, &s.repo_id, None, PrFeedbackPolicy::default())
            .await
            .expect_err("members cannot change the policy");
    }
}
