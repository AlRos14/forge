use api_types::{
    ArtifactKind as ApiArtifactKind, ArtifactResponse,
    ArtifactStorageKind as ApiArtifactStorageKind, CollaborationListQuery,
    CollaborationTarget as ApiTarget, CreateArtifactRequest, CreateDecisionRequest,
    CreateHandoffRequest, CreateMessageRequest, CreateProposalRequest,
    DecisionOutcome as ApiDecisionOutcome, DecisionResponse, HandoffIntent as ApiHandoffIntent,
    HandoffResponse, HandoffStatus as ApiHandoffStatus, HandoffTransitionRequest, MessageResponse,
    PaginatedResponse, ProposalResponse, ProposalStatus as ApiProposalStatus,
    ProposalTarget as ApiProposalTarget, ProposalTargetKind as ApiProposalTargetKind,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use db::{
    ActorRef, Artifact, ArtifactKind, ArtifactStorageKind, CollaborationTarget,
    Decision as DbDecision, DecisionOutcome, Handoff, HandoffIntent, HandoffStatus,
    Message as DbMessage, PageRequest, Proposal, ProposalStatus, ProposalTarget,
    ProposalTargetKind, SortBy, SortOrder,
};
use serde_json::Value;
use services::{
    CollaborationActorSource, CreateArtifactInput, CreateDecisionInput, CreateHandoffInput,
    CreateMessageInput, CreateProposalInput,
};

use crate::{
    errors::{ApiError, ApiResult},
    routes::{auth::AuthenticatedUser, paginated},
    state::AppState,
};

fn page(params: CollaborationListQuery) -> PageRequest {
    PageRequest {
        cursor: params.cursor,
        limit: params.limit.unwrap_or(20).clamp(1, 100),
        include_total: params.include_total.unwrap_or(false),
        sort_by: SortBy::CreatedAt,
        sort_order: SortOrder::Desc,
    }
}

fn actor_to_db(actor: api_types::ActorRef) -> ActorRef {
    match actor {
        api_types::ActorRef::Human(id) => ActorRef::Human(id),
        api_types::ActorRef::Agent(id) => ActorRef::Agent(id),
    }
}

fn actor_to_api(actor: ActorRef) -> api_types::ActorRef {
    match actor {
        ActorRef::Human(id) => api_types::ActorRef::Human(id),
        ActorRef::Agent(id) => api_types::ActorRef::Agent(id),
    }
}

fn target_to_db(target: ApiTarget) -> CollaborationTarget {
    match target {
        ApiTarget::Actor { actor } => CollaborationTarget::Actor(actor_to_db(actor)),
        ApiTarget::Role { role_id } => CollaborationTarget::Role(role_id),
        ApiTarget::Task => CollaborationTarget::Task,
    }
}

fn target_to_api(target: CollaborationTarget) -> ApiTarget {
    match target {
        CollaborationTarget::Actor(actor) => ApiTarget::Actor {
            actor: actor_to_api(actor),
        },
        CollaborationTarget::Role(role_id) => ApiTarget::Role { role_id },
        CollaborationTarget::Task => ApiTarget::Task,
    }
}

fn artifact_kind_to_db(kind: ApiArtifactKind) -> ArtifactKind {
    match kind {
        ApiArtifactKind::Plan => ArtifactKind::Plan,
        ApiArtifactKind::ReviewReport => ArtifactKind::ReviewReport,
        ApiArtifactKind::ValidationReport => ArtifactKind::ValidationReport,
        ApiArtifactKind::Diff => ArtifactKind::Diff,
        ApiArtifactKind::Patch => ArtifactKind::Patch,
        ApiArtifactKind::Summary => ArtifactKind::Summary,
        ApiArtifactKind::DesignDocument => ArtifactKind::DesignDocument,
        ApiArtifactKind::Investigation => ArtifactKind::Investigation,
        ApiArtifactKind::ApiContract => ArtifactKind::ApiContract,
        ApiArtifactKind::TestReport => ArtifactKind::TestReport,
    }
}

fn artifact_kind_to_api(kind: ArtifactKind) -> ApiArtifactKind {
    match kind {
        ArtifactKind::Plan => ApiArtifactKind::Plan,
        ArtifactKind::ReviewReport => ApiArtifactKind::ReviewReport,
        ArtifactKind::ValidationReport => ApiArtifactKind::ValidationReport,
        ArtifactKind::Diff => ApiArtifactKind::Diff,
        ArtifactKind::Patch => ApiArtifactKind::Patch,
        ArtifactKind::Summary => ApiArtifactKind::Summary,
        ArtifactKind::DesignDocument => ApiArtifactKind::DesignDocument,
        ArtifactKind::Investigation => ApiArtifactKind::Investigation,
        ArtifactKind::ApiContract => ApiArtifactKind::ApiContract,
        ArtifactKind::TestReport => ApiArtifactKind::TestReport,
    }
}

fn artifact_storage_to_db(kind: ApiArtifactStorageKind) -> ArtifactStorageKind {
    match kind {
        ApiArtifactStorageKind::Inline => ArtifactStorageKind::Inline,
        ApiArtifactStorageKind::External => ArtifactStorageKind::External,
    }
}

fn artifact_storage_to_api(kind: ArtifactStorageKind) -> ApiArtifactStorageKind {
    match kind {
        ArtifactStorageKind::Inline => ApiArtifactStorageKind::Inline,
        ArtifactStorageKind::External => ApiArtifactStorageKind::External,
    }
}

fn artifact_response(record: Artifact, include_content: bool) -> ApiResult<ArtifactResponse> {
    let metadata: Value = serde_json::from_str(&record.metadata_json)
        .map_err(|_| ApiError::internal("Stored Artifact metadata is invalid"))?;
    if !metadata.is_object() {
        return Err(ApiError::internal(
            "Stored Artifact metadata is not an object",
        ));
    }
    Ok(ArtifactResponse {
        id: record.id,
        task_id: record.task_id,
        kind: artifact_kind_to_api(record.kind),
        storage_kind: artifact_storage_to_api(record.storage_kind),
        content: include_content.then_some(record.content).flatten(),
        metadata,
        digest: record.digest,
        producer_execution_id: record.producer_execution_id,
        producer: actor_to_api(record.producer),
        created_at: record.created_at,
    })
}

fn message_response(record: DbMessage) -> MessageResponse {
    MessageResponse {
        id: record.id,
        task_id: record.task_id,
        sender: actor_to_api(record.sender),
        target: target_to_api(record.target),
        body: record.body,
        artifact_ids: record.artifact_ids,
        created_at: record.created_at,
    }
}

fn handoff_status_to_db(value: ApiHandoffStatus) -> HandoffStatus {
    match value {
        ApiHandoffStatus::Pending => HandoffStatus::Pending,
        ApiHandoffStatus::Accepted => HandoffStatus::Accepted,
        ApiHandoffStatus::Completed => HandoffStatus::Completed,
        ApiHandoffStatus::Declined => HandoffStatus::Declined,
        ApiHandoffStatus::Cancelled => HandoffStatus::Cancelled,
    }
}

fn handoff_status_to_api(value: HandoffStatus) -> ApiHandoffStatus {
    match value {
        HandoffStatus::Pending => ApiHandoffStatus::Pending,
        HandoffStatus::Accepted => ApiHandoffStatus::Accepted,
        HandoffStatus::Completed => ApiHandoffStatus::Completed,
        HandoffStatus::Declined => ApiHandoffStatus::Declined,
        HandoffStatus::Cancelled => ApiHandoffStatus::Cancelled,
    }
}

fn handoff_intent_to_db(value: ApiHandoffIntent) -> HandoffIntent {
    match value {
        ApiHandoffIntent::Rework => HandoffIntent::Rework,
        ApiHandoffIntent::Delegation => HandoffIntent::Delegation,
        ApiHandoffIntent::Question => HandoffIntent::Question,
        ApiHandoffIntent::Answer => HandoffIntent::Answer,
        ApiHandoffIntent::Investigate => HandoffIntent::Investigate,
        ApiHandoffIntent::DecisionRequest => HandoffIntent::DecisionRequest,
    }
}

fn handoff_intent_to_api(value: HandoffIntent) -> ApiHandoffIntent {
    match value {
        HandoffIntent::Rework => ApiHandoffIntent::Rework,
        HandoffIntent::Delegation => ApiHandoffIntent::Delegation,
        HandoffIntent::Question => ApiHandoffIntent::Question,
        HandoffIntent::Answer => ApiHandoffIntent::Answer,
        HandoffIntent::Investigate => ApiHandoffIntent::Investigate,
        HandoffIntent::DecisionRequest => ApiHandoffIntent::DecisionRequest,
    }
}

fn handoff_response(record: Handoff) -> HandoffResponse {
    HandoffResponse {
        id: record.id,
        task_id: record.task_id,
        created_by: actor_to_api(record.created_by),
        source_role_id: record.source_role_id,
        target: target_to_api(record.target),
        intent: handoff_intent_to_api(record.intent),
        parent_execution_id: record.parent_execution_id,
        expected_policy_ref: record.expected_policy_ref,
        status: handoff_status_to_api(record.status),
        version: record.version,
        artifact_ids: record.artifact_ids,
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

fn proposal_target_to_db(value: ApiProposalTarget) -> ProposalTarget {
    ProposalTarget {
        kind: match value.kind {
            ApiProposalTargetKind::Task => ProposalTargetKind::Task,
            ApiProposalTargetKind::Execution => ProposalTargetKind::Execution,
            ApiProposalTargetKind::Workspace => ProposalTargetKind::Workspace,
        },
        id: value.id,
    }
}

fn proposal_target_to_api(value: ProposalTarget) -> ApiProposalTarget {
    ApiProposalTarget {
        kind: match value.kind {
            ProposalTargetKind::Task => ApiProposalTargetKind::Task,
            ProposalTargetKind::Execution => ApiProposalTargetKind::Execution,
            ProposalTargetKind::Workspace => ApiProposalTargetKind::Workspace,
        },
        id: value.id,
    }
}

fn proposal_status_to_api(value: ProposalStatus) -> ApiProposalStatus {
    match value {
        ProposalStatus::Open => ApiProposalStatus::Open,
        ProposalStatus::Resolved => ApiProposalStatus::Resolved,
        ProposalStatus::Withdrawn => ApiProposalStatus::Withdrawn,
        ProposalStatus::Superseded => ApiProposalStatus::Superseded,
    }
}

fn proposal_response(record: Proposal) -> ProposalResponse {
    ProposalResponse {
        id: record.id,
        task_id: record.task_id,
        proposer: actor_to_api(record.proposer),
        target: proposal_target_to_api(record.target),
        action: record.action,
        reason: record.reason,
        target_version: record.target_version,
        target_digest: record.target_digest,
        required_policy_ref: record.required_policy_ref,
        required_policy_version: record.required_policy_version,
        required_policy_digest: record.required_policy_digest,
        content_version: record.content_version,
        status: proposal_status_to_api(record.status),
        supersedes_proposal_id: record.supersedes_proposal_id,
        artifact_ids: record.artifact_ids,
        created_at: record.created_at,
    }
}

fn decision_outcome_to_db(value: ApiDecisionOutcome) -> DecisionOutcome {
    match value {
        ApiDecisionOutcome::Approve => DecisionOutcome::Approve,
        ApiDecisionOutcome::Reject => DecisionOutcome::Reject,
        ApiDecisionOutcome::Supersede => DecisionOutcome::Supersede,
    }
}

fn decision_outcome_to_api(value: DecisionOutcome) -> ApiDecisionOutcome {
    match value {
        DecisionOutcome::Approve => ApiDecisionOutcome::Approve,
        DecisionOutcome::Reject => ApiDecisionOutcome::Reject,
        DecisionOutcome::Supersede => ApiDecisionOutcome::Supersede,
    }
}

fn decision_response(record: DbDecision) -> DecisionResponse {
    DecisionResponse {
        id: record.id,
        task_id: record.task_id,
        proposal_id: record.proposal_id,
        proposal_version: record.proposal_version,
        outcome: decision_outcome_to_api(record.outcome),
        rationale: record.rationale,
        policy_ref: record.policy_ref,
        policy_version: record.policy_version,
        policy_digest: record.policy_digest,
        actors: record.actors.into_iter().map(actor_to_api).collect(),
        created_at: record.created_at,
    }
}

fn source(user: &AuthenticatedUser) -> CollaborationActorSource {
    CollaborationActorSource::Human(user.user_id.clone())
}

pub async fn create_artifact(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Json(body): Json<CreateArtifactRequest>,
) -> ApiResult<Json<ArtifactResponse>> {
    let record = state
        .collaboration_service
        .create_artifact(
            source(&user),
            CreateArtifactInput {
                task_id,
                producer_execution_id: body.producer_execution_id,
                kind: artifact_kind_to_db(body.kind),
                storage_kind: artifact_storage_to_db(body.storage_kind),
                content: body.content,
                content_ref: body.content_ref,
                metadata_json: body.metadata.to_string(),
                digest: body.digest,
            },
        )
        .await?;
    Ok(Json(artifact_response(record, true)?))
}

pub async fn list_artifacts(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Query(query): Query<CollaborationListQuery>,
) -> ApiResult<Json<PaginatedResponse<ArtifactResponse>>> {
    let page = state
        .collaboration_service
        .list_artifacts(&task_id, source(&user), page(query))
        .await?;
    let has_more = page.next_cursor.is_some();
    let mut items = Vec::with_capacity(page.items.len());
    for record in page.items {
        items.push(artifact_response(record, false)?);
    }
    Ok(Json(PaginatedResponse {
        items,
        next_cursor: page.next_cursor,
        has_more,
        total_count: page.total_count.and_then(|count| u64::try_from(count).ok()),
    }))
}

pub async fn get_artifact(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<ArtifactResponse>> {
    let record = state
        .collaboration_service
        .get_artifact(&id, source(&user))
        .await?;
    Ok(Json(artifact_response(record, true)?))
}

pub async fn create_message(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Json(body): Json<CreateMessageRequest>,
) -> ApiResult<Json<MessageResponse>> {
    let record = state
        .collaboration_service
        .create_message(
            source(&user),
            CreateMessageInput {
                task_id,
                target: target_to_db(body.target),
                body: body.body,
                artifact_ids: body.artifact_ids,
            },
        )
        .await?;
    Ok(Json(message_response(record)))
}

pub async fn list_messages(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Query(query): Query<CollaborationListQuery>,
) -> ApiResult<Json<PaginatedResponse<MessageResponse>>> {
    let page = state
        .collaboration_service
        .list_messages(&task_id, source(&user), page(query))
        .await?;
    Ok(Json(paginated(page, message_response)))
}

pub async fn get_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<MessageResponse>> {
    let record = state
        .collaboration_service
        .get_message(&id, source(&user))
        .await?;
    Ok(Json(message_response(record)))
}

pub async fn create_handoff(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Json(body): Json<CreateHandoffRequest>,
) -> ApiResult<Json<HandoffResponse>> {
    let record = state
        .collaboration_service
        .create_handoff(
            source(&user),
            CreateHandoffInput {
                task_id,
                source_role_id: body.source_role_id,
                target: target_to_db(body.target),
                intent: handoff_intent_to_db(body.intent),
                parent_execution_id: body.parent_execution_id,
                expected_policy_ref: body.expected_policy_ref,
                artifact_ids: body.artifact_ids,
            },
        )
        .await?;
    Ok(Json(handoff_response(record)))
}

pub async fn list_handoffs(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Query(query): Query<CollaborationListQuery>,
) -> ApiResult<Json<PaginatedResponse<HandoffResponse>>> {
    let page = state
        .collaboration_service
        .list_handoffs(&task_id, source(&user), page(query))
        .await?;
    Ok(Json(paginated(page, handoff_response)))
}

pub async fn get_handoff(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<HandoffResponse>> {
    let record = state
        .collaboration_service
        .get_handoff(&id, source(&user))
        .await?;
    Ok(Json(handoff_response(record)))
}

pub async fn transition_handoff(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
    Json(body): Json<HandoffTransitionRequest>,
) -> ApiResult<Json<HandoffResponse>> {
    let record = state
        .collaboration_service
        .transition_handoff(
            source(&user),
            &id,
            body.expected_version,
            handoff_status_to_db(body.status),
        )
        .await?;
    Ok(Json(handoff_response(record)))
}

pub async fn create_proposal(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Json(body): Json<CreateProposalRequest>,
) -> ApiResult<Json<ProposalResponse>> {
    let record = state
        .collaboration_service
        .create_proposal(
            source(&user),
            CreateProposalInput {
                task_id,
                target: proposal_target_to_db(body.target),
                action: body.action,
                reason: body.reason,
                target_version: body.target_version,
                target_digest: body.target_digest,
                required_policy_ref: body.required_policy_ref,
                required_policy_version: body.required_policy_version,
                required_policy_digest: body.required_policy_digest,
                supersedes_proposal_id: body.supersedes_proposal_id,
                artifact_ids: body.artifact_ids,
            },
        )
        .await?;
    Ok(Json(proposal_response(record)))
}

pub async fn list_proposals(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Query(query): Query<CollaborationListQuery>,
) -> ApiResult<Json<PaginatedResponse<ProposalResponse>>> {
    let page = state
        .collaboration_service
        .list_proposals(&task_id, source(&user), page(query))
        .await?;
    Ok(Json(paginated(page, proposal_response)))
}

pub async fn get_proposal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<ProposalResponse>> {
    let record = state
        .collaboration_service
        .get_proposal(&id, source(&user))
        .await?;
    Ok(Json(proposal_response(record)))
}

pub async fn withdraw_proposal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<ProposalResponse>> {
    let record = state
        .collaboration_service
        .withdraw_proposal(source(&user), &id)
        .await?;
    Ok(Json(proposal_response(record)))
}

pub async fn create_decision(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Json(body): Json<CreateDecisionRequest>,
) -> ApiResult<Json<DecisionResponse>> {
    let record = state
        .collaboration_service
        .record_decision(
            CreateDecisionInput {
                task_id,
                proposal_id: body.proposal_id,
                proposal_version: body.proposal_version,
                outcome: decision_outcome_to_db(body.outcome),
                rationale: body.rationale,
                policy_ref: body.policy_ref,
                policy_version: body.policy_version,
                policy_digest: body.policy_digest,
            },
            vec![source(&user)],
        )
        .await?;
    Ok(Json(decision_response(record)))
}

pub async fn list_decisions(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Query(query): Query<CollaborationListQuery>,
) -> ApiResult<Json<PaginatedResponse<DecisionResponse>>> {
    let page = state
        .collaboration_service
        .list_decisions(&task_id, source(&user), page(query))
        .await?;
    Ok(Json(paginated(page, decision_response)))
}

pub async fn get_decision(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<DecisionResponse>> {
    let record = state
        .collaboration_service
        .get_decision(&id, source(&user))
        .await?;
    Ok(Json(decision_response(record)))
}
