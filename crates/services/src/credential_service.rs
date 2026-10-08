//! Deterministic provider credential ownership and protected persistence.
//!
//! Credentials are account-owned inputs to external HarnessAdapters. This
//! service stores encrypted credentials, supplies them in memory to their
//! configured consumers, and records provider connection health. It contains
//! no model or interaction loop.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use chacha20poly1305::{
    aead::{rand_core::RngCore, Aead, KeyInit, OsRng},
    XChaCha20Poly1305, XNonce,
};
use db::{CredentialHandle, CredentialHandleRepo, SqliteDb};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tokio::sync::Mutex;

use crate::{Result, ServiceError};

const MAX_PROVIDER_CREDENTIAL_RESPONSE_BYTES: usize = 1024 * 1024;
const CHATGPT_USAGE_ENDPOINT: &str = "https://chatgpt.com/backend-api/wham/usage";

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("provider credential was not found")]
    NotFound,
    #[error("provider credential changed before the operation completed")]
    VersionConflict,
    #[error("protected credential persistence failed")]
    Persistence,
    #[error("provider credential is unavailable")]
    Unavailable,
    #[error("provider credential refresh failed")]
    RefreshFailed,
}

#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Secret([redacted])")
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct OAuthCredentialBundle {
    pub schema_version: u32,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at_ms: u64,
    pub token_endpoint: String,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub provider_account_id: Option<String>,
}

impl std::fmt::Debug for OAuthCredentialBundle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OAuthCredentialBundle")
            .field("schema_version", &self.schema_version)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("token_endpoint", &self.token_endpoint)
            .field("scopes", &self.scopes)
            .field("provider_account_id", &self.provider_account_id)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct ConnectApiKeyCredential {
    pub owner_user_id: String,
    pub provider: String,
    pub label: String,
    pub credential: Secret,
    pub base_url: Option<String>,
}

