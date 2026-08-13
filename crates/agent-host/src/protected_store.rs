use std::{collections::BTreeSet, fmt, sync::Arc};

use agent_runtime::core::{
    checkpoint::{CheckpointStore, TurnCheckpoint},
    error::{ErrorKind, RuntimeError},
    ids::SessionId,
    store::{Secret, SessionSnapshot, SessionStore},
};
use async_trait::async_trait;
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, OsRng, rand_core::RngCore},
};
use db::{CredentialHandle, SqliteDb};
use sqlx::Row;

#[derive(Clone)]
pub struct SqliteProtectedRuntimeStore {
    db: Arc<SqliteDb>,
    cipher: Arc<XChaCha20Poly1305>,
    key_revision: i64,
}

impl SqliteProtectedRuntimeStore {
    pub fn new(db: Arc<SqliteDb>, master_key: [u8; 32], key_revision: i64) -> Self {
        Self {
            db,
            cipher: Arc::new(XChaCha20Poly1305::new((&master_key).into())),
            key_revision,
        }
    }

    fn seal(&self, plaintext: &[u8]) -> Result<(Vec<u8>, Vec<u8>), RuntimeError> {
        let mut nonce = [0_u8; 24];
        OsRng.fill_bytes(&mut nonce);
        let ciphertext = self
            .cipher
            .encrypt(XNonce::from_slice(&nonce), plaintext)
            .map_err(|_| {
                RuntimeError::new(ErrorKind::Internal, "protected state encryption failed")
            })?;
        Ok((ciphertext, nonce.to_vec()))
    }

    fn open(&self, ciphertext: &[u8], nonce: &[u8]) -> Result<Vec<u8>, RuntimeError> {
        if nonce.len() != 24 {
            return Err(RuntimeError::new(
                ErrorKind::Serialization,
                "protected state nonce is invalid",
            ));
        }
        self.cipher
            .decrypt(XNonce::from_slice(nonce), ciphertext)
            .map_err(|_| {
                RuntimeError::new(
                    ErrorKind::Serialization,
                    "protected state could not be opened",
                )
            })
    }

    /// Internal protected-payload seam used by the interaction broker.  The
    /// bytes never cross into public profile/session/domain projections.
    pub(crate) fn seal_protected(
        &self,
        plaintext: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>), RuntimeError> {
        self.seal(plaintext)
    }

    /// Internal protected-payload seam used by the interaction broker.
    pub(crate) fn open_protected(
        &self,
        ciphertext: &[u8],
        nonce: &[u8],
    ) -> Result<Vec<u8>, RuntimeError> {
        self.open(ciphertext, nonce)
    }

    pub(crate) fn database(&self) -> Arc<SqliteDb> {
        Arc::clone(&self.db)
    }

    pub(crate) async fn forge_session_id_for_runtime(
        &self,
        runtime_id: &SessionId,
    ) -> Result<String, crate::AgentHostError> {
        self.forge_session_id(runtime_id)
            .await
            .map_err(|_| crate::AgentHostError::SessionNotFound)
    }

    async fn forge_session_id(&self, runtime_id: &SessionId) -> Result<String, RuntimeError> {
        sqlx::query_scalar::<_, String>("SELECT id FROM agent_session WHERE runtime_session_id = ?")
            .bind(runtime_id.as_str())
            .fetch_optional(self.db.pool())
            .await
            .map_err(|_| RuntimeError::internal("protected state lookup failed"))?
            .ok_or_else(|| RuntimeError::not_found("runtime session mapping not found"))
    }

