//! Protected Agent Runtime questionnaire broker.
//!
//! Request and answer bodies are encrypted in `protected_interaction`; only
//! redaction-safe lifecycle metadata is exposed to Forge's ordinary surfaces.

use std::{
    fmt,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use agent_runtime::core::interaction::{
    InteractionBroker, InteractionOutcomeKind, InteractionReadiness, InteractionRequest,
    InteractionResponse, QuestionAnswer,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{AgentHostError, protected_store::SqliteProtectedRuntimeStore};

const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InteractionAnswerValue {
    Choice {
        question_id: String,
        choice_id: String,
    },
    FreeForm {
        question_id: String,
        value: String,
    },
}

impl fmt::Debug for InteractionAnswerValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Choice { .. } => f.write_str("InteractionAnswerValue::Choice"),
            Self::FreeForm { value, .. } => f
                .debug_struct("InteractionAnswerValue::FreeForm")
                .field("value_chars", &value.chars().count())
                .finish(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InteractionAnswer {
    pub request_id: String,
    pub expected_version: i64,
    pub values: Vec<InteractionAnswerValue>,
}

impl InteractionAnswer {
    pub fn new(id: impl Into<String>, version: i64, values: Vec<InteractionAnswerValue>) -> Self {
        Self {
            request_id: id.into(),
            expected_version: version,
            values,
        }
    }
}

impl fmt::Debug for InteractionAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InteractionAnswer")
            .field("request_id", &self.request_id)
            .field("expected_version", &self.expected_version)
            .field("value_count", &self.values.len())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectedInteractionSummary {
    pub id: String,
    pub session_id: String,
    pub interaction_kind: String,
    pub prompt_redacted: String,
    pub status: String,
    pub expires_at: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone)]
pub struct InteractionBrokerHandle {
    store: Arc<SqliteProtectedRuntimeStore>,
}

impl fmt::Debug for InteractionBrokerHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InteractionBrokerHandle")
            .finish_non_exhaustive()
    }
}

impl InteractionBrokerHandle {
    pub fn new(store: Arc<SqliteProtectedRuntimeStore>) -> Self {
        Self { store }
    }

    pub async fn list_pending_for_owner(
        &self,
        owner_user_id: &str,
        forge_session_id: &str,
    ) -> Result<Vec<ProtectedInteractionSummary>, AgentHostError> {
        let rows = sqlx::query(
            "SELECT p.id,p.session_id,p.interaction_kind,p.prompt_redacted,p.status,p.expires_at,
                    p.version,p.created_at,p.updated_at
             FROM protected_interaction p JOIN agent_session s ON s.id=p.session_id
             JOIN agent_identity i ON i.id=s.identity_id
             WHERE p.session_id=? AND i.owner_id=? AND p.status='pending'
             ORDER BY p.created_at,p.id",
        )
        .bind(forge_session_id)
        .bind(owner_user_id)
        .fetch_all(self.store.database().pool())
        .await
        .map_err(|_| AgentHostError::ProtectedPersistence)?;
        rows.into_iter().map(summary).collect()
    }

    pub async fn answer(
        &self,
        owner_user_id: &str,
        answer: InteractionAnswer,
    ) -> Result<ProtectedInteractionSummary, AgentHostError> {
        self.answer_with_session(owner_user_id, None, answer).await
    }

    /// Answer an interaction only when it belongs to the authenticated
    /// account's session.  The session check is kept in the broker so callers
    /// cannot accidentally widen the authority by preflighting a summary and
    /// then invoking the owner-only operation.
    pub async fn answer_for_session(
        &self,
        owner_user_id: &str,
        forge_session_id: &str,
        answer: InteractionAnswer,
    ) -> Result<ProtectedInteractionSummary, AgentHostError> {
        self.answer_with_session(owner_user_id, Some(forge_session_id), answer)
            .await
    }

