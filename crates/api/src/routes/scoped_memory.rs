use std::{collections::HashSet, sync::Arc};

use api_types::{
    ContextManifestListQuery, ContextManifestListResponse, ContextManifestQuery,
    ContextManifestResponse, ContextManifestSourceResponse, MemoryLifecycleRequest,
    MemoryLifecycleResponse, MemoryProvenanceQuery, MemoryProvenanceResponse,
    MemoryPublicationRequest,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use db::{
    AgentContextScopeRepo, AgentRepo, MemoryGetQuery, MemoryItem, MemoryLifecycleAssertion,
    MemoryScopeGrant, ProjectMemberRepo, ProjectRepo, ScopedMemoryRepository, TaskRepo,
};
use services::{
    ContextManifestService, MemoryAccessContext, MemoryLifecycleInput, MemoryPublicationInput,
};
use uuid::Uuid;

use crate::{
    errors::{ApiError, ApiResult},
    routes::auth::AuthenticatedUser,
    state::AppState,
};

const MAX_PROVENANCE_ASSERTIONS: usize = 200;
const MAX_MANIFEST_SOURCES: usize = 500;

/// Explicitly publish a private assertion into a server-authorized canonical
/// scope. The response is provenance-only so a publication endpoint cannot be
/// used as a content exfiltration surface.
pub async fn publish_memory(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(memory_id): Path<String>,
    Json(request): Json<MemoryPublicationRequest>,
) -> ApiResult<(StatusCode, Json<MemoryProvenanceResponse>)> {
    let source_id = parse_uuid(&memory_id, "memory_id")?;
    require_owned_identity(&state, &user, &request.actor_identity_id).await?;
    let source_grant = authorized_scope_grant(
        &state,
        &user,
        &request.source_scope_type,
        &request.source_scope_id,
    )
    .await?;
    let target_grant = authorized_scope_grant(
        &state,
        &user,
        &request.target_scope_type,
        &request.target_scope_id,
    )
    .await?;
    validate_target_visibility(&request.target_scope_type, &request.target_visibility)?;
    validate_target_linkage(&request)?;

    let access = MemoryAccessContext {
        identity_id: Some(request.actor_identity_id.clone()),
        grants: vec![source_grant, target_grant],
    };
    let published = state
        .memory_service
        .publish(
            &access,
            MemoryPublicationInput {
                source_id,
                source_scope_type: request.source_scope_type,
                source_scope_id: request.source_scope_id,
                target_scope_type: request.target_scope_type,
                target_scope_id: request.target_scope_id,
                target_project_id: request.target_project_id,
                target_task_id: request.target_task_id,
                target_visibility: request.target_visibility,
                target_authority: request.target_authority,
                actor_identity_id: request.actor_identity_id,
                reason: request.reason,
                evidence_json: request.evidence_json,
            },
        )
        .await?;
    let lifecycle = state
        .db
        .list_memory_lifecycle_assertions(&published.id)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(provenance_response(published, lifecycle)?),
    ))
}

/// Append an immutable lifecycle assertion. Destructive assertions are
/// owner-only in the service; shared actors may add disputed/evidence records.
pub async fn assert_memory_lifecycle(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(memory_id): Path<String>,
    Json(request): Json<MemoryLifecycleRequest>,
) -> ApiResult<Json<MemoryLifecycleResponse>> {
    let memory_id = parse_uuid(&memory_id, "memory_id")?;
    require_owned_identity(&state, &user, &request.actor_identity_id).await?;
    let grant =
        authorized_scope_grant(&state, &user, &request.scope_type, &request.scope_id).await?;
    let related_memory_id = request
        .related_memory_id
        .as_deref()
        .map(|value| parse_uuid(value, "related_memory_id"))
        .transpose()?;
    let assertion = state
        .memory_service
        .assert_lifecycle(
            &MemoryAccessContext {
                identity_id: Some(request.actor_identity_id.clone()),
                grants: vec![grant],
            },
            MemoryLifecycleInput {
                memory_id,
                assertion_type: request.assertion_type,
                related_memory_id,
                reason: request.reason,
                evidence_json: request.evidence_json,
                actor_identity_id: request.actor_identity_id,
            },
        )
        .await?;
    Ok(Json(lifecycle_response(assertion)))
}