    /// Loads the server-issued identity/scope binding for one runtime session.
    ///
    /// The optional Task workspace is joined by the exact host-supplied path;
    /// a path that is not the current persisted workspace is therefore
    /// rejected before RuntimeBuilder receives a filesystem-capable tool.
    pub(crate) async fn runtime_scope_binding(
        &self,
        forge_session_id: &str,
        runtime_session_id: &str,
        workspace_path: Option<&str>,
    ) -> Result<crate::RuntimeScopeBinding, crate::AgentHostError> {
        let row = sqlx::query(
            "SELECT session.identity_id,
                    identity.account_permission_ceiling,
                    identity.paused,
                    identity.archived_at,
                    profile.tool_policy_json,
                    scope.scope_type,
                    scope.scope_id,
                    scope.project_id,
                    scope.task_role,
                    scope.workspace_access,
                    chat.kind AS agent_chat_kind,
                    chat.project_id AS agent_chat_project_id,
                    binding.permission_ceiling_json AS binding_permission_ceiling,
                    workspace.worktree_path
             FROM agent_session AS session
             JOIN agent_identity AS identity
               ON identity.id = session.identity_id
             JOIN agent_profile AS profile
               ON profile.id = session.profile_id
             JOIN agent_context_scope AS scope
               ON scope.id = session.context_scope_id
             LEFT JOIN agent_chat AS chat
               ON scope.scope_type = 'agent_chat'
              AND chat.id = scope.scope_id
             LEFT JOIN project_agent_binding AS binding
               ON binding.project_id = CASE
                    WHEN scope.scope_type = 'agent_chat' THEN chat.project_id
                    ELSE scope.project_id
                  END
              AND binding.identity_id = session.identity_id
              AND binding.state = 'active'
             LEFT JOIN workspace
               ON workspace.task_id = scope.scope_id
              AND workspace.status IN ('creating', 'ready', 'error')
              AND workspace.worktree_path = ?
             WHERE session.id = ?
               AND session.runtime_session_id = ?
             LIMIT 1",
        )
        .bind(workspace_path)
        .bind(forge_session_id)
        .bind(runtime_session_id)
        .fetch_optional(self.db.pool())
        .await
        .map_err(|_| crate::AgentHostError::ProtectedPersistence)?
        .ok_or(crate::AgentHostError::SessionNotFound)?;

        let identity_id: String = row
            .try_get("identity_id")
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        let account_permission_ceiling: String = row
            .try_get("account_permission_ceiling")
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        let identity_paused: i64 = row
            .try_get("paused")
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        let identity_archived_at: Option<String> = row
            .try_get("archived_at")
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        let profile_tool_policy: String = row
            .try_get("tool_policy_json")
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        let scope_type: String = row
            .try_get("scope_type")
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        let scope_id: String = row
            .try_get("scope_id")
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        let project_id: Option<String> = row
            .try_get("project_id")
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        let agent_chat_kind: Option<String> = row
            .try_get("agent_chat_kind")
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        let agent_chat_project_id: Option<String> = row
            .try_get("agent_chat_project_id")
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        let task_role: Option<String> = row
            .try_get("task_role")
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        let workspace_access: String = row
            .try_get("workspace_access")
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        let persisted_workspace_path: Option<String> = row
            .try_get("worktree_path")
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        let binding_permission_ceiling: Option<String> = row
            .try_get("binding_permission_ceiling")
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        let scope_type = match scope_type.as_str() {
            "account" => crate::CanonicalScopeType::Account,
            "project" => crate::CanonicalScopeType::Project,
            "agent_chat" => crate::CanonicalScopeType::AgentChat,
            "task" => crate::CanonicalScopeType::Task,
            _ => {
                return Err(crate::AgentHostError::Authority(
                    "persisted canonical scope type is invalid".to_owned(),
                ));
            }
        };
        let workspace_access = match workspace_access.as_str() {
            "deny" => crate::WorkspaceAccess::Deny,
            "task_read" => crate::WorkspaceAccess::TaskRead,
            "task_write" => crate::WorkspaceAccess::TaskWrite,
            _ => {
                return Err(crate::AgentHostError::Authority(
                    "persisted workspace access is invalid".to_owned(),
                ));
            }
        };
        let scope = crate::CanonicalScope {
            scope_type,
            scope_id,
            workspace_access,
        };
        scope.validate()?;
        let agent_chat_project_id =
            if matches!(scope.scope_type, crate::CanonicalScopeType::AgentChat) {
                match agent_chat_kind.as_deref() {
                    Some("account_main") => {
                        if agent_chat_project_id.is_some() || project_id.is_some() {
                            return Err(crate::AgentHostError::Authority(
                                "Main Agent Chat has an invalid Project binding".to_owned(),
                            ));
                        }
                        None
                    }
                    Some("project") => {
                        let Some(chat_project_id) = agent_chat_project_id else {
                            return Err(crate::AgentHostError::Authority(
                                "Project Agent Chat has no owning Project".to_owned(),
                            ));
                        };
                        if project_id.as_deref() != Some(chat_project_id.as_str()) {
                            return Err(crate::AgentHostError::Authority(
                                "Project Agent Chat scope does not match its owning Project"
                                    .to_owned(),
                            ));
                        }
                        Some(chat_project_id)
                    }
                    _ => {
                        return Err(crate::AgentHostError::Authority(
                            "persisted Agent Chat kind is not admitted".to_owned(),
                        ));
                    }
                }
            } else {
                None
            };
        if matches!(scope.scope_type, crate::CanonicalScopeType::Task)
            && persisted_workspace_path.is_none()
        {
            return Err(crate::AgentHostError::Authority(
                "Task session has no active persisted workspace".to_owned(),
            ));
        }
        if !matches!(scope.scope_type, crate::CanonicalScopeType::Task)
            && persisted_workspace_path.is_some()
        {
            return Err(crate::AgentHostError::Authority(
                "non-Task session is bound to a workspace".to_owned(),
            ));
        }
        if identity_paused != 0 || identity_archived_at.is_some() {
            return Err(crate::AgentHostError::Authority(
                "native session identity is no longer active".to_owned(),
            ));
        }
        if project_id.is_some()
            && !matches!(scope.scope_type, crate::CanonicalScopeType::Task)
            && binding_permission_ceiling.is_none()
        {
            return Err(crate::AgentHostError::Authority(
                "native session Project authority is no longer active".to_owned(),
            ));
        }
        let mut allowed_permissions = permission_set(&account_permission_ceiling);
        intersect_permissions(
            &mut allowed_permissions,
            &permission_set(&profile_tool_policy),
        );
        intersect_permissions(
            &mut allowed_permissions,
            &scope_permission_set(
                scope.scope_type,
                scope.workspace_access,
                agent_chat_project_id.is_some(),
            ),
        );
        if let Some(binding_permissions) = binding_permission_ceiling {
            intersect_permissions(
                &mut allowed_permissions,
                &permission_set(&binding_permissions),
            );
        }
        Ok(crate::RuntimeScopeBinding {
            identity_id,
            scope,
            task_role,
            workspace_path: persisted_workspace_path,
            agent_chat_project_id,
            allowed_permissions,
        })
    }