#[derive(Clone)]
pub struct ConnectOAuthCredential {
    pub owner_user_id: String,
    pub provider: String,
    pub base_url: String,
    pub credential_label: String,
    pub credential: OAuthCredentialBundle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialRevocationOutcome {
    NotSupported,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderEntryTestOutcome {
    pub ok: bool,
    pub latency_ms: u64,
    pub message: Option<String>,
    pub checked_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderUsageWindowOutcome {
    pub id: String,
    pub used_percent: f64,
    pub window_minutes: Option<i64>,
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderUsageOutcome {
    pub provider: String,
    pub probed: bool,
    pub plan_type: Option<String>,
    pub windows: Vec<ProviderUsageWindowOutcome>,
    pub fetched_at: String,
    pub detail: Option<String>,
}

impl ProviderUsageOutcome {
    fn unsupported(provider: String, detail: impl Into<String>) -> Self {
        Self {
            provider,
            probed: false,
            plan_type: None,
            windows: Vec::new(),
            fetched_at: db::now_rfc3339(),
            detail: Some(detail.into()),
        }
    }
}

#[derive(Clone)]
struct StoredCredential {
    provider: String,
    method: String,
    version: i64,
    plaintext: String,
}

#[derive(Clone)]
struct CredentialStore {
    db: Arc<SqliteDb>,
    cipher: Arc<XChaCha20Poly1305>,
    key_revision: i64,
    refresh_locks: Arc<StdMutex<HashMap<String, Arc<Mutex<()>>>>>,
    oauth_client: reqwest::Client,
    revocation_client: reqwest::Client,
}

impl CredentialStore {
    fn new(db: Arc<SqliteDb>, master_key: [u8; 32], key_revision: i64) -> Self {
        Self {
            db,
            cipher: Arc::new(XChaCha20Poly1305::new((&master_key).into())),
            key_revision,
            refresh_locks: Arc::new(StdMutex::new(HashMap::new())),
            oauth_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(8))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("credential refresh client configuration is valid"),
            revocation_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("credential revocation client configuration is valid"),
        }
    }

    fn seal(&self, plaintext: &[u8]) -> std::result::Result<(Vec<u8>, Vec<u8>), CredentialError> {
        let mut nonce = [0_u8; 24];
        OsRng.fill_bytes(&mut nonce);
        let ciphertext = self
            .cipher
            .encrypt(XNonce::from_slice(&nonce), plaintext)
            .map_err(|_| CredentialError::Persistence)?;
        Ok((ciphertext, nonce.to_vec()))
    }

    fn open(
        &self,
        ciphertext: &[u8],
        nonce: &[u8],
    ) -> std::result::Result<Vec<u8>, CredentialError> {
        if nonce.len() != 24 {
            return Err(CredentialError::Persistence);
        }
        self.cipher
            .decrypt(XNonce::from_slice(nonce), ciphertext)
            .map_err(|_| CredentialError::Persistence)
    }

    async fn create_api_key(
        &self,
        id: &str,
        owner_user_id: &str,
        provider: &str,
        label: &str,
        secret: &Secret,
        now: &str,
    ) -> std::result::Result<CredentialHandle, CredentialError> {
        let (ciphertext, nonce) = self.seal(secret.expose().as_bytes())?;
        let mut transaction = self
            .db
            .pool()
            .begin()
            .await
            .map_err(|_| CredentialError::Persistence)?;
        sqlx::query(
            "INSERT INTO credential_handle (
                id, owner_user_id, provider, label, status,
                credential_method, metadata_json, version, created_at, updated_at
             ) VALUES (?, ?, ?, ?, 'configured', 'api_key', '{}', 1, ?, ?)",
        )
        .bind(id)
        .bind(owner_user_id)
        .bind(provider)
        .bind(label)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|_| CredentialError::Persistence)?;
        self.insert_ciphertext(&mut transaction, id, ciphertext, nonce, now)
            .await?;
        transaction
            .commit()
            .await
            .map_err(|_| CredentialError::Persistence)?;
        Ok(CredentialHandle {
            id: id.to_owned(),
            owner_user_id: owner_user_id.to_owned(),
            provider: provider.to_owned(),
            label: label.to_owned(),
            status: "configured".to_owned(),
            credential_method: "api_key".to_owned(),
            metadata_json: "{}".to_owned(),
            version: 1,
            created_at: now.to_owned(),
            updated_at: now.to_owned(),
        })
    }

    async fn create_oauth(
        &self,
        input: &ConnectOAuthCredential,
        id: &str,
        metadata_json: &str,
        now: &str,
    ) -> std::result::Result<CredentialHandle, CredentialError> {
        let plaintext =
            serde_json::to_vec(&input.credential).map_err(|_| CredentialError::Persistence)?;
        let (ciphertext, nonce) = self.seal(&plaintext)?;
        let mut transaction = self
            .db
            .pool()
            .begin()
            .await
            .map_err(|_| CredentialError::Persistence)?;
        sqlx::query(
            "INSERT INTO credential_handle (
                id, owner_user_id, provider, label, status,
                credential_method, metadata_json, version, created_at, updated_at
             ) VALUES (?, ?, ?, ?, 'configured', 'oauth_bundle', ?, 1, ?, ?)",
        )
        .bind(id)
        .bind(&input.owner_user_id)
        .bind(&input.provider)
        .bind(&input.credential_label)
        .bind(metadata_json)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|_| CredentialError::Persistence)?;
        self.insert_ciphertext(&mut transaction, id, ciphertext, nonce, now)
            .await?;
        transaction
            .commit()
            .await
            .map_err(|_| CredentialError::Persistence)?;
        Ok(CredentialHandle {
            id: id.to_owned(),
            owner_user_id: input.owner_user_id.clone(),
            provider: input.provider.clone(),
            label: input.credential_label.clone(),
            status: "configured".to_owned(),
            credential_method: "oauth_bundle".to_owned(),
            metadata_json: metadata_json.to_owned(),
            version: 1,
            created_at: now.to_owned(),
            updated_at: now.to_owned(),
        })
    }

    async fn insert_ciphertext(
        &self,
        transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        id: &str,
        ciphertext: Vec<u8>,
        nonce: Vec<u8>,
        now: &str,
    ) -> std::result::Result<(), CredentialError> {
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
        .execute(&mut **transaction)
        .await
        .map_err(|_| CredentialError::Persistence)?;
        Ok(())
    }

    async fn load_stored(
        &self,
        handle_id: &str,
        owner_user_id: &str,
    ) -> std::result::Result<StoredCredential, CredentialError> {
        let row = sqlx::query(
            "SELECT handle.provider, handle.credential_method, handle.version,
                    secret.ciphertext, secret.nonce
             FROM protected_credential_secret AS secret
             JOIN credential_handle AS handle ON handle.id = secret.handle_id
             WHERE handle.id = ? AND handle.owner_user_id = ?
               AND handle.status = 'configured'",
        )
        .bind(handle_id)
        .bind(owner_user_id)
        .fetch_optional(self.db.pool())
        .await
        .map_err(|_| CredentialError::Persistence)?
        .ok_or(CredentialError::Unavailable)?;
        let ciphertext: Vec<u8> = row
            .try_get("ciphertext")
            .map_err(|_| CredentialError::Persistence)?;
        let nonce: Vec<u8> = row
            .try_get("nonce")
            .map_err(|_| CredentialError::Persistence)?;
        let plaintext = self.open(&ciphertext, &nonce)?;
        Ok(StoredCredential {
            provider: row
                .try_get("provider")
                .map_err(|_| CredentialError::Persistence)?,
            method: row
                .try_get("credential_method")
                .map_err(|_| CredentialError::Persistence)?,
            version: row
                .try_get("version")
                .map_err(|_| CredentialError::Persistence)?,
            plaintext: String::from_utf8(plaintext).map_err(|_| CredentialError::Persistence)?,
        })
    }

    async fn acquire(
        &self,
        owner_user_id: &str,
        handle_id: &str,
        minimum_validity_ms: u64,
    ) -> std::result::Result<Secret, CredentialError> {
        let stored = self.load_stored(handle_id, owner_user_id).await?;
        if stored.method == "api_key" {
            return Ok(Secret::new(stored.plaintext));
        }
        let bundle: OAuthCredentialBundle =
            serde_json::from_str(&stored.plaintext).map_err(|_| CredentialError::RefreshFailed)?;
        if bundle.expires_at_ms > now_ms().saturating_add(minimum_validity_ms) {
            return Ok(Secret::new(bundle.access_token));
        }
        let lock = self.refresh_lock(handle_id);
        let _guard = lock.lock().await;
        let current = self.load_stored(handle_id, owner_user_id).await?;
        let mut bundle: OAuthCredentialBundle =
            serde_json::from_str(&current.plaintext).map_err(|_| CredentialError::RefreshFailed)?;
        if bundle.expires_at_ms > now_ms().saturating_add(minimum_validity_ms) {
            return Ok(Secret::new(bundle.access_token));
        }
        let mut form = vec![
            ("grant_type", "refresh_token".to_owned()),
            ("refresh_token", bundle.refresh_token.clone()),
            ("client_id", bundle.client_id.clone()),
        ];
        if let Some(client_secret) = bundle.client_secret.clone() {
            form.push(("client_secret", client_secret));
        }
        let response = self
            .oauth_client
            .post(&bundle.token_endpoint)
            .form(&form)
            .send()
            .await
            .map_err(|_| CredentialError::RefreshFailed)?;
        if !response.status().is_success() {
            if matches!(response.status().as_u16(), 400 | 401 | 403) {
                self.mark_invalid(handle_id, owner_user_id, current.version)
                    .await;
            }
            return Err(CredentialError::RefreshFailed);
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_PROVIDER_CREDENTIAL_RESPONSE_BYTES as u64)
        {
            return Err(CredentialError::RefreshFailed);
        }
        let body = response
            .bytes()
            .await
            .map_err(|_| CredentialError::RefreshFailed)?;
        if body.len() > MAX_PROVIDER_CREDENTIAL_RESPONSE_BYTES {
            return Err(CredentialError::RefreshFailed);
        }
        let refreshed: RefreshTokenResponse =
            serde_json::from_slice(&body).map_err(|_| CredentialError::RefreshFailed)?;
        if refreshed.access_token.trim().is_empty() || refreshed.expires_in == 0 {
            return Err(CredentialError::RefreshFailed);
        }
        bundle.access_token = refreshed.access_token;
        if let Some(refresh_token) = refreshed.refresh_token {
            bundle.refresh_token = refresh_token;
        }
        bundle.expires_at_ms = now_ms().saturating_add(refreshed.expires_in.saturating_mul(1000));
        if let Some(scope) = refreshed.scope {
            bundle.scopes = scope.split_whitespace().map(str::to_owned).collect();
        }
        self.rotate_oauth(handle_id, owner_user_id, current.version, &bundle)
            .await?;
        Ok(Secret::new(bundle.access_token))
    }

    async fn rotate_oauth(
        &self,
        handle_id: &str,
        owner_user_id: &str,
        expected_version: i64,
        bundle: &OAuthCredentialBundle,
    ) -> std::result::Result<(), CredentialError> {
        let plaintext = serde_json::to_vec(bundle).map_err(|_| CredentialError::RefreshFailed)?;
        let (ciphertext, nonce) = self.seal(&plaintext)?;
        let now = db::now_rfc3339();
        let mut transaction = self
            .db
            .pool()
            .begin()
            .await
            .map_err(|_| CredentialError::RefreshFailed)?;
        let updated = sqlx::query(
            "UPDATE credential_handle
             SET version = version + 1, status = 'configured', updated_at = ?
             WHERE id = ? AND owner_user_id = ? AND version = ?
               AND credential_method = 'oauth_bundle' AND status = 'configured'",
        )
        .bind(&now)
        .bind(handle_id)
        .bind(owner_user_id)
        .bind(expected_version)
        .execute(&mut *transaction)
        .await
        .map_err(|_| CredentialError::RefreshFailed)?;
        if updated.rows_affected() == 0 {
            return Err(CredentialError::RefreshFailed);
        }
        sqlx::query(
            "UPDATE protected_credential_secret
             SET ciphertext = ?, nonce = ?, key_revision = ?, updated_at = ?
             WHERE handle_id = ?",
        )
        .bind(ciphertext)
        .bind(nonce)
        .bind(self.key_revision)
        .bind(&now)
        .bind(handle_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| CredentialError::RefreshFailed)?;
        transaction
            .commit()
            .await
            .map_err(|_| CredentialError::RefreshFailed)?;
        Ok(())
    }

    async fn mark_invalid(&self, handle_id: &str, owner_user_id: &str, version: i64) {
        let _ = sqlx::query(
            "UPDATE credential_handle
             SET status = 'invalid', version = version + 1, updated_at = ?
             WHERE id = ? AND owner_user_id = ? AND version = ?",
        )
        .bind(db::now_rfc3339())
        .bind(handle_id)
        .bind(owner_user_id)
        .bind(version)
        .execute(self.db.pool())
        .await;
    }

    async fn seal_authorization_state(
        &self,
        operation_id: &str,
        plaintext: &[u8],
        now: &str,
    ) -> std::result::Result<(), CredentialError> {
        let (ciphertext, nonce) = self.seal(plaintext)?;
        sqlx::query(
            "INSERT INTO protected_provider_authorization_state (
                operation_id, ciphertext, nonce, key_revision, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(operation_id) DO UPDATE SET
                ciphertext = excluded.ciphertext, nonce = excluded.nonce,
                key_revision = excluded.key_revision, updated_at = excluded.updated_at",
        )
        .bind(operation_id)
        .bind(ciphertext)
        .bind(nonce)
        .bind(self.key_revision)
        .bind(now)
        .bind(now)
        .execute(self.db.pool())
        .await
        .map_err(|_| CredentialError::Persistence)?;
        Ok(())
    }

    async fn open_authorization_state(
        &self,
        operation_id: &str,
    ) -> std::result::Result<Vec<u8>, CredentialError> {
        let row = sqlx::query(
            "SELECT ciphertext, nonce FROM protected_provider_authorization_state
             WHERE operation_id = ?",
        )
        .bind(operation_id)
        .fetch_optional(self.db.pool())
        .await
        .map_err(|_| CredentialError::Persistence)?
        .ok_or(CredentialError::Persistence)?;
        let ciphertext: Vec<u8> = row
            .try_get("ciphertext")
            .map_err(|_| CredentialError::Persistence)?;
        let nonce: Vec<u8> = row
            .try_get("nonce")
            .map_err(|_| CredentialError::Persistence)?;
        self.open(&ciphertext, &nonce)
    }

    async fn delete_authorization_state(
        &self,
        operation_id: &str,
    ) -> std::result::Result<(), CredentialError> {
        sqlx::query("DELETE FROM protected_provider_authorization_state WHERE operation_id = ?")
            .bind(operation_id)
            .execute(self.db.pool())
            .await
            .map_err(|_| CredentialError::Persistence)?;
        Ok(())
    }

    async fn revoke_at_version(
        &self,
        handle_id: &str,
        owner_user_id: &str,
        expected_version: i64,
        now: &str,
    ) -> std::result::Result<CredentialRevocationOutcome, CredentialError> {
        let remote_bundle = self
            .remote_revocation_bundle(handle_id, owner_user_id)
            .await;
        let mut transaction = self
            .db
            .pool()
            .begin()
            .await
            .map_err(|_| CredentialError::Persistence)?;
        let updated = sqlx::query(
            "UPDATE credential_handle
             SET status = 'revoked', version = version + 1, updated_at = ?
             WHERE id = ? AND owner_user_id = ? AND version = ?",
        )
        .bind(now)
        .bind(handle_id)
        .bind(owner_user_id)
        .bind(expected_version)
        .execute(&mut *transaction)
        .await
        .map_err(|_| CredentialError::Persistence)?;
        if updated.rows_affected() == 0 {
            return Err(CredentialError::VersionConflict);
        }
        sqlx::query("DELETE FROM protected_credential_secret WHERE handle_id = ?")
            .bind(handle_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| CredentialError::Persistence)?;
        mark_credential_dependents_unavailable(&mut transaction, handle_id, now).await?;
        transaction
            .commit()
            .await
            .map_err(|_| CredentialError::Persistence)?;
        Ok(self.best_effort_remote_revocation(remote_bundle).await)
    }

    async fn remote_revocation_bundle(
        &self,
        handle_id: &str,
        owner_user_id: &str,
    ) -> Option<OAuthCredentialBundle> {
        let stored = self.load_stored(handle_id, owner_user_id).await.ok()?;
        if stored.method != "oauth_bundle" || stored.provider != "gemini" {
            return None;
        }
        serde_json::from_str(&stored.plaintext).ok()
    }

    async fn best_effort_remote_revocation(
        &self,
        bundle: Option<OAuthCredentialBundle>,
    ) -> CredentialRevocationOutcome {
        let Some(bundle) = bundle else {
            return CredentialRevocationOutcome::NotSupported;
        };
        match self
            .revocation_client
            .post("https://oauth2.googleapis.com/revoke")
            .form(&[("token", bundle.refresh_token)])
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                CredentialRevocationOutcome::Succeeded
            }
            _ => CredentialRevocationOutcome::Failed,
        }
    }

    fn refresh_lock(&self, handle_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self
            .refresh_locks
            .lock()
            .expect("credential refresh lock registry poisoned");
        Arc::clone(
            locks
                .entry(handle_id.to_owned())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    }
}