/// Return metadata-only provenance after canonical scope authorization. The
/// underlying repository performs the ACL check before loading the row body;
/// this route then intentionally drops all content-bearing fields.
pub async fn get_memory_provenance(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(memory_id): Path<String>,
    Query(query): Query<MemoryProvenanceQuery>,
) -> ApiResult<Json<MemoryProvenanceResponse>> {
    let memory_id = parse_uuid(&memory_id, "memory_id")?;
    require_owned_identity(&state, &user, &query.identity_id).await?;
    let grant = authorized_scope_grant(&state, &user, &query.scope_type, &query.scope_id).await?;
    let mut item = state
        .db
        .get_memory_item_scoped(MemoryGetQuery {
            id: memory_id.to_string(),
            identity_id: Some(query.identity_id.clone()),
            grants: vec![grant.clone()],
            include_retracted: true,
        })
        .await?
        .ok_or_else(|| ApiError::not_found("memory_item", memory_id.to_string()))?;
    for linked_id in [
        item.publication_source_id.clone(),
        item.supersedes_id.clone(),
    ]
    .into_iter()
    .flatten()
    {
        let linked = state
            .db
            .get_memory_item_scoped(MemoryGetQuery {
                id: linked_id.clone(),
                identity_id: Some(query.identity_id.clone()),
                grants: vec![grant.clone()],
                include_retracted: true,
            })
            .await?;
        if linked.is_none() {
            if item.publication_source_id.as_deref() == Some(linked_id.as_str()) {
                item.publication_source_id = None;
            }
            if item.supersedes_id.as_deref() == Some(linked_id.as_str()) {
                item.supersedes_id = None;
            }
        }
    }
    let mut lifecycle = state
        .db
        .list_memory_lifecycle_assertions(&memory_id.to_string())
        .await?
        .into_iter()
        .take(MAX_PROVENANCE_ASSERTIONS)
        .collect::<Vec<_>>();
    for assertion in &mut lifecycle {
        let Some(related_id) = assertion.related_memory_id.clone() else {
            continue;
        };
        let related = state
            .db
            .get_memory_item_scoped(MemoryGetQuery {
                id: related_id,
                identity_id: Some(query.identity_id.clone()),
                grants: vec![grant.clone()],
                include_retracted: true,
            })
            .await?;
        if related.is_none() {
            assertion.related_memory_id = None;
        }
    }
    Ok(Json(provenance_response(item, lifecycle)?))
}

/// Inspect an immutable context manifest and its source decisions. This is
/// intentionally bounded and never returns source fragments or evidence.
pub async fn get_context_manifest(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(manifest_id): Path<String>,
    Query(query): Query<ContextManifestQuery>,
) -> ApiResult<Json<ContextManifestResponse>> {
    let manifest_id = parse_uuid(&manifest_id, "manifest_id")?;
    let identity_id = parse_uuid(&query.identity_id, "identity_id")?;
    let context_scope_id = parse_uuid(&query.context_scope_id, "context_scope_id")?;
    require_owned_identity(&state, &user, &query.identity_id).await?;
    let context_scope =
        AgentContextScopeRepo::get_context_scope(&*state.db, &context_scope_id.to_string())
            .await?
            .ok_or_else(|| ApiError::not_found("context_scope", context_scope_id.to_string()))?;
    if context_scope.identity_id != identity_id.to_string() {
        return Err(ApiError::not_found(
            "context_manifest",
            manifest_id.to_string(),
        ));
    }
    // Re-check the owning account/project/Agent Chat/task scope, rather than
    // trusting that a context-scope row was created by an earlier caller.
    authorized_scope_grant(
        &state,
        &user,
        &context_scope.scope_type,
        &context_scope.scope_id,
    )
    .await?;

    let service = ContextManifestService::new(Arc::clone(&state.db));
    let manifest = service
        .get_authorized(manifest_id, identity_id, context_scope_id)
        .await?
        .ok_or_else(|| ApiError::not_found("context_manifest", manifest_id.to_string()))?;
    let sources = service
        .sources(manifest_id)
        .await?
        .into_iter()
        .take(MAX_MANIFEST_SOURCES)
        .map(context_source_response)
        .collect();
    Ok(Json(context_manifest_response(manifest, sources)))
}