    /// Resolves a replaceable runtime session to the stable identity/scope
    /// LCM timeline. The runtime id and canonical scope must both match the
    /// persisted Forge session; a timeline id alone cannot be used to open
    /// the store.
    pub async fn lcm_store_for_runtime_session(
        &self,
        runtime_id: &str,
        scope_type: &str,
        scope_id: &str,
    ) -> Result<crate::SqliteLcmStore, crate::AgentHostError> {
        let row = sqlx::query(
            "SELECT session.identity_id, scope.scope_type, scope.scope_id
             FROM agent_session AS session
             JOIN agent_context_scope AS scope
               ON scope.id = session.context_scope_id
             WHERE session.runtime_session_id = ?
               AND scope.scope_type = ? AND scope.scope_id = ?
             LIMIT 1",
        )
        .bind(runtime_id)
        .bind(scope_type)
        .bind(scope_id)
        .fetch_optional(self.db.pool())
        .await
        .map_err(|_| crate::AgentHostError::ProtectedPersistence)?
        .ok_or(crate::AgentHostError::SessionNotFound)?;
        let identity_id: String = row
            .try_get("identity_id")
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        let stored_scope_type: String = row
            .try_get("scope_type")
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        let stored_scope_id: String = row
            .try_get("scope_id")
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        let authorization_revision =
            agent_runtime::registry::RegistryRevision::from_content(format!(
                "forge-lcm-authorization-v1\n{identity_id}\n{stored_scope_type}\n{stored_scope_id}"
            ));
        crate::SqliteLcmStore::open_for_binding(
            Arc::clone(&self.db),
            &identity_id,
            &stored_scope_type,
            &stored_scope_id,
            authorization_revision.as_str(),
            &db::now_rfc3339(),
        )
        .await
    }