async fn mark_credential_dependents_unavailable(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    handle_id: &str,
    now: &str,
) -> std::result::Result<(), CredentialError> {
    sqlx::query(
        "UPDATE agent_connection_health
         SET status = 'unavailable', error_code = 'credential_revoked', updated_at = ?
         WHERE profile_id IN (SELECT id FROM agent_profile WHERE credential_ref = ?)",
    )
    .bind(now)
    .bind(handle_id)
    .execute(&mut **transaction)
    .await
    .map_err(|_| CredentialError::Persistence)?;
    sqlx::query(
        "UPDATE agent_session
         SET status = 'degraded', connection_status = 'unavailable',
             version = version + 1, updated_at = ?
         WHERE profile_id IN (SELECT id FROM agent_profile WHERE credential_ref = ?)
           AND status NOT IN ('replaced', 'terminated')",
    )
    .bind(now)
    .bind(handle_id)
    .execute(&mut **transaction)
    .await
    .map_err(|_| CredentialError::Persistence)?;
    Ok(())
}

#[derive(Deserialize)]
struct RefreshTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: u64,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Clone)]
pub struct CredentialService {
    db: Arc<SqliteDb>,
    store: CredentialStore,
}

impl std::fmt::Debug for CredentialService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CredentialService")
            .finish_non_exhaustive()
    }
}