/// List recent manifests for an owned identity. With no context filter, only
/// manifests whose current canonical scope is still authorized are returned;
/// this keeps the inspector useful after membership and binding changes without
/// exposing stale scope metadata.
pub async fn list_context_manifests(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(identity_id): Path<String>,
    Query(query): Query<ContextManifestListQuery>,
) -> ApiResult<Json<ContextManifestListResponse>> {
    let identity = parse_uuid(&identity_id, "identity_id")?;
    require_owned_identity(&state, &user, &identity_id).await?;
    let requested_scope = query
        .context_scope_id
        .as_deref()
        .map(|value| parse_uuid(value, "context_scope_id"))
        .transpose()?;
    if let Some(scope_id) = requested_scope {
        let scope = AgentContextScopeRepo::get_context_scope(&*state.db, &scope_id.to_string())
            .await?
            .ok_or_else(|| ApiError::not_found("context_scope", scope_id.to_string()))?;
        if scope.identity_id != identity_id {
            return Err(ApiError::not_found("context_scope", scope_id.to_string()));
        }
        authorized_scope_grant(&state, &user, &scope.scope_type, &scope.scope_id).await?;
    }
    let service = ContextManifestService::new(Arc::clone(&state.db));
    let manifests = service
        .list_authorized(
            identity,
            requested_scope,
            query.limit.unwrap_or(20).clamp(1, 50),
        )
        .await?;
    let allowed_scope_ids = if requested_scope.is_none() {
        let mut allowed = HashSet::new();
        for scope in AgentContextScopeRepo::list_context_scopes(&*state.db, &identity_id).await? {
            if authorized_scope_grant(&state, &user, &scope.scope_type, &scope.scope_id)
                .await
                .is_ok()
            {
                allowed.insert(scope.id);
            }
        }
        Some(allowed)
    } else {
        None
    };
    let mut items = Vec::with_capacity(manifests.len());
    for manifest in manifests {
        if allowed_scope_ids
            .as_ref()
            .is_some_and(|allowed| !allowed.contains(&manifest.context_scope_id))
        {
            continue;
        }
        let sources = service
            .sources(parse_uuid(&manifest.id, "manifest_id")?)
            .await?
            .into_iter()
            .take(MAX_MANIFEST_SOURCES)
            .map(context_source_response)
            .collect();
        items.push(context_manifest_response(manifest, sources));
    }
    Ok(Json(ContextManifestListResponse {
        items,
        has_more: false,
    }))
}

async fn require_owned_identity(
    state: &AppState,
    user: &AuthenticatedUser,
    identity_id: &str,
) -> ApiResult<db::Agent> {
    let identity = AgentRepo::get_by_id(&*state.db, identity_id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent_identity", identity_id.to_owned()))?;
    if identity.owner_id.as_deref() != Some(user.user_id.as_str()) {
        return Err(ApiError::not_found(
            "agent_identity",
            identity_id.to_owned(),
        ));
    }
    Ok(identity)
}

async fn authorized_scope_grant(
    state: &AppState,
    user: &AuthenticatedUser,
    scope_type: &str,
    scope_id: &str,
) -> ApiResult<MemoryScopeGrant> {
    let scope_type = scope_type.trim().to_ascii_lowercase();
    if scope_id.trim().is_empty() {
        return Err(ApiError::bad_request("scope_id must not be empty"));
    }
    let visibility = match scope_type.as_str() {
        "account" => {
            if scope_id != user.user_id {
                return Err(ApiError::not_found("account", scope_id.to_owned()));
            }
            vec!["account".to_owned()]
        }
        "project" => {
            require_project_visible(state, scope_id, user).await?;
            vec!["project".to_owned()]
        }
        "agent_chat" => {
            let chat = state
                .agent_chat_service
                .get_authorized_chat(&user.user_id, scope_id)
                .await?;
            if !matches!(chat.kind.as_str(), "account_main" | "project") {
                return Err(ApiError::bad_request("Agent Chat kind is not admitted"));
            }
            vec!["chat".to_owned()]
        }
        "task" => {
            let task = TaskRepo::get_by_id(&*state.db, scope_id, false)
                .await?
                .ok_or_else(|| ApiError::not_found("task", scope_id.to_owned()))?;
            require_project_visible(state, &task.project_id, user).await?;
            vec!["project".to_owned()]
        }
        _ => {
            return Err(ApiError::bad_request(
                "scope_type must be account, project, agent_chat, or task",
            ));
        }
    };
    Ok(MemoryScopeGrant {
        scope_type,
        scope_id: scope_id.to_owned(),
        visibility,
        identity_id: None,
    })
}

fn validate_target_visibility(scope_type: &str, visibility: &str) -> ApiResult<()> {
    let allowed = match scope_type {
        "account" => ["account"].as_slice(),
        "project" | "task" => ["project"].as_slice(),
        "agent_chat" => ["chat"].as_slice(),
        _ => return Err(ApiError::bad_request("invalid target scope type")),
    };
    if allowed.contains(&visibility) {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "target_visibility is not valid for the target scope",
        ))
    }
}