    async fn answer_with_session(
        &self,
        owner_user_id: &str,
        forge_session_id: Option<&str>,
        answer: InteractionAnswer,
    ) -> Result<ProtectedInteractionSummary, AgentHostError> {
        let mut sql = "SELECT p.request_ciphertext,p.request_nonce,p.status,p.version,p.expires_at
             FROM protected_interaction p JOIN agent_session s ON s.id=p.session_id
             JOIN agent_identity i ON i.id=s.identity_id
             WHERE p.id=? AND i.owner_id=?"
            .to_owned();
        if forge_session_id.is_some() {
            sql.push_str(" AND p.session_id=?");
        }
        sql.push_str(" LIMIT 1");
        let mut query = sqlx::query(&sql)
            .bind(&answer.request_id)
            .bind(owner_user_id);
        if let Some(forge_session_id) = forge_session_id {
            query = query.bind(forge_session_id);
        }
        let row = query
            .fetch_optional(self.store.database().pool())
            .await
            .map_err(|_| AgentHostError::ProtectedPersistence)?
            .ok_or(AgentHostError::SessionNotFound)?;
        let status: String = row
            .try_get("status")
            .map_err(|_| AgentHostError::ProtectedPersistence)?;
        let version: i64 = row
            .try_get("version")
            .map_err(|_| AgentHostError::ProtectedPersistence)?;
        let expires: Option<String> = row
            .try_get("expires_at")
            .map_err(|_| AgentHostError::ProtectedPersistence)?;
        if status != "pending" || version != answer.expected_version {
            return Err(AgentHostError::Authority(
                "protected interaction is unavailable or version changed".into(),
            ));
        }
        if expired(expires.as_deref()) {
            self.expire(&answer.request_id, version).await?;
            return Err(AgentHostError::Authority(
                "protected interaction has expired".into(),
            ));
        }
        let ciphertext: Vec<u8> = row
            .try_get("request_ciphertext")
            .map_err(|_| AgentHostError::ProtectedPersistence)?;
        let nonce: Vec<u8> = row
            .try_get("request_nonce")
            .map_err(|_| AgentHostError::ProtectedPersistence)?;
        let request: InteractionRequest = serde_json::from_slice(
            &self
                .store
                .open_protected(&ciphertext, &nonce)
                .map_err(|_| AgentHostError::ProtectedPersistence)?,
        )
        .map_err(|_| AgentHostError::ProtectedPersistence)?;
        if request.id().as_str() != answer.request_id {
            return Err(AgentHostError::Authority(
                "protected interaction request identity mismatch".into(),
            ));
        }
        let response = InteractionResponse::answered(
            request.id().clone(),
            answer
                .values
                .into_iter()
                .map(InteractionAnswerValue::runtime)
                .collect(),
        );
        response.validate_for(&request).map_err(|_| {
            AgentHostError::Authority("protected interaction answer is invalid".into())
        })?;
        let plain =
            serde_json::to_vec(&response).map_err(|_| AgentHostError::ProtectedPersistence)?;
        let (ciphertext, nonce) = self
            .store
            .seal_protected(&plain)
            .map_err(|_| AgentHostError::ProtectedPersistence)?;
        let changed = sqlx::query(
            "UPDATE protected_interaction SET response_ciphertext=?,response_nonce=?,status='answered',
                    version=version+1,updated_at=? WHERE id=? AND status='pending' AND version=?",
        )
        .bind(ciphertext).bind(nonce).bind(db::now_rfc3339()).bind(&answer.request_id).bind(version)
        .execute(self.store.database().pool()).await
        .map_err(|_| AgentHostError::ProtectedPersistence)?;
        if changed.rows_affected() == 0 {
            return Err(AgentHostError::Authority(
                "protected interaction version changed".into(),
            ));
        }
        self.summary_with_scope(&answer.request_id, Some(owner_user_id), forge_session_id)
            .await
    }

    pub async fn cancel(
        &self,
        owner_user_id: &str,
        request_id: &str,
        version: i64,
    ) -> Result<ProtectedInteractionSummary, AgentHostError> {
        self.cancel_with_session(owner_user_id, None, request_id, version)
            .await
    }

    /// Cancel an interaction only when it belongs to the authenticated
    /// account's session.
    pub async fn cancel_for_session(
        &self,
        owner_user_id: &str,
        forge_session_id: &str,
        request_id: &str,
        version: i64,
    ) -> Result<ProtectedInteractionSummary, AgentHostError> {
        self.cancel_with_session(owner_user_id, Some(forge_session_id), request_id, version)
            .await
    }