impl CredentialService {
    pub fn new(db: Arc<SqliteDb>, key_material: &[u8]) -> Self {
        let digest = Sha256::digest(key_material);
        let mut master_key = [0_u8; 32];
        master_key.copy_from_slice(&digest);
        Self {
            store: CredentialStore::new(Arc::clone(&db), master_key, 1),
            db,
        }
    }

    pub async fn connect_api_key_credential(
        &self,
        input: ConnectApiKeyCredential,
    ) -> Result<CredentialHandle> {
        if input.label.trim().is_empty() || input.credential.expose().trim().is_empty() {
            return Err(ServiceError::invalid_operation(
                "label and credential are required",
            ));
        }
        if !matches!(
            input.provider.as_str(),
            "openai" | "xai" | "gemini" | "openai_compatible" | "openrouter"
        ) {
            return Err(ServiceError::invalid_operation(
                "provider is not supported for external HarnessAdapters",
            ));
        }
        let base_url = match input.base_url.as_deref().map(str::trim) {
            Some(value) if !value.is_empty() => value.to_owned(),
            _ => default_api_key_base_url(&input.provider)
                .ok_or_else(|| {
                    ServiceError::invalid_operation("base_url is required for this provider")
                })?
                .to_owned(),
        };
        provider_url_addresses(&base_url)
            .await
            .map_err(ServiceError::invalid_operation)?;
        let now = db::now_rfc3339();
        let id = db::new_uuid_v4();
        let handle = self
            .store
            .create_api_key(
                &id,
                &input.owner_user_id,
                &input.provider,
                &input.label,
                &input.credential,
                &now,
            )
            .await
            .map_err(credential_error_to_service)?;
        self.record_entry_base_url(&id, &base_url).await;
        Ok(CredentialHandle {
            metadata_json: json!({"base_url": base_url}).to_string(),
            ..handle
        })
    }

    pub async fn connect_oauth_credential(
        &self,
        input: ConnectOAuthCredential,
    ) -> Result<CredentialHandle> {
        if input.provider.trim().is_empty()
            || input.credential.access_token.trim().is_empty()
            || input.credential.refresh_token.trim().is_empty()
        {
            return Err(ServiceError::invalid_operation(
                "OAuth connection is missing required provider or token data",
            ));
        }
        if !matches!(input.provider.as_str(), "openai" | "xai" | "gemini") {
            return Err(ServiceError::invalid_operation(
                "provider does not support Forge-managed OAuth",
            ));
        }
        provider_url_addresses(&input.base_url)
            .await
            .map_err(ServiceError::invalid_operation)?;
        let now = db::now_rfc3339();
        let id = db::new_uuid_v4();
        let metadata_json = json!({
            "base_url": input.base_url.trim_end_matches('/'),
            "scopes": input.credential.scopes.clone(),
            "provider_account_id": input.credential.provider_account_id.clone(),
        })
        .to_string();
        self.store
            .create_oauth(&input, &id, &metadata_json, &now)
            .await
            .map_err(credential_error_to_service)
    }

    pub async fn require_owned_entry(
        &self,
        owner_user_id: &str,
        credential_id: &str,
    ) -> Result<CredentialHandle> {
        let handle = CredentialHandleRepo::get_credential_handle(&*self.db, credential_id)
            .await?
            .filter(|handle| handle.owner_user_id == owner_user_id)
            .ok_or_else(|| {
                ServiceError::not_found("credential_handle", credential_id.to_owned())
            })?;
        if handle.status != "configured" {
            return Err(ServiceError::invalid_operation(
                "provider entry is disconnected",
            ));
        }
        Ok(handle)
    }