fn validate_target_linkage(request: &MemoryPublicationRequest) -> ApiResult<()> {
    if request.target_scope_type == "account"
        && (request.target_project_id.is_some()
            || request.target_task_id.is_some()
            || request.target_chat_id.is_some())
    {
        return Err(ApiError::bad_request(
            "account target scopes cannot carry project, task, or Agent Chat linkage",
        ));
    }
    if request.target_scope_type == "project"
        && request.target_project_id.as_deref() != Some(request.target_scope_id.as_str())
    {
        return Err(ApiError::bad_request(
            "target_project_id must match a project target scope",
        ));
    }
    if request.target_scope_type == "project"
        && (request.target_task_id.is_some() || request.target_chat_id.is_some())
    {
        return Err(ApiError::bad_request(
            "project target scopes cannot carry task or Agent Chat linkage",
        ));
    }
    if request.target_scope_type == "agent_chat"
        && request.target_chat_id.as_deref() != Some(request.target_scope_id.as_str())
    {
        return Err(ApiError::bad_request(
            "target_chat_id must match an Agent Chat target scope",
        ));
    }
    if request.target_scope_type == "agent_chat" && request.target_task_id.is_some() {
        return Err(ApiError::bad_request(
            "Agent Chat target scopes cannot carry task linkage",
        ));
    }
    if request.target_scope_type == "task"
        && request.target_task_id.as_deref() != Some(request.target_scope_id.as_str())
    {
        return Err(ApiError::bad_request(
            "target_task_id must match a task target scope",
        ));
    }
    if request.target_scope_type == "task" && request.target_chat_id.is_some() {
        return Err(ApiError::bad_request(
            "task target scopes cannot carry Agent Chat linkage",
        ));
    }
    Ok(())
}

async fn require_project_visible(
    state: &AppState,
    project_id: &str,
    user: &AuthenticatedUser,
) -> ApiResult<()> {
    let project = ProjectRepo::get_by_id(&*state.db, project_id)
        .await?
        .ok_or_else(|| ApiError::not_found("project", project_id.to_owned()))?;
    if project.owner_id.is_none() {
        return Ok(());
    }
    if ProjectMemberRepo::get_member(&*state.db, project_id, &user.user_id)
        .await?
        .is_none()
    {
        return Err(ApiError::not_found("project", project_id.to_owned()));
    }
    Ok(())
}