    async fn cancel_with_session(
        &self,
        owner_user_id: &str,
        forge_session_id: Option<&str>,
        request_id: &str,
        version: i64,
    ) -> Result<ProtectedInteractionSummary, AgentHostError> {
        let mut sql =
            "UPDATE protected_interaction SET status='cancelled',version=version+1,updated_at=?
             WHERE id=? AND version=? AND status='pending' AND EXISTS (
               SELECT 1 FROM agent_session s JOIN agent_identity i ON i.id=s.identity_id
               WHERE s.id=protected_interaction.session_id AND i.owner_id=?"
                .to_owned();
        if forge_session_id.is_some() {
            sql.push_str(" AND s.id=?");
        }
        sql.push(')');
        let mut query = sqlx::query(&sql)
            .bind(db::now_rfc3339())
            .bind(request_id)
            .bind(version)
            .bind(owner_user_id);
        if let Some(forge_session_id) = forge_session_id {
            query = query.bind(forge_session_id);
        }
        let changed = query
            .execute(self.store.database().pool())
            .await
            .map_err(|_| AgentHostError::ProtectedPersistence)?;
        if changed.rows_affected() == 0 {
            return Err(AgentHostError::Authority(
                "protected interaction is unavailable or version changed".into(),
            ));
        }
        self.summary_with_scope(request_id, Some(owner_user_id), forge_session_id)
            .await
    }

    async fn summary_with_scope(
        &self,
        id: &str,
        owner: Option<&str>,
        forge_session_id: Option<&str>,
    ) -> Result<ProtectedInteractionSummary, AgentHostError> {
        let mut sql = "SELECT p.id,p.session_id,p.interaction_kind,p.prompt_redacted,p.status,p.expires_at,p.version,p.created_at,p.updated_at
                       FROM protected_interaction p JOIN agent_session s ON s.id=p.session_id JOIN agent_identity i ON i.id=s.identity_id WHERE p.id=?".to_owned();
        if owner.is_some() {
            sql.push_str(" AND i.owner_id=?");
        }
        if forge_session_id.is_some() {
            sql.push_str(" AND p.session_id=?");
        }
        let mut query = sqlx::query(&sql).bind(id);
        if let Some(owner) = owner {
            query = query.bind(owner);
        }
        if let Some(forge_session_id) = forge_session_id {
            query = query.bind(forge_session_id);
        }
        let row = query
            .fetch_optional(self.store.database().pool())
            .await
            .map_err(|_| AgentHostError::ProtectedPersistence)?
            .ok_or(AgentHostError::SessionNotFound)?;
        summary(row)
    }

    async fn expire(&self, id: &str, version: i64) -> Result<(), AgentHostError> {
        sqlx::query("UPDATE protected_interaction SET status='expired',version=version+1,updated_at=? WHERE id=? AND status='pending' AND version=?")
            .bind(db::now_rfc3339()).bind(id).bind(version).execute(self.store.database().pool()).await
            .map_err(|_| AgentHostError::ProtectedPersistence).map(|_| ())
    }

    async fn persist(&self, request: &InteractionRequest) -> Result<(), AgentHostError> {
        request.validate().map_err(|_| {
            AgentHostError::Authority("protected interaction request is invalid".into())
        })?;
        let session_id = self
            .store
            .forge_session_id_for_runtime(request.origin().session())
            .await?;
        let plain =
            serde_json::to_vec(request).map_err(|_| AgentHostError::ProtectedPersistence)?;
        let (ciphertext, nonce) = self
            .store
            .seal_protected(&plain)
            .map_err(|_| AgentHostError::ProtectedPersistence)?;
        let count = request.questionnaire_payload().questions().len();
        let prompt = format!(
            "protected questionnaire ({count} question{})",
            if count == 1 { "" } else { "s" }
        );
        let expires = request
            .deadline()
            .instant()
            .map(|value| value.as_millis().to_string());
        let now = db::now_rfc3339();
        sqlx::query("INSERT INTO protected_interaction (id,session_id,interaction_kind,prompt_redacted,request_ciphertext,request_nonce,request_fingerprint,status,expires_at,version,created_at,updated_at) VALUES (?,?,?,?,?,?,?,'pending',?,1,?,?) ON CONFLICT(id) DO NOTHING")
            .bind(request.id().as_str()).bind(session_id).bind("questionnaire").bind(prompt).bind(ciphertext).bind(nonce).bind(request.fingerprint().as_str()).bind(expires).bind(&now).bind(&now)
            .execute(self.store.database().pool()).await.map_err(|_| AgentHostError::ProtectedPersistence).map(|_| ())
    }

    async fn poll(
        &self,
        request: &InteractionRequest,
    ) -> Result<Option<InteractionResponse>, AgentHostError> {
        let row = sqlx::query("SELECT p.response_ciphertext,p.response_nonce,p.status,p.expires_at FROM protected_interaction p JOIN agent_session s ON s.id=p.session_id WHERE p.id=? AND s.runtime_session_id=? LIMIT 1")
            .bind(request.id().as_str()).bind(request.origin().session().as_str()).fetch_optional(self.store.database().pool()).await
            .map_err(|_| AgentHostError::ProtectedPersistence)?.ok_or(AgentHostError::SessionNotFound)?;
        let status: String = row
            .try_get("status")
            .map_err(|_| AgentHostError::ProtectedPersistence)?;
        match status.as_str() {
            "pending" => {
                let expires: Option<String> = row
                    .try_get("expires_at")
                    .map_err(|_| AgentHostError::ProtectedPersistence)?;
                if expired(expires.as_deref()) {
                    self.expire(request.id().as_str(), 1).await?;
                    Ok(Some(InteractionResponse::timed_out(request.id().clone())))
                } else {
                    Ok(None)
                }
            }
            "answered" => {
                let ciphertext: Vec<u8> = row
                    .try_get("response_ciphertext")
                    .map_err(|_| AgentHostError::ProtectedPersistence)?;
                let nonce: Vec<u8> = row
                    .try_get("response_nonce")
                    .map_err(|_| AgentHostError::ProtectedPersistence)?;
                let response: InteractionResponse = serde_json::from_slice(
                    &self
                        .store
                        .open_protected(&ciphertext, &nonce)
                        .map_err(|_| AgentHostError::ProtectedPersistence)?,
                )
                .map_err(|_| AgentHostError::ProtectedPersistence)?;
                response
                    .validate_for(request)
                    .map_err(|_| AgentHostError::ProtectedPersistence)?;
                Ok(Some(response))
            }
            "expired" => Ok(Some(InteractionResponse::timed_out(request.id().clone()))),
            "cancelled" => Ok(Some(InteractionResponse::cancelled(request.id().clone()))),
            _ => Ok(Some(InteractionResponse::unavailable(
                request.id().clone(),
                "protected interaction is unavailable",
            ))),
        }
    }

    async fn close_async(&self, id: &str, outcome: InteractionOutcomeKind) {
        let status = match outcome {
            InteractionOutcomeKind::TimedOut => "expired",
            InteractionOutcomeKind::Cancelled | InteractionOutcomeKind::Unavailable => "cancelled",
            InteractionOutcomeKind::Answered | InteractionOutcomeKind::Declined => return,
        };
        let _ = sqlx::query("UPDATE protected_interaction SET status=?,version=version+1,updated_at=? WHERE id=? AND status='pending'")
            .bind(status).bind(db::now_rfc3339()).bind(id).execute(self.store.database().pool()).await;
    }
}

#[async_trait]
impl InteractionBroker for InteractionBrokerHandle {
    fn readiness(&self) -> InteractionReadiness {
        InteractionReadiness::Ready
    }

