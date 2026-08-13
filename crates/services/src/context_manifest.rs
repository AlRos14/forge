use std::sync::Arc;

use db::{
    now_rfc3339, ContextManifest, ContextManifestSource, CreateContextManifest,
    CreateContextManifestSource, ScopedMemoryRepository, SqliteDb,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{Result, ServiceError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextManifestInput {
    pub id: Uuid,
    pub identity_id: Uuid,
    pub agent_session_id: Option<Uuid>,
    pub context_scope_id: Uuid,
    pub scope_type: String,
    pub scope_id: String,
    pub policy_revision: String,
    pub domain_revision: String,
    pub lcm_binding_revision: Option<String>,
    pub runtime_manifest_id: Option<String>,
    pub runtime_manifest_fingerprint: Option<String>,
    pub request_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextSourceInput {
    pub ordinal: i64,
    pub source_id: String,
    pub source_type: String,
    pub source_revision: String,
    pub selection_reason: String,
    pub disposition: String,
    pub retention_priority: i64,
    pub fragment_fingerprint: String,
    pub sensitivity: String,
}

#[derive(Clone)]
pub struct ContextManifestService<R = SqliteDb> {
    db: Arc<R>,
}

impl<R> ContextManifestService<R>
where
    R: ScopedMemoryRepository + Send + Sync,
{
    pub fn new(db: Arc<R>) -> Self {
        Self { db }
    }

    pub async fn create(
        &self,
        input: ContextManifestInput,
        offered_sources: &[ContextSourceInput],
    ) -> Result<ContextManifest> {
        validate_manifest_input(&input)?;
        if offered_sources
            .iter()
            .any(|source| source.sensitivity.eq_ignore_ascii_case("secret"))
        {
            return Err(ServiceError::invalid_operation(
                "secret content cannot enter a context manifest",
            ));
        }
        for source in offered_sources {
            validate_manifest_source(source)?;
        }
        let combined_fingerprint = combined_fingerprint(&input, offered_sources);
        self.db
            .create_context_manifest(CreateContextManifest {
                id: input.id.to_string(),
                identity_id: input.identity_id.to_string(),
                agent_session_id: input.agent_session_id.map(|id| id.to_string()),
                context_scope_id: input.context_scope_id.to_string(),
                scope_type: input.scope_type,
                scope_id: input.scope_id,
                policy_revision: input.policy_revision,
                domain_revision: input.domain_revision,
                lcm_binding_revision: input.lcm_binding_revision,
                runtime_manifest_id: input.runtime_manifest_id,
                runtime_manifest_fingerprint: input.runtime_manifest_fingerprint,
                combined_fingerprint,
                request_fingerprint: input.request_fingerprint,
                created_at: now_rfc3339(),
            })
            .await
            .map_err(Into::into)
    }

    pub async fn append_source(
        &self,
        manifest_id: Uuid,
        source: ContextSourceInput,
    ) -> Result<ContextManifestSource> {
        if source.sensitivity.eq_ignore_ascii_case("secret") {
            return Err(ServiceError::invalid_operation(
                "secret content cannot enter a context manifest",
            ));
        }
        validate_manifest_source(&source)?;
        if !matches!(
            source.disposition.as_str(),
            "offered" | "included" | "summarized" | "omitted" | "deduplicated" | "rejected"
        ) {
            return Err(ServiceError::invalid_operation(
                "invalid context manifest disposition",
            ));
        }
        self.db
            .append_context_manifest_source(CreateContextManifestSource {
                manifest_id: manifest_id.to_string(),
                ordinal: source.ordinal,
                source_id: source.source_id,
                source_type: source.source_type,
                source_revision: source.source_revision,
                selection_reason: source.selection_reason,
                disposition: source.disposition,
                retention_priority: source.retention_priority,
                fragment_fingerprint: source.fragment_fingerprint,
            })
            .await
            .map_err(Into::into)
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<ContextManifest>> {
        self.db
            .get_context_manifest(&id.to_string())
            .await
            .map_err(Into::into)
    }

    pub async fn get_authorized(
        &self,
        id: Uuid,
        identity_id: Uuid,
        context_scope_id: Uuid,
    ) -> Result<Option<ContextManifest>> {
        self.db
            .get_context_manifest_scoped(
                &id.to_string(),
                &identity_id.to_string(),
                &context_scope_id.to_string(),
            )
            .await
            .map_err(Into::into)
    }

    pub async fn list_authorized(
        &self,
        identity_id: Uuid,
        context_scope_id: Option<Uuid>,
        limit: u32,
    ) -> Result<Vec<ContextManifest>> {
        let context_scope_id = context_scope_id.map(|id| id.to_string());
        self.db
            .list_context_manifests_scoped(
                &identity_id.to_string(),
                context_scope_id.as_deref(),
                i64::from(limit.clamp(1, 100)),
            )
            .await
            .map_err(Into::into)
    }

    pub async fn sources(&self, id: Uuid) -> Result<Vec<ContextManifestSource>> {
        self.db
            .list_context_manifest_sources(&id.to_string())
            .await
            .map_err(Into::into)
    }
}

pub fn fragment_fingerprint(source_id: &str, source_revision: &str, body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_id.as_bytes());
    hasher.update([0]);
    hasher.update(source_revision.as_bytes());
    hasher.update([0]);
    hasher.update(body.as_bytes());
    hex::encode(hasher.finalize())
}

fn validate_manifest_input(input: &ContextManifestInput) -> Result<()> {
    guard_manifest_field("policy_revision", &input.policy_revision, 256)?;
    guard_manifest_field("domain_revision", &input.domain_revision, 256)?;
    if let Some(value) = input.lcm_binding_revision.as_deref() {
        guard_manifest_field("lcm_binding_revision", value, 512)?;
    }
    if let Some(value) = input.runtime_manifest_id.as_deref() {
        guard_manifest_field("runtime_manifest_id", value, 512)?;
    }
    if let Some(value) = input.runtime_manifest_fingerprint.as_deref() {
        guard_manifest_field("runtime_manifest_fingerprint", value, 512)?;
    }
    guard_manifest_field("request_fingerprint", &input.request_fingerprint, 512)?;
    guard_manifest_field("scope_type", &input.scope_type, 64)?;
    guard_manifest_field("scope_id", &input.scope_id, 256)?;
    Ok(())
}

fn validate_manifest_source(source: &ContextSourceInput) -> Result<()> {
    guard_manifest_field("source_id", &source.source_id, 512)?;
    guard_manifest_field("source_type", &source.source_type, 128)?;
    guard_manifest_field("source_revision", &source.source_revision, 512)?;
    guard_manifest_field("selection_reason", &source.selection_reason, 4 * 1024)?;
    guard_manifest_field("disposition", &source.disposition, 64)?;
    guard_manifest_field("fragment_fingerprint", &source.fragment_fingerprint, 512)?;
    Ok(())
}

fn guard_manifest_field(name: &str, value: &str, max_len: usize) -> Result<()> {
    if value.len() > max_len {
        return Err(ServiceError::invalid_operation(format!(
            "context manifest {name} exceeds the {max_len}-byte limit"
        )));
    }
    let lower = value.to_ascii_lowercase();
    if lower.contains("authorization: bearer")
        || lower.contains("api_key")
        || lower.contains("sk-")
        || lower.contains("private key")
        || lower.contains("-----begin")
    {
        return Err(ServiceError::invalid_operation(format!(
            "protected values cannot be stored in context manifest {name}"
        )));
    }
    Ok(())
}

fn combined_fingerprint(input: &ContextManifestInput, sources: &[ContextSourceInput]) -> String {
    let mut canonical = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        input.identity_id,
        input.context_scope_id,
        input.scope_type,
        input.scope_id,
        input.policy_revision,
        input.domain_revision,
        input.lcm_binding_revision.as_deref().unwrap_or_default(),
        input.runtime_manifest_id.as_deref().unwrap_or_default(),
        input
            .runtime_manifest_fingerprint
            .as_deref()
            .unwrap_or_default(),
        input.request_fingerprint,
    );
    let mut ordered = sources.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        left.ordinal
            .cmp(&right.ordinal)
            .then_with(|| left.source_id.cmp(&right.source_id))
    });
    for source in ordered {
        canonical.push_str(&format!(
            "\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            source.ordinal,
            source.source_id,
            source.source_revision,
            source.selection_reason,
            source.disposition,
            source.fragment_fingerprint,
            source.sensitivity,
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    hex::encode(hasher.finalize())
}