fn provenance_response(
    item: MemoryItem,
    lifecycle: Vec<MemoryLifecycleAssertion>,
) -> ApiResult<MemoryProvenanceResponse> {
    if item.sensitivity == "secret" {
        return Err(ApiError::not_found("memory_item", item.id));
    }
    let source_ref = serde_json::from_str::<serde_json::Value>(&item.metadata_json)
        .ok()
        .and_then(|value| {
            value
                .get("source_ref")
                .and_then(serde_json::Value::as_str)
                .map(safe_metadata_value)
        });
    Ok(MemoryProvenanceResponse {
        id: item.id,
        scope_type: safe_metadata_value(&item.scope_type),
        scope_id: safe_metadata_value(&item.scope_id),
        visibility: safe_metadata_value(&item.visibility),
        owner_identity_id: item
            .owner_identity_id
            .map(|value| safe_metadata_value(&value)),
        authority: safe_metadata_value(&item.authority),
        sensitivity: safe_metadata_value(&item.sensitivity),
        retention_priority: item.retention_priority,
        source_type: safe_metadata_value(&item.source_type),
        source_ref,
        source_event_id: item
            .source_event_id
            .map(|value| safe_metadata_value(&value)),
        source_scope_type: item
            .source_scope_type
            .map(|value| safe_metadata_value(&value)),
        source_scope_id: item
            .source_scope_id
            .map(|value| safe_metadata_value(&value)),
        source_revision: item
            .source_revision
            .map(|value| safe_metadata_value(&value)),
        source_chat_sequence: None,
        publication_source_id: item
            .publication_source_id
            .map(|value| safe_metadata_value(&value)),
        supersedes_id: item.supersedes_id.map(|value| safe_metadata_value(&value)),
        valid_from: item.valid_from.map(|value| safe_metadata_value(&value)),
        valid_until: item.valid_until.map(|value| safe_metadata_value(&value)),
        created_by_type: item
            .created_by_type
            .map(|value| safe_metadata_value(&value)),
        created_by_id: item.created_by_id.map(|value| safe_metadata_value(&value)),
        created_at: item.created_at,
        lifecycle: lifecycle
            .into_iter()
            .take(MAX_PROVENANCE_ASSERTIONS)
            .map(lifecycle_response)
            .collect(),
    })
}

fn lifecycle_response(assertion: MemoryLifecycleAssertion) -> MemoryLifecycleResponse {
    MemoryLifecycleResponse {
        id: assertion.id,
        memory_item_id: assertion.memory_item_id,
        assertion_type: safe_metadata_value(&assertion.assertion_type),
        related_memory_id: assertion
            .related_memory_id
            .map(|value| safe_metadata_value(&value)),
        reason: assertion.reason.map(|reason| safe_metadata_value(&reason)),
        evidence_present: !assertion.evidence_json.trim().is_empty(),
        asserted_by_type: safe_metadata_value(&assertion.asserted_by_type),
        asserted_by_id: assertion
            .asserted_by_id
            .map(|value| safe_metadata_value(&value)),
        source_event_id: assertion
            .source_event_id
            .map(|value| safe_metadata_value(&value)),
        created_at: assertion.created_at,
    }
}

fn context_source_response(source: db::ContextManifestSource) -> ContextManifestSourceResponse {
    ContextManifestSourceResponse {
        ordinal: source.ordinal,
        source_id: safe_metadata_value(&source.source_id),
        source_type: safe_metadata_value(&source.source_type),
        source_revision: safe_metadata_value(&source.source_revision),
        selection_reason: safe_metadata_value(&source.selection_reason),
        disposition: safe_metadata_value(&source.disposition),
        retention_priority: source.retention_priority,
        fragment_fingerprint: safe_metadata_value(&source.fragment_fingerprint),
    }
}

fn context_manifest_response(
    manifest: db::ContextManifest,
    sources: Vec<ContextManifestSourceResponse>,
) -> ContextManifestResponse {
    ContextManifestResponse {
        id: manifest.id,
        identity_id: manifest.identity_id,
        agent_session_id: manifest.agent_session_id,
        context_scope_id: manifest.context_scope_id,
        scope_type: safe_metadata_value(&manifest.scope_type),
        scope_id: safe_metadata_value(&manifest.scope_id),
        policy_revision: safe_metadata_value(&manifest.policy_revision),
        domain_revision: safe_metadata_value(&manifest.domain_revision),
        lcm_binding_revision: manifest
            .lcm_binding_revision
            .map(|value| safe_metadata_value(&value)),
        runtime_manifest_id: manifest
            .runtime_manifest_id
            .map(|value| safe_metadata_value(&value)),
        runtime_manifest_fingerprint: manifest
            .runtime_manifest_fingerprint
            .map(|value| safe_metadata_value(&value)),
        combined_fingerprint: manifest.combined_fingerprint,
        request_fingerprint: safe_metadata_value(&manifest.request_fingerprint),
        created_at: manifest.created_at,
        sources,
    }
}

fn parse_uuid(value: &str, field: &'static str) -> ApiResult<Uuid> {
    Uuid::parse_str(value)
        .map_err(|error| ApiError::bad_request(format!("invalid {field} UUID: {error}")))
}