    async fn interact(&self, request: &InteractionRequest) -> InteractionResponse {
        if self.persist(request).await.is_err() {
            return InteractionResponse::unavailable(
                request.id().clone(),
                "protected interaction persistence is unavailable",
            );
        }
        loop {
            match self.poll(request).await {
                Ok(Some(response)) => return response,
                Ok(None) => tokio::time::sleep(POLL_INTERVAL).await,
                Err(_) => {
                    return InteractionResponse::unavailable(
                        request.id().clone(),
                        "protected interaction persistence is unavailable",
                    );
                }
            }
        }
    }

    fn close(
        &self,
        id: &agent_runtime::core::ids::InteractionRequestId,
        outcome: InteractionOutcomeKind,
    ) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let broker = self.clone();
        let id = id.as_str().to_owned();
        handle.spawn(async move {
            broker.close_async(&id, outcome).await;
        });
    }
}

fn summary(row: sqlx::sqlite::SqliteRow) -> Result<ProtectedInteractionSummary, AgentHostError> {
    Ok(ProtectedInteractionSummary {
        id: row
            .try_get("id")
            .map_err(|_| AgentHostError::ProtectedPersistence)?,
        session_id: row
            .try_get("session_id")
            .map_err(|_| AgentHostError::ProtectedPersistence)?,
        interaction_kind: row
            .try_get("interaction_kind")
            .map_err(|_| AgentHostError::ProtectedPersistence)?,
        prompt_redacted: row
            .try_get("prompt_redacted")
            .map_err(|_| AgentHostError::ProtectedPersistence)?,
        status: row
            .try_get("status")
            .map_err(|_| AgentHostError::ProtectedPersistence)?,
        expires_at: row
            .try_get("expires_at")
            .map_err(|_| AgentHostError::ProtectedPersistence)?,
        version: row
            .try_get("version")
            .map_err(|_| AgentHostError::ProtectedPersistence)?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| AgentHostError::ProtectedPersistence)?,
        updated_at: row
            .try_get("updated_at")
            .map_err(|_| AgentHostError::ProtectedPersistence)?,
    })
}

impl InteractionAnswerValue {
    fn runtime(self) -> QuestionAnswer {
        match self {
            Self::Choice {
                question_id,
                choice_id,
            } => QuestionAnswer::choice(question_id.into(), choice_id.into()),
            Self::FreeForm { question_id, value } => {
                QuestionAnswer::free_form(question_id.into(), value)
            }
        }
    }
}

fn expired(value: Option<&str>) -> bool {
    let Some(value) = value else { return false };
    let Ok(deadline) = value.parse::<u64>() else {
        return true;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or(u64::MAX);
    now >= deadline
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_answer_content() {
        let answer = InteractionAnswer::new(
            "request",
            1,
            vec![InteractionAnswerValue::FreeForm {
                question_id: "secret-question".into(),
                value: "private answer".into(),
            }],
        );
        let output = format!("{answer:?} {:?}", answer.values[0]);
        assert!(!output.contains("private answer"));
        assert!(!output.contains("secret-question"));
        assert!(output.contains("value_count"));
    }
}