    pub async fn create_credential(
        &self,
        id: &str,
        owner_user_id: &str,
        provider: &str,
        label: &str,
        secret: Secret,
        now: &str,
    ) -> Result<CredentialHandle, crate::AgentHostError> {
        let (ciphertext, nonce) = self
            .seal(secret.expose().as_bytes())
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        let mut transaction = self
            .db
            .pool()
            .begin()
            .await
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        sqlx::query(
            "INSERT INTO credential_handle (
                id, owner_user_id, provider, label, status, created_at, updated_at
             ) VALUES (?, ?, ?, ?, 'configured', ?, ?)",
        )
        .bind(id)
        .bind(owner_user_id)
        .bind(provider)
        .bind(label)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        sqlx::query(
            "INSERT INTO protected_credential_secret (
                handle_id, ciphertext, nonce, key_revision, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(ciphertext)
        .bind(nonce)
        .bind(self.key_revision)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        transaction
            .commit()
            .await
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        Ok(CredentialHandle {
            id: id.to_owned(),
            owner_user_id: owner_user_id.to_owned(),
            provider: provider.to_owned(),
            label: label.to_owned(),
            status: "configured".to_owned(),
            created_at: now.to_owned(),
            updated_at: now.to_owned(),
        })
    }

    pub async fn load_credential(
        &self,
        handle_id: &str,
        owner_user_id: &str,
    ) -> Result<Secret, crate::AgentHostError> {
        let row = sqlx::query(
            "SELECT secret.ciphertext, secret.nonce
             FROM protected_credential_secret AS secret
             JOIN credential_handle AS handle ON handle.id = secret.handle_id
             WHERE handle.id = ? AND handle.owner_user_id = ? AND handle.status = 'configured'",
        )
        .bind(handle_id)
        .bind(owner_user_id)
        .fetch_optional(self.db.pool())
        .await
        .map_err(|_| crate::AgentHostError::ProtectedPersistence)?
        .ok_or(crate::AgentHostError::CredentialNotFound)?;
        let ciphertext: Vec<u8> = row
            .try_get("ciphertext")
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        let nonce: Vec<u8> = row
            .try_get("nonce")
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        let plaintext = self
            .open(&ciphertext, &nonce)
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        let value = String::from_utf8(plaintext)
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        Ok(Secret::new(value))
    }