    /// Resolve the credential identity frozen into an Execution or Agent Chat
    /// invocation snapshot and add its secret to the in-memory runtime env.
    /// The current Agent is consulted only as an ownership fence; its selected
    /// profile and credential are never used to choose the handle.
    pub async fn inject_snapshot_credential_env(&self, snapshot: &mut Value) -> Result<()> {
        let executor_type = snapshot
            .get("executor_type")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "executor config snapshot has no external HarnessAdapter type",
                )
            })?;
        let executor_kind = executor_type
            .parse::<executors::ExecutorKind>()
            .map_err(ServiceError::invalid_operation)?;
        let credential_ref = match snapshot.get("credential_ref") {
            None | Some(Value::Null) => return Ok(()),
            Some(Value::String(reference)) if !reference.trim().is_empty() => reference.clone(),
            Some(_) => {
                return Err(ServiceError::invalid_operation(
                    "snapshot credential reference is invalid",
                ));
            }
        };
        let agent_id = snapshot
            .get("agent_id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "credential-backed invocation snapshot has no Agent identity",
                )
            })?;
        let agent = db::AgentRepo::get_by_id(&*self.db, agent_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("agent_identity", agent_id.to_owned()))?;
        let handle = CredentialHandleRepo::get_credential_handle(&*self.db, &credential_ref)
            .await?
            .ok_or_else(|| {
                ServiceError::invalid_operation("snapshot credential entry is unavailable")
            })?;
        if agent.owner_id.as_deref() != Some(handle.owner_user_id.as_str()) {
            return Err(ServiceError::invalid_operation(
                "snapshot credential entry is not owned by the Agent account",
            ));
        }
        if handle.status != "configured" {
            return Err(ServiceError::invalid_operation(
                "snapshot credential entry is disconnected",
            ));
        }
        if handle.credential_method != "api_key" {
            return Err(ServiceError::invalid_operation(
                "snapshot credential cannot drive a CLI harness",
            ));
        }
        let snapshot_provider = snapshot
            .get("provider")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "credential-backed invocation snapshot has no provider identity",
                )
            })?;
        if snapshot_provider != handle.provider {
            return Err(ServiceError::invalid_operation(
                "snapshot credential provider does not match the frozen provider identity",
            ));
        }
        crate::provider_authorization::runtime_supported(
            &handle.provider,
            &handle.credential_method,
            &executor_kind.to_string(),
        )
        .map_err(ServiceError::invalid_operation)?;
        let variable = provider_env_variable(&handle.provider).ok_or_else(|| {
            ServiceError::invalid_operation("provider has no harness environment contract")
        })?;
        let secret = self
            .store
            .acquire(&handle.owner_user_id, &handle.id, 60_000)
            .await
            .map_err(credential_error_to_service)?;
        let env = snapshot
            .as_object_mut()
            .map(|snapshot| snapshot.entry("runtime_env").or_insert_with(|| json!({})))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                ServiceError::invalid_operation("executor config snapshot is not injectable")
            })?;
        env.insert(
            variable.to_owned(),
            Value::String(secret.expose().to_owned()),
        );
        Ok(())
    }

    pub async fn test_provider_entry(
        &self,
        owner_user_id: &str,
        credential_id: &str,
    ) -> Result<ProviderEntryTestOutcome> {
        let entry = self
            .require_owned_entry(owner_user_id, credential_id)
            .await?;
        let base_url = entry_base_url(&entry)?;
        provider_url_addresses(&base_url)
            .await
            .map_err(ServiceError::invalid_operation)?;
        let secret = self
            .store
            .acquire(owner_user_id, credential_id, 60_000)
            .await
            .map_err(credential_error_to_service)?;
        let base = base_url.trim_end_matches('/');
        let is_oauth = entry.credential_method == "oauth_bundle";
        let openai_oauth = is_oauth && entry.provider == "openai";
        let probe_url = match entry.provider.as_str() {
            "openrouter" => format!("{base}/key"),
            "openai" if openai_oauth => base.to_owned(),
            _ => format!("{base}/models"),
        };
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| ServiceError::invalid_operation("provider test client unavailable"))?;
        let request = if entry.provider == "gemini" && !is_oauth {
            client
                .get(&probe_url)
                .header("x-goog-api-key", secret.expose())
        } else {
            client.get(&probe_url).bearer_auth(secret.expose())
        };
        let started = std::time::Instant::now();
        let outcome = match request.send().await {
            Ok(response) => {
                let latency_ms = started.elapsed().as_millis() as u64;
                let status = response.status();
                if status.is_success() {
                    ProviderEntryTestOutcome {
                        ok: true,
                        latency_ms,
                        message: None,
                        checked_at: db::now_rfc3339(),
                    }
                } else if matches!(status.as_u16(), 401 | 403) {
                    ProviderEntryTestOutcome {
                        ok: false,
                        latency_ms,
                        message: Some(format!(
                            "provider rejected the credential (HTTP {})",
                            status.as_u16()
                        )),
                        checked_at: db::now_rfc3339(),
                    }
                } else if openai_oauth {
                    ProviderEntryTestOutcome {
                        ok: true,
                        latency_ms,
                        message: Some("endpoint reachable; authorization accepted".to_owned()),
                        checked_at: db::now_rfc3339(),
                    }
                } else {
                    ProviderEntryTestOutcome {
                        ok: false,
                        latency_ms,
                        message: Some(format!("provider returned HTTP {}", status.as_u16())),
                        checked_at: db::now_rfc3339(),
                    }
                }
            }
            Err(error) => {
                let reason = if error.is_timeout() {
                    "provider did not respond within 10 seconds"
                } else if error.is_connect() {
                    "provider could not be reached"
                } else {
                    "provider request failed before a response arrived"
                };
                ProviderEntryTestOutcome {
                    ok: false,
                    latency_ms: started.elapsed().as_millis() as u64,
                    message: Some(reason.to_owned()),
                    checked_at: db::now_rfc3339(),
                }
            }
        };
        Ok(outcome)
    }

    pub async fn usage_provider_entry(
        &self,
        owner_user_id: &str,
        credential_id: &str,
    ) -> Result<ProviderUsageOutcome> {
        let entry = self
            .require_owned_entry(owner_user_id, credential_id)
            .await?;
        let base_url = entry_base_url(&entry)?;
        let is_codex_oauth = entry.credential_method == "oauth_bundle"
            && entry.provider == "openai"
            && is_codex_backend(&base_url);
        if !is_codex_oauth {
            return Ok(ProviderUsageOutcome::unsupported(
                entry.provider,
                "usage probe not supported for this provider",
            ));
        }
        let secret = match self
            .store
            .acquire(owner_user_id, credential_id, 60_000)
            .await
        {
            Ok(secret) => secret,
            Err(_) => {
                return Ok(ProviderUsageOutcome::unsupported(
                    entry.provider,
                    "provider credential could not be refreshed",
                ))
            }
        };
        let account_id = entry_provider_account_id(&entry);
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
        {
            Ok(client) => client,
            Err(_) => {
                return Ok(ProviderUsageOutcome::unsupported(
                    entry.provider,
                    "usage probe client unavailable",
                ))
            }
        };
        let mut request = client
            .get(CHATGPT_USAGE_ENDPOINT)
            .header("originator", "codex_cli_rs")
            .bearer_auth(secret.expose());
        if let Some(account_id) = account_id.as_deref() {
            request = request.header("chatgpt-account-id", account_id);
        }
        Ok(match request.send().await {
            Ok(response) if response.status().is_success() => match response.text().await {
                Ok(body) => match usage_snapshot_from_wham_json(&body, now_unix_seconds()) {
                    Ok((plan_type, windows)) => ProviderUsageOutcome {
                        provider: entry.provider,
                        probed: true,
                        plan_type,
                        windows,
                        fetched_at: db::now_rfc3339(),
                        detail: None,
                    },
                    Err(_) => ProviderUsageOutcome::unsupported(
                        entry.provider,
                        "usage probe returned an unreadable response",
                    ),
                },
                Err(_) => ProviderUsageOutcome::unsupported(
                    entry.provider,
                    "usage probe response could not be read",
                ),
            },
            Ok(response) => ProviderUsageOutcome::unsupported(
                entry.provider,
                format!("usage probe returned HTTP {}", response.status().as_u16()),
            ),
            Err(error) => {
                let reason = if error.is_timeout() {
                    "usage probe did not respond within 10 seconds"
                } else if error.is_connect() {
                    "usage probe could not be reached"
                } else {
                    "usage probe request failed before a response arrived"
                };
                ProviderUsageOutcome::unsupported(entry.provider, reason)
            }
        })
    }

    pub async fn revoke_credential_at_version(
        &self,
        id: &str,
        owner: &str,
        version: i64,
        now: &str,
    ) -> std::result::Result<CredentialRevocationOutcome, CredentialError> {
        self.store.revoke_at_version(id, owner, version, now).await
    }

    pub async fn seal_authorization_state(
        &self,
        id: &str,
        plaintext: &[u8],
        now: &str,
    ) -> Result<()> {
        self.store
            .seal_authorization_state(id, plaintext, now)
            .await
            .map_err(credential_error_to_service)
    }

    pub async fn open_authorization_state(&self, id: &str) -> Result<Vec<u8>> {
        self.store
            .open_authorization_state(id)
            .await
            .map_err(credential_error_to_service)
    }

    pub async fn delete_authorization_state(&self, id: &str) -> Result<()> {
        self.store
            .delete_authorization_state(id)
            .await
            .map_err(credential_error_to_service)
    }

    async fn record_entry_base_url(&self, id: &str, base_url: &str) {
        let _ = sqlx::query("UPDATE credential_handle SET metadata_json = json_set(metadata_json, '$.base_url', ?) WHERE id = ?")
            .bind(base_url.trim_end_matches('/')).bind(id).execute(self.db.pool()).await;
    }
}