fn safe_metadata_value(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    let protected = lower.contains("authorization: bearer")
        || lower.contains("api_key")
        || lower.contains("sk-")
        || lower.contains("private key")
        || lower.contains("-----begin");
    if protected {
        return "[protected metadata redacted]".to_owned();
    }
    const MAX_BYTES: usize = 256;
    if value.len() <= MAX_BYTES {
        return value.to_owned();
    }
    let mut end = MAX_BYTES - 3;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &value[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn publication(target_scope_type: &str, target_scope_id: &str) -> MemoryPublicationRequest {
        MemoryPublicationRequest {
            source_scope_type: "account".to_owned(),
            source_scope_id: "source".to_owned(),
            target_scope_type: target_scope_type.to_owned(),
            target_scope_id: target_scope_id.to_owned(),
            target_project_id: None,
            target_task_id: None,
            target_chat_id: None,
            target_visibility: "project".to_owned(),
            target_authority: "observation".to_owned(),
            actor_identity_id: "identity".to_owned(),
            reason: "explicit publication".to_owned(),
            evidence_json: "{}".to_owned(),
        }
    }

    #[test]
    fn publication_linkage_rejects_scope_confusion() {
        let request = publication("project", "project-1");
        assert!(validate_target_linkage(&request).is_err());
    }

    #[test]
    fn publication_linkage_requires_canonical_target_ids() {
        let mut request = publication("project", "project-1");
        request.target_project_id = Some("project-1".to_owned());
        assert!(validate_target_linkage(&request).is_ok());
        assert!(validate_target_visibility("project", "participants").is_err());
        assert!(validate_target_visibility("project", "project").is_ok());
        assert!(validate_target_visibility("agent_chat", "participants").is_err());
        assert!(validate_target_visibility("agent_chat", "chat").is_ok());
        assert!(validate_target_visibility("room", "participants").is_err());
    }

    #[test]
    fn provenance_response_is_metadata_only_and_bounds_untrusted_fields() {
        let item = MemoryItem {
            row_id: 1,
            id: "memory".to_owned(),
            project_id: Some("project".to_owned()),
            task_id: None,
            execution_id: None,
            scope_type: "project".to_owned(),
            scope_id: "project".to_owned(),
            visibility: "project".to_owned(),
            owner_identity_id: None,
            authority: "observation".to_owned(),
            sensitivity: "internal".to_owned(),
            retention_priority: 10,
            provenance_json: "{}".to_owned(),
            publication_source_id: None,
            supersedes_id: None,
            valid_from: None,
            valid_until: None,
            source_event_id: None,
            source_scope_type: Some("project".to_owned()),
            source_scope_id: Some("project".to_owned()),
            source_revision: Some("1".to_owned()),
            source_type: "comment".to_owned(),
            kind: "comment".to_owned(),
            title: "secret title must not leave this function".to_owned(),
            summary: Some("secret body summary".to_owned()),
            body: "Authorization: Bearer sk-secret".to_owned(),
            metadata_json: serde_json::json!({
                "source_ref": "a source ref that is intentionally bounded"
            })
            .to_string(),
            confidence: None,
            quality_score: None,
            created_by_type: Some("agent".to_owned()),
            created_by_id: None,
            created_at: "now".to_owned(),
        };
        let response = provenance_response(
            item,
            vec![MemoryLifecycleAssertion {
                id: "assertion".to_owned(),
                memory_item_id: "memory".to_owned(),
                assertion_type: "evidence".to_owned(),
                related_memory_id: None,
                reason: Some("a".repeat(400)),
                evidence_json: r#"{"body":"do not expose"}"#.to_owned(),
                asserted_by_type: "agent".to_owned(),
                asserted_by_id: None,
                source_event_id: None,
                created_at: "now".to_owned(),
            }],
        )
        .expect("provenance response");
        let value = serde_json::to_value(response).expect("response serializes");
        assert!(value.get("body").is_none());
        assert!(value.get("title").is_none());
        assert!(value.get("evidence_json").is_none());
        assert_eq!(value["lifecycle"][0]["evidence_present"], true);
        assert!(value["lifecycle"][0]["reason"].as_str().unwrap().len() <= 256);
    }
}