    pub async fn revoke_credential(
        &self,
        handle_id: &str,
        owner_user_id: &str,
        now: &str,
    ) -> Result<(), crate::AgentHostError> {
        let mut transaction = self
            .db
            .pool()
            .begin()
            .await
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        let result = sqlx::query(
            "UPDATE credential_handle
             SET status = 'revoked', updated_at = ?
             WHERE id = ? AND owner_user_id = ?",
        )
        .bind(now)
        .bind(handle_id)
        .bind(owner_user_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        if result.rows_affected() == 0 {
            return Err(crate::AgentHostError::CredentialNotFound);
        }
        sqlx::query("DELETE FROM protected_credential_secret WHERE handle_id = ?")
            .bind(handle_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        transaction
            .commit()
            .await
            .map_err(|_| crate::AgentHostError::ProtectedPersistence)?;
        Ok(())
    }
}

fn permission_set(value: &str) -> BTreeSet<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(value) else {
        return BTreeSet::new();
    };
    match value {
        serde_json::Value::Array(values) => values
            .into_iter()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect(),
        serde_json::Value::Object(map) => map
            .get("permissions")
            .or_else(|| map.get("allowed"))
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect(),
        _ => BTreeSet::new(),
    }
}

fn intersect_permissions(target: &mut BTreeSet<String>, layer: &BTreeSet<String>) {
    *target = target.intersection(layer).cloned().collect();
}

fn scope_permission_set(
    scope_type: crate::CanonicalScopeType,
    workspace_access: crate::WorkspaceAccess,
    project_agent_chat: bool,
) -> BTreeSet<String> {
    let mut values: Vec<&str> = match scope_type {
        crate::CanonicalScopeType::Account => vec![
            "read_account",
            "propose_discovery",
            "propose_project",
            "propose_handoff",
            "propose_message",
            "propose_commitment",
            "propose_memory",
            "propose_session",
        ],
        crate::CanonicalScopeType::Project => vec![
            "read_project",
            "read_memory",
            "propose_task",
            "propose_message",
            "propose_commitment",
            "propose_memory",
            "propose_review",
            "propose_decision",
            "propose_session",
        ],
        crate::CanonicalScopeType::AgentChat => vec![
            "read_agent_chat",
            "read_memory",
            "propose_message",
            "propose_commitment",
            "propose_memory",
            "propose_session",
        ],
        crate::CanonicalScopeType::Task => match workspace_access {
            crate::WorkspaceAccess::TaskRead => {
                vec!["read_task", "read_memory", "task_read", "propose_review"]
            }
            crate::WorkspaceAccess::TaskWrite => {
                vec!["read_task", "read_memory", "task_read", "task_write"]
            }
            crate::WorkspaceAccess::Deny => vec![],
        },
    };
    if matches!(scope_type, crate::CanonicalScopeType::AgentChat) && project_agent_chat {
        values.push("propose_task");
    } else if matches!(scope_type, crate::CanonicalScopeType::AgentChat) {
        values.extend(["propose_discovery", "propose_project", "propose_handoff"]);
    }
    values.into_iter().map(str::to_owned).collect()
}

impl fmt::Debug for SqliteProtectedRuntimeStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SqliteProtectedRuntimeStore")
            .field("key_revision", &self.key_revision)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl SessionStore for SqliteProtectedRuntimeStore {
    async fn load(&self, id: &SessionId) -> Result<Option<SessionSnapshot>, RuntimeError> {
        let forge_session_id = match self.forge_session_id(id).await {
            Ok(value) => value,
            Err(error) if error.kind == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let row = sqlx::query(
            "SELECT snapshot_ciphertext, snapshot_nonce
             FROM protected_agent_session_state WHERE session_id = ?",
        )
        .bind(forge_session_id)
        .fetch_optional(self.db.pool())
        .await
        .map_err(|_| RuntimeError::internal("protected session load failed"))?;
        let Some(row) = row else {
            return Ok(None);
        };
        let ciphertext: Option<Vec<u8>> = row
            .try_get("snapshot_ciphertext")
            .map_err(|_| RuntimeError::internal("protected session row is invalid"))?;
        let nonce: Option<Vec<u8>> = row
            .try_get("snapshot_nonce")
            .map_err(|_| RuntimeError::internal("protected session row is invalid"))?;
        match (ciphertext, nonce) {
            (Some(ciphertext), Some(nonce)) => {
                let bytes = self.open(&ciphertext, &nonce)?;
                serde_json::from_slice(&bytes).map(Some).map_err(Into::into)
            }
            _ => Ok(None),
        }
    }

    async fn save(&self, snapshot: &SessionSnapshot) -> Result<(), RuntimeError> {
        let forge_session_id = self.forge_session_id(&snapshot.id).await?;
        let bytes = serde_json::to_vec(snapshot)?;
        let (ciphertext, nonce) = self.seal(&bytes)?;
        sqlx::query(
            "INSERT INTO protected_agent_session_state (
                session_id, snapshot_ciphertext, snapshot_nonce,
                key_revision, state_revision, updated_at
             ) VALUES (?, ?, ?, ?, 1, ?)
             ON CONFLICT(session_id) DO UPDATE SET
                snapshot_ciphertext = excluded.snapshot_ciphertext,
                snapshot_nonce = excluded.snapshot_nonce,
                key_revision = excluded.key_revision,
                state_revision = protected_agent_session_state.state_revision + 1,
                updated_at = excluded.updated_at",
        )
        .bind(forge_session_id)
        .bind(ciphertext)
        .bind(nonce)
        .bind(self.key_revision)
        .bind(db::now_rfc3339())
        .execute(self.db.pool())
        .await
        .map_err(|_| RuntimeError::internal("protected session save failed"))?;
        Ok(())
    }
}

#[async_trait]
impl CheckpointStore for SqliteProtectedRuntimeStore {
    async fn load_latest(
        &self,
        session: &SessionId,
    ) -> Result<Option<TurnCheckpoint>, RuntimeError> {
        let forge_session_id = match self.forge_session_id(session).await {
            Ok(value) => value,
            Err(error) if error.kind == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let row = sqlx::query(
            "SELECT checkpoint_ciphertext, checkpoint_nonce
             FROM protected_agent_session_state WHERE session_id = ?",
        )
        .bind(forge_session_id)
        .fetch_optional(self.db.pool())
        .await
        .map_err(|_| RuntimeError::internal("protected checkpoint load failed"))?;
        let Some(row) = row else {
            return Ok(None);
        };
        let ciphertext: Option<Vec<u8>> = row
            .try_get("checkpoint_ciphertext")
            .map_err(|_| RuntimeError::internal("protected checkpoint row is invalid"))?;
        let nonce: Option<Vec<u8>> = row
            .try_get("checkpoint_nonce")
            .map_err(|_| RuntimeError::internal("protected checkpoint row is invalid"))?;
        match (ciphertext, nonce) {
            (Some(ciphertext), Some(nonce)) => {
                let bytes = self.open(&ciphertext, &nonce)?;
                serde_json::from_slice(&bytes).map(Some).map_err(Into::into)
            }
            _ => Ok(None),
        }
    }

    async fn save(&self, checkpoint: &TurnCheckpoint) -> Result<(), RuntimeError> {
        let forge_session_id = self.forge_session_id(&checkpoint.session).await?;
        let bytes = serde_json::to_vec(checkpoint)?;
        let (ciphertext, nonce) = self.seal(&bytes)?;
        let fingerprint = checkpoint.operation_fingerprint.to_string();
        let result = sqlx::query(
            "INSERT INTO protected_agent_session_state (
                session_id, checkpoint_ciphertext, checkpoint_nonce,
                checkpoint_turn_id, checkpoint_revision, checkpoint_fingerprint,
                key_revision, state_revision, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, 1, ?)
             ON CONFLICT(session_id) DO UPDATE SET
                checkpoint_ciphertext = excluded.checkpoint_ciphertext,
                checkpoint_nonce = excluded.checkpoint_nonce,
                checkpoint_turn_id = excluded.checkpoint_turn_id,
                checkpoint_revision = excluded.checkpoint_revision,
                checkpoint_fingerprint = excluded.checkpoint_fingerprint,
                key_revision = excluded.key_revision,
                state_revision = protected_agent_session_state.state_revision + 1,
                updated_at = excluded.updated_at
             WHERE protected_agent_session_state.checkpoint_revision IS NULL
                OR excluded.checkpoint_turn_id != protected_agent_session_state.checkpoint_turn_id
                OR excluded.checkpoint_revision > protected_agent_session_state.checkpoint_revision
                OR (
                    excluded.checkpoint_revision = protected_agent_session_state.checkpoint_revision
                    AND excluded.checkpoint_fingerprint = protected_agent_session_state.checkpoint_fingerprint
                )",
        )
        .bind(forge_session_id)
        .bind(ciphertext)
        .bind(nonce)
        .bind(checkpoint.turn.as_str())
        .bind(i64::try_from(checkpoint.state_revision).unwrap_or(i64::MAX))
        .bind(fingerprint)
        .bind(self.key_revision)
        .bind(db::now_rfc3339())
        .execute(self.db.pool())
        .await
        .map_err(|_| RuntimeError::internal("protected checkpoint save failed"))?;
        if result.rows_affected() == 0 {
            return Err(RuntimeError::conflict(
                "protected checkpoint revision moved backwards or changed fingerprint",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_permission_intersection_fails_closed() {
        let mut effective = permission_set(r#"{"allowed":["read_project","task_write"]}"#);
        intersect_permissions(&mut effective, &permission_set(r#"["read_project"]"#));
        assert_eq!(effective, BTreeSet::from(["read_project".to_owned()]));

        let mut malformed = permission_set("not-json");
        intersect_permissions(
            &mut malformed,
            &scope_permission_set(
                crate::CanonicalScopeType::Project,
                crate::WorkspaceAccess::Deny,
                false,
            ),
        );
        assert!(malformed.is_empty());
    }

    #[test]
    fn task_scope_ceiling_is_the_only_filesystem_policy() {
        let task = scope_permission_set(
            crate::CanonicalScopeType::Task,
            crate::WorkspaceAccess::TaskWrite,
            false,
        );
        assert!(task.contains("task_read"));
        assert!(task.contains("task_write"));

        let project = scope_permission_set(
            crate::CanonicalScopeType::Project,
            crate::WorkspaceAccess::Deny,
            false,
        );
        assert!(!project.contains("task_read"));
        assert!(!project.contains("task_write"));
    }
}