fn credential_error_to_service(error: CredentialError) -> ServiceError {
    match error {
        CredentialError::NotFound | CredentialError::Unavailable => {
            ServiceError::not_found("credential_handle", "unavailable")
        }
        CredentialError::VersionConflict => ServiceError::Db(db::DbError::VersionConflict),
        CredentialError::Persistence => {
            ServiceError::Domain("protected credential persistence failed".to_owned())
        }
        CredentialError::RefreshFailed => {
            ServiceError::Domain("provider credential could not be refreshed".to_owned())
        }
    }
}

fn provider_env_variable(provider: &str) -> Option<&'static str> {
    match provider {
        "openai" | "openai_compatible" => Some("OPENAI_API_KEY"),
        "gemini" => Some("GEMINI_API_KEY"),
        "xai" => Some("XAI_API_KEY"),
        "openrouter" => Some("OPENROUTER_API_KEY"),
        _ => None,
    }
}

pub fn default_api_key_base_url(provider: &str) -> Option<&'static str> {
    match provider {
        "openai" => Some("https://api.openai.com/v1"),
        "xai" => Some("https://api.x.ai/v1"),
        "gemini" => Some("https://generativelanguage.googleapis.com/v1beta"),
        "openrouter" => Some("https://openrouter.ai/api/v1"),
        _ => None,
    }
}

fn default_oauth_base_url(provider: &str) -> Option<&'static str> {
    match provider {
        "openai" => Some("https://chatgpt.com/backend-api/codex"),
        "xai" => Some("https://api.x.ai/v1"),
        "gemini" => Some("https://generativelanguage.googleapis.com/v1beta"),
        _ => None,
    }
}

fn is_codex_backend(base_url: &str) -> bool {
    base_url.contains("chatgpt.com/backend-api/codex")
}

pub fn entry_base_url(entry: &CredentialHandle) -> Result<String> {
    let stored = serde_json::from_str::<Value>(&entry.metadata_json)
        .ok()
        .and_then(|metadata| {
            metadata
                .get("base_url")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
    if let Some(base_url) = stored {
        return Ok(base_url);
    }
    let fallback = if entry.credential_method == "oauth_bundle" {
        default_oauth_base_url(&entry.provider)
    } else {
        default_api_key_base_url(&entry.provider)
    };
    fallback
        .map(str::to_owned)
        .ok_or_else(|| ServiceError::invalid_operation("provider entry has no usable API endpoint"))
}

pub fn entry_provider_account_id(entry: &CredentialHandle) -> Option<String> {
    serde_json::from_str::<Value>(&entry.metadata_json)
        .ok()
        .and_then(|metadata| {
            metadata
                .get("provider_account_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
}

#[derive(Debug, Deserialize)]
struct WhamUsageWindowWire {
    used_percent: Option<f64>,
    limit_window_seconds: Option<i64>,
    reset_after_seconds: Option<i64>,
    reset_at: Option<i64>,
}
#[derive(Debug, Deserialize)]
struct WhamUsageRateLimitWire {
    primary_window: Option<WhamUsageWindowWire>,
    secondary_window: Option<WhamUsageWindowWire>,
}
#[derive(Debug, Deserialize)]
struct WhamUsagePayloadWire {
    plan_type: Option<String>,
    rate_limit: Option<WhamUsageRateLimitWire>,
}

fn usage_snapshot_from_wham_json(
    body: &str,
    now: i64,
) -> std::result::Result<(Option<String>, Vec<ProviderUsageWindowOutcome>), serde_json::Error> {
    let payload: WhamUsagePayloadWire = serde_json::from_str(body)?;
    let mut windows = Vec::new();
    if let Some(rate_limit) = payload.rate_limit {
        for (id, window) in [
            ("primary", rate_limit.primary_window),
            ("secondary", rate_limit.secondary_window),
        ] {
            let Some(window) = window else { continue };
            let Some(used_percent) = window.used_percent.filter(|percent| percent.is_finite())
            else {
                continue;
            };
            let resets_at = window
                .reset_at
                .and_then(rfc3339_from_unix_seconds)
                .or_else(|| {
                    window
                        .reset_after_seconds
                        .filter(|seconds| *seconds > 0)
                        .and_then(|seconds| rfc3339_from_unix_seconds(now.saturating_add(seconds)))
                });
            windows.push(ProviderUsageWindowOutcome {
                id: id.to_owned(),
                used_percent,
                window_minutes: window.limit_window_seconds.map(|seconds| seconds / 60),
                resets_at,
            });
        }
    }
    Ok((payload.plan_type, windows))
}

fn rfc3339_from_unix_seconds(seconds: i64) -> Option<String> {
    chrono::DateTime::from_timestamp(seconds, 0).map(|instant| instant.to_rfc3339())
}
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
fn now_unix_seconds() -> i64 {
    chrono::Utc::now().timestamp()
}

async fn provider_url_addresses(
    base_url: &str,
) -> std::result::Result<(url::Url, Vec<SocketAddr>), &'static str> {
    let parsed = url::Url::parse(base_url).map_err(|_| "base_url must be an absolute URL")?;
    if parsed.username() != "" || parsed.password().is_some() || parsed.fragment().is_some() {
        return Err("base_url must not contain userinfo or a fragment");
    }
    if parsed.scheme() != "https" {
        return Err("base_url must use https");
    }
    let host = parsed.host().ok_or("base_url must include a host")?;
    let port = parsed
        .port_or_known_default()
        .ok_or("base_url has no known port")?;
    let addresses = match host {
        url::Host::Ipv4(address) => vec![SocketAddr::new(IpAddr::V4(address), port)],
        url::Host::Ipv6(address) => vec![SocketAddr::new(IpAddr::V6(address), port)],
        url::Host::Domain(domain) => {
            if restricted_provider_hostname(domain) {
                return Err("base_url must not target a local or private hostname");
            }
            tokio::net::lookup_host((domain, port))
                .await
                .map_err(|_| "base_url hostname could not be resolved")?
                .collect::<Vec<_>>()
        }
    };
    if addresses.is_empty()
        || addresses
            .iter()
            .any(|address| restricted_provider_ip(address.ip()))
    {
        return Err("base_url must not target a private or local address");
    }
    Ok((parsed, addresses))
}

fn restricted_provider_hostname(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".home.arpa")
}

fn restricted_provider_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(address) => {
            let octets = address.octets();
            address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || address.is_unspecified()
                || address.is_broadcast()
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
                || (octets[0] == 198 && (18..=19).contains(&octets[1]))
                || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
                || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113)
                || octets[0] >= 224
        }
        IpAddr::V6(address) => {
            let segments = address.segments();
            address.is_loopback()
                || address.is_unspecified()
                || (segments[0] & 0xffc0) == 0xfe80
                || (segments[0] & 0xffc0) == 0xfec0
                || (segments[0] & 0xfe00) == 0xfc00
                || segments[0] >= 0xff00
                || address
                    .to_ipv4()
                    .is_some_and(|mapped| restricted_provider_ip(IpAddr::V4(mapped)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::{
        create_sqlite_pool, run_migrations, AgentRepo, AgentStatus, CreateAgentIdentity,
        CreateAgentProfile, SelectAgentProfile,
    };
    use serde_json::json;

    #[tokio::test]
    async fn external_harness_credential_stays_encrypted_until_in_memory_injection() {
        let pool = create_sqlite_pool("sqlite::memory:")
            .await
            .expect("pool creates");
        run_migrations(&pool).await.expect("migrations run");
        let db = Arc::new(SqliteDb::new(pool));
        let now = db::now_rfc3339();
        sqlx::query(
            "INSERT INTO user (id, email, password_hash, display_name, created_at, updated_at)
             VALUES ('credential-owner', 'credential-owner@example.test', 'test', NULL, ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(db.pool())
        .await
        .expect("owner creates");

        let credentials = CredentialService::new(Arc::clone(&db), b"credential-service-test-key");
        let secret = "credential-secret-stays-out-of-database";
        let handle = credentials
            .connect_api_key_credential(ConnectApiKeyCredential {
                owner_user_id: "credential-owner".to_owned(),
                provider: "openai".to_owned(),
                label: "external harness key".to_owned(),
                credential: Secret::new(secret.to_owned()),
                base_url: Some("https://8.8.8.8/v1".to_owned()),
            })
            .await
            .expect("credential stores");

        let ciphertext: Vec<u8> = sqlx::query_scalar(
            "SELECT ciphertext FROM protected_credential_secret WHERE handle_id = ?",
        )
        .bind(&handle.id)
        .fetch_one(db.pool())
        .await
        .expect("credential ciphertext exists");
        assert!(!String::from_utf8_lossy(&ciphertext).contains(secret));

        let identity_id = "credential-harness-agent".to_owned();
        let profile_id = "credential-harness-profile-a".to_owned();
        AgentRepo::create_identity_with_profile(
            &*db,
            CreateAgentIdentity {
                id: identity_id.clone(),
                name: "External Harness Agent".to_owned(),
                description: None,
                max_concurrent_tasks: 1,
                heartbeat_interval_seconds: 30,
                max_missed_heartbeats: 3,
                status: AgentStatus::Idle,
                last_heartbeat_at: None,
                is_default: false,
                paused: false,
                owner_id: Some("credential-owner".to_owned()),
                visibility: "account".to_owned(),
                account_permission_ceiling: "{}".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
            CreateAgentProfile {
                id: profile_id.clone(),
                identity_id: identity_id.clone(),
                backend_kind: "cli".to_owned(),
                executor_type: "codex".to_owned(),
                provider: Some("openai".to_owned()),
                model: None,
                reasoning_effort: None,
                permission_policy: None,
                prompt_template: None,
                capabilities_json: "[]".to_owned(),
                tool_policy_json: "{}".to_owned(),
                config_json: "{}".to_owned(),
                credential_ref: Some(handle.id.clone()),
                daemon_id: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("external profile creates");

        let current_handle = credentials
            .connect_api_key_credential(ConnectApiKeyCredential {
                owner_user_id: "credential-owner".to_owned(),
                provider: "openai".to_owned(),
                label: "replacement account".to_owned(),
                credential: Secret::new("replacement-profile-secret".to_owned()),
                base_url: Some("https://8.8.8.8/v1".to_owned()),
            })
            .await
            .expect("replacement credential stores");
        let profile_b_id = "credential-harness-profile-b".to_owned();
        db::AgentProfileRepo::create_profile(
            &*db,
            CreateAgentProfile {
                id: profile_b_id.clone(),
                identity_id: identity_id.clone(),
                backend_kind: "cli".to_owned(),
                executor_type: "codex".to_owned(),
                provider: Some("openai".to_owned()),
                model: None,
                reasoning_effort: None,
                permission_policy: None,
                prompt_template: None,
                capabilities_json: "[]".to_owned(),
                tool_policy_json: "{}".to_owned(),
                config_json: "{}".to_owned(),
                credential_ref: Some(current_handle.id.clone()),
                daemon_id: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("replacement profile creates");
        let current_agent = AgentRepo::get_by_id(&*db, &identity_id)
            .await
            .expect("current Agent loads")
            .expect("current Agent exists");
        db::AgentProfileRepo::select_profile(
            &*db,
            SelectAgentProfile {
                identity_id: identity_id.clone(),
                profile_id: profile_b_id,
                expected_version: current_agent.version,
                updated_at: db::now_rfc3339(),
            },
        )
        .await
        .expect("current Agent profile changes");
        credentials
            .revoke_credential_at_version(
                &current_handle.id,
                "credential-owner",
                current_handle.version,
                &db::now_rfc3339(),
            )
            .await
            .expect("current profile credential revokes");

        let mut adapter_snapshot = json!({
            "executor_type": "codex",
            "agent_id": identity_id.clone(),
            "profile_id": profile_id.clone(),
            "credential_ref": handle.id.clone(),
            "provider": "openai",
            "config": {}
        });
        credentials
            .inject_snapshot_credential_env(&mut adapter_snapshot)
            .await
            .expect("credential injects into adapter snapshot");
        assert_eq!(
            adapter_snapshot["runtime_env"]["OPENAI_API_KEY"],
            json!(secret)
        );
        assert!(!adapter_snapshot
            .to_string()
            .contains("replacement-profile-secret"));

        let profile_c_id = "credential-harness-profile-c".to_owned();
        db::AgentProfileRepo::create_profile(
            &*db,
            CreateAgentProfile {
                id: profile_c_id.clone(),
                identity_id: identity_id.clone(),
                backend_kind: "cli".to_owned(),
                executor_type: "codex".to_owned(),
                provider: None,
                model: None,
                reasoning_effort: None,
                permission_policy: None,
                prompt_template: None,
                capabilities_json: "[]".to_owned(),
                tool_policy_json: "{}".to_owned(),
                config_json: "{}".to_owned(),
                credential_ref: None,
                daemon_id: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("uncredentialed profile creates");
        let current_agent = AgentRepo::get_by_id(&*db, &identity_id)
            .await
            .expect("current Agent reloads")
            .expect("current Agent exists");
        db::AgentProfileRepo::select_profile(
            &*db,
            SelectAgentProfile {
                identity_id: identity_id.clone(),
                profile_id: profile_c_id,
                expected_version: current_agent.version,
                updated_at: db::now_rfc3339(),
            },
        )
        .await
        .expect("current profile becomes uncredentialed");
        let mut snapshot_a = json!({
            "executor_type": "codex",
            "agent_id": identity_id.clone(),
            "profile_id": profile_id.clone(),
            "credential_ref": handle.id.clone(),
            "provider": "openai",
            "config": {}
        });
        credentials
            .inject_snapshot_credential_env(&mut snapshot_a)
            .await
            .expect("snapshot credential survives an uncredentialed current profile");
        assert_eq!(snapshot_a["runtime_env"]["OPENAI_API_KEY"], json!(secret));

        let mut ambient_snapshot = json!({
            "executor_type": "codex",
            "agent_id": identity_id.clone(),
            "credential_ref": null,
            "provider": "openai",
            "config": {}
        });
        credentials
            .inject_snapshot_credential_env(&mut ambient_snapshot)
            .await
            .expect("no explicit credential retains ambient semantics");
        assert!(ambient_snapshot.get("runtime_env").is_none());

        credentials
            .revoke_credential_at_version(
                &handle.id,
                "credential-owner",
                handle.version,
                &db::now_rfc3339(),
            )
            .await
            .expect("frozen credential revokes");
        let mut revoked_snapshot = json!({
            "executor_type": "codex",
            "agent_id": identity_id.clone(),
            "credential_ref": handle.id.clone(),
            "provider": "openai",
            "config": {}
        });
        let error = credentials
            .inject_snapshot_credential_env(&mut revoked_snapshot)
            .await
            .expect_err("revoked snapshot credential fails before invocation");
        assert!(error.to_string().contains("disconnected"));
        assert!(revoked_snapshot.get("runtime_env").is_none());

        let mut retired_snapshot = json!({
            "executor_type": "embedded",
            "agent_id": identity_id,
            "credential_ref": handle.id,
            "provider": "openai",
            "config": {}
        });
        let error = credentials
            .inject_snapshot_credential_env(&mut retired_snapshot)
            .await
            .expect_err("retired snapshots cannot receive provider credentials");
        assert!(error.to_string().contains("retired"));
        assert!(retired_snapshot.get("runtime_env").is_none());
    }
}
