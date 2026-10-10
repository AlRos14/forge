use std::str::FromStr;

use crate::{
    errors::{ApiError, ApiResult},
    routes::auth::AuthenticatedUser,
    state::AppState,
};
use api_types::{
    ActionExecutionResponse, AgentActionResponse, AnswerQuestionRequest, ApproveActionRequest,
    AskQuestionRequest, CommitmentEvidenceResponse, CommitmentResponse, CompleteCommitmentRequest,
    CoordinationListQuery, CreateCommitmentRequest, ExecuteActionRequest,
    ExecuteOrchestrationActionRequest, ExecuteTaskProposalRequest, InboxItemResponse,
    ProposeActionRequest, QuestionResponse, TaskProposalExecutionResponse, TaskProposalRequest,
    TransferCommitmentRequest, UpdateCommitmentRequest, UpdateInboxItemRequest,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use db::{
    Agent, AgentAction, AgentActionListQuery, AgentActionRepo, AgentActionStatus, AgentCommitment,
    AgentCommitmentEvidence, AgentCommitmentListQuery, AgentCommitmentRepo, AgentCommitmentStatus,
    AgentInboxItem, AgentInboxListQuery, AgentInboxRepo, AgentInboxStatus, AgentQuestion,
    AgentQuestionListQuery, AgentQuestionStatus, AgentRepo, TaskRepo,
};
use serde_json::Value;

pub async fn list_commitments(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(identity_id): Path<String>,
    Query(query): Query<CoordinationListQuery>,
) -> ApiResult<Json<Vec<CommitmentResponse>>> {
    require_owned_identity(&state, &identity_id, &user.user_id).await?;
    if let (Some(scope_type), Some(scope_id)) =
        (query.scope_type.as_deref(), query.scope_id.as_deref())
    {
        authorize_scope_member(&state, scope_type, scope_id, &user.user_id).await?;
    }
    let commitments = state
        .db
        .list_commitments(AgentCommitmentListQuery {
            owner_identity_id: Some(identity_id),
            scope_type: query.scope_type.clone(),
            scope_id: query.scope_id.clone(),
            status: parse_commitment_status(query.status.as_deref())?,
            limit: bounded_limit(query.limit),
        })
        .await?;
    let mut visible = Vec::with_capacity(commitments.len());
    for commitment in commitments {
        if authorize_commitment_read(&state, &commitment, &user.user_id)
            .await
            .is_ok()
        {
            visible.push(commitment_response(commitment));
        }
    }
    Ok(Json(visible))
}

pub async fn create_commitment(
    _state: State<AppState>,
    _user: AuthenticatedUser,
    _identity_id: Path<String>,
    _request: Json<CreateCommitmentRequest>,
) -> ApiResult<(StatusCode, Json<CommitmentResponse>)> {
    Err(ApiError::gone_with_code(
        "operation_retired",
        "legacy Agent commitments were retired in Plan PR11",
    ))
}

pub async fn get_commitment(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<Json<CommitmentResponse>> {
    let commitment = state
        .db
        .get_commitment(&id)
        .await?
        .ok_or_else(|| ApiError::not_found("commitment", id.clone()))?;
    authorize_commitment_read(&state, &commitment, &user.user_id).await?;
    Ok(Json(commitment_response(commitment)))
}

pub async fn update_commitment(
    _state: State<AppState>,
    _user: AuthenticatedUser,
    _id: Path<String>,
    _request: Json<UpdateCommitmentRequest>,
) -> ApiResult<Json<CommitmentResponse>> {
    Err(ApiError::gone_with_code(
        "operation_retired",
        "legacy Agent commitments were retired in Plan PR11",
    ))
}

pub async fn complete_commitment(
    _state: State<AppState>,
    _user: AuthenticatedUser,
    _id: Path<String>,
    _request: Json<CompleteCommitmentRequest>,
) -> ApiResult<Json<CommitmentResponse>> {
    Err(ApiError::gone_with_code(
        "operation_retired",
        "legacy Agent commitments were retired in Plan PR11",
    ))
}

pub async fn transfer_commitment(
    _state: State<AppState>,
    _user: AuthenticatedUser,
    _id: Path<String>,
    _request: Json<TransferCommitmentRequest>,
) -> ApiResult<Json<CommitmentResponse>> {
    Err(ApiError::gone_with_code(
        "operation_retired",
        "legacy Agent commitments were retired in Plan PR11",
    ))
}

pub async fn cancel_commitment(
    _state: State<AppState>,
    _user: AuthenticatedUser,
    _id: Path<String>,
    _request: Json<UpdateCommitmentRequest>,
) -> ApiResult<Json<CommitmentResponse>> {
    Err(ApiError::gone_with_code(
        "operation_retired",
        "legacy Agent commitments were retired in Plan PR11",
    ))
}

pub async fn list_commitment_evidence(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<CommitmentEvidenceResponse>>> {
    let commitment = state
        .db
        .get_commitment(&id)
        .await?
        .ok_or_else(|| ApiError::not_found("commitment", id.clone()))?;
    authorize_commitment_read(&state, &commitment, &user.user_id).await?;
    Ok(Json(
        state
            .db
            .list_commitment_evidence(&id)
            .await?
            .into_iter()
            .map(evidence_response)
            .collect(),
    ))
}

pub async fn list_inbox(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(identity_id): Path<String>,
    Query(query): Query<CoordinationListQuery>,
) -> ApiResult<Json<Vec<InboxItemResponse>>> {
    require_owned_identity(&state, &identity_id, &user.user_id).await?;
    if let (Some(scope_type), Some(scope_id)) =
        (query.scope_type.as_deref(), query.scope_id.as_deref())
    {
        authorize_scope_member(&state, scope_type, scope_id, &user.user_id).await?;
    }
    let items = state
        .db
        .list_inbox_items(AgentInboxListQuery {
            recipient_identity_id: identity_id,
            status: parse_inbox_status(query.status.as_deref())?,
            scope_type: query.scope_type.clone(),
            scope_id: query.scope_id.clone(),
            limit: bounded_limit(query.limit),
        })
        .await?;
    let mut visible = Vec::with_capacity(items.len());
    for item in items {
        if authorize_scope_member(&state, &item.scope_type, &item.scope_id, &user.user_id)
            .await
            .is_ok()
        {
            visible.push(inbox_response(item));
        }
    }
    Ok(Json(visible))
}

pub async fn get_inbox_item(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<Json<InboxItemResponse>> {
    let item = state
        .db
        .get_inbox_item(&id)
        .await?
        .ok_or_else(|| ApiError::not_found("inbox_item", id.clone()))?;
    require_owned_identity(&state, &item.recipient_identity_id, &user.user_id).await?;
    authorize_scope_member(&state, &item.scope_type, &item.scope_id, &user.user_id).await?;
    Ok(Json(inbox_response(item)))
}

pub async fn update_inbox_item(
    _state: State<AppState>,
    _user: AuthenticatedUser,
    _id: Path<String>,
    _request: Json<UpdateInboxItemRequest>,
) -> ApiResult<Json<InboxItemResponse>> {
    Err(ApiError::gone_with_code(
        "operation_retired",
        "legacy Agent inbox operations were retired in Plan PR11",
    ))
}

pub async fn list_questions(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(identity_id): Path<String>,
    Query(query): Query<CoordinationListQuery>,
) -> ApiResult<Json<Vec<QuestionResponse>>> {
    require_owned_identity(&state, &identity_id, &user.user_id).await?;
    if let (Some(scope_type), Some(scope_id)) =
        (query.scope_type.as_deref(), query.scope_id.as_deref())
    {
        authorize_scope_member(&state, scope_type, scope_id, &user.user_id).await?;
    }
    let questions = state
        .db
        .list_questions(AgentQuestionListQuery {
            recipient_identity_id: identity_id,
            status: parse_question_status(query.status.as_deref())?,
            scope_type: query.scope_type.clone(),
            scope_id: query.scope_id.clone(),
            limit: bounded_limit(query.limit),
        })
        .await?;
    let mut visible = Vec::with_capacity(questions.len());
    for question in questions {
        if authorize_question_read(&state, &question, &user.user_id)
            .await
            .is_ok()
        {
            visible.push(question_response(question));
        }
    }
    Ok(Json(visible))
}

pub async fn ask_question(
    _state: State<AppState>,
    _user: AuthenticatedUser,
    _identity_id: Path<String>,
    _request: Json<AskQuestionRequest>,
) -> ApiResult<(StatusCode, Json<QuestionResponse>)> {
    Err(ApiError::gone_with_code(
        "operation_retired",
        "legacy Agent questions were retired in Plan PR11",
    ))
}

pub async fn get_question(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<Json<QuestionResponse>> {
    let question = state
        .db
        .get_question(&id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent_question", id.clone()))?;
    authorize_question_read(&state, &question, &user.user_id).await?;
    Ok(Json(question_response(question)))
}

pub async fn answer_question(
    _state: State<AppState>,
    _user: AuthenticatedUser,
    _id: Path<String>,
    _request: Json<AnswerQuestionRequest>,
) -> ApiResult<Json<QuestionResponse>> {
    Err(ApiError::gone_with_code(
        "operation_retired",
        "legacy Agent questions were retired in Plan PR11",
    ))
}

pub async fn list_actions(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(identity_id): Path<String>,
    Query(query): Query<CoordinationListQuery>,
) -> ApiResult<Json<Vec<AgentActionResponse>>> {
    require_owned_identity(&state, &identity_id, &user.user_id).await?;
    if let (Some(scope_type), Some(scope_id)) =
        (query.scope_type.as_deref(), query.scope_id.as_deref())
    {
        authorize_scope_member(&state, scope_type, scope_id, &user.user_id).await?;
    }
    let actions = state
        .db
        .list_actions(AgentActionListQuery {
            actor_identity_id: Some(identity_id),
            scope_type: query.scope_type.clone(),
            scope_id: query.scope_id.clone(),
            status: parse_action_status(query.status.as_deref())?,
            limit: bounded_limit(query.limit),
        })
        .await?;
    let mut visible = Vec::with_capacity(actions.len());
    for action in actions {
        if authorize_action_read(&state, &action, &user.user_id)
            .await
            .is_ok()
        {
            visible.push(action_response(action));
        }
    }
    Ok(Json(visible))
}

pub async fn propose_action(
    _state: State<AppState>,
    _user: AuthenticatedUser,
    _identity_id: Path<String>,
    _request: Json<ProposeActionRequest>,
) -> ApiResult<(StatusCode, Json<AgentActionResponse>)> {
    Err(ApiError::gone_with_code(
        "operation_retired",
        "legacy Agent actions were retired in Plan PR11",
    ))
}

pub async fn propose_task(
    _state: State<AppState>,
    _user: AuthenticatedUser,
    _identity_id: Path<String>,
    _request: Json<TaskProposalRequest>,
) -> ApiResult<(StatusCode, Json<AgentActionResponse>)> {
    Err(ApiError::gone_with_code(
        "operation_retired",
        "legacy Agent task proposals were retired in Plan PR11; use Task coordination",
    ))
}

pub async fn get_action(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<Json<AgentActionResponse>> {
    let action = state
        .db
        .get_action(&id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent_action", id.clone()))?;
    authorize_action_read(&state, &action, &user.user_id).await?;
    Ok(Json(action_response(action)))
}

pub async fn approve_action(
    _state: State<AppState>,
    _user: AuthenticatedUser,
    _id: Path<String>,
    _request: Json<ApproveActionRequest>,
) -> ApiResult<Json<AgentActionResponse>> {
    Err(ApiError::gone_with_code(
        "operation_retired",
        "legacy Agent action approvals were retired in Plan PR11",
    ))
}

pub async fn execute_action(
    _state: State<AppState>,
    _user: AuthenticatedUser,
    _id: Path<String>,
    _request: Json<ExecuteActionRequest>,
) -> ApiResult<Json<ActionExecutionResponse>> {
    Err(ApiError::gone_with_code(
        "operation_retired",
        "legacy Agent action execution was retired in Plan PR11",
    ))
}

/// Execute a Main Agent Charter/Project proposal through its typed domain
/// materializer. The generic `/execute` endpoint intentionally refuses these
/// operations so a caller cannot manufacture a successful result envelope.
pub async fn execute_orchestration_action(
    _state: State<AppState>,
    _user: AuthenticatedUser,
    _id: Path<String>,
    _request: Json<ExecuteOrchestrationActionRequest>,
) -> ApiResult<Json<ActionExecutionResponse>> {
    Err(ApiError::gone_with_code(
        "operation_retired",
        "Main/Project Agent orchestration actions were retired in Plan PR11",
    ))
}

pub async fn execute_task_proposal(
    _state: State<AppState>,
    _user: AuthenticatedUser,
    _id: Path<String>,
    _request: Json<ExecuteTaskProposalRequest>,
) -> ApiResult<Json<TaskProposalExecutionResponse>> {
    Err(ApiError::gone_with_code(
        "operation_retired",
        "legacy Agent task proposals were retired in Plan PR11; use Task coordination",
    ))
}

async fn require_owned_identity(
    state: &AppState,
    identity_id: &str,
    user_id: &str,
) -> ApiResult<Agent> {
    AgentRepo::get_by_id(&*state.db, identity_id)
        .await?
        .filter(|agent| agent.owner_id.as_deref() == Some(user_id))
        .ok_or_else(|| ApiError::not_found("agent", identity_id.to_owned()))
}

async fn authorize_commitment_read(
    state: &AppState,
    commitment: &AgentCommitment,
    user_id: &str,
) -> ApiResult<()> {
    let owns_identity = AgentRepo::get_by_id(&*state.db, &commitment.owner_identity_id)
        .await?
        .is_some_and(|agent| agent.owner_id.as_deref() == Some(user_id));
    if owns_identity && commitment.scope_type == "account" && commitment.scope_id == user_id {
        return Ok(());
    }
    authorize_scope_member(state, &commitment.scope_type, &commitment.scope_id, user_id).await
}

async fn authorize_question_read(
    state: &AppState,
    question: &AgentQuestion,
    user_id: &str,
) -> ApiResult<()> {
    let owns_identity = AgentRepo::get_by_id(&*state.db, &question.recipient_identity_id)
        .await?
        .is_some_and(|agent| agent.owner_id.as_deref() == Some(user_id));
    if owns_identity && question.scope_type == "account" && question.scope_id == user_id {
        return Ok(());
    }
    authorize_scope_member(state, &question.scope_type, &question.scope_id, user_id).await
}

async fn authorize_action_read(
    state: &AppState,
    action: &AgentAction,
    user_id: &str,
) -> ApiResult<()> {
    let owns_identity = AgentRepo::get_by_id(&*state.db, &action.actor_identity_id)
        .await?
        .is_some_and(|agent| agent.owner_id.as_deref() == Some(user_id));
    if owns_identity && action.scope_type == "account" && action.scope_id == user_id {
        return Ok(());
    }
    authorize_scope_member(state, &action.scope_type, &action.scope_id, user_id).await
}

async fn authorize_scope_member(
    state: &AppState,
    scope_type: &str,
    scope_id: &str,
    user_id: &str,
) -> ApiResult<()> {
    match scope_type {
        "account" if scope_id == user_id => Ok(()),
        "project" => {
            crate::routes::project_agents::require_project_member(state, scope_id, user_id)
                .await
                .map(|_| ())
        }
        "agent_chat" => state
            .agent_chat_history
            .get_authorized_chat(user_id, scope_id)
            .await
            .map(|_| ())
            .map_err(Into::into),
        "task" => {
            let task = TaskRepo::get_by_id(&*state.db, scope_id, false)
                .await?
                .ok_or_else(|| ApiError::not_found("task", scope_id.to_owned()))?;
            crate::routes::project_agents::require_project_member(state, &task.project_id, user_id)
                .await
                .map(|_| ())
        }
        "agent" => require_owned_identity(state, scope_id, user_id)
            .await
            .map(|_| ()),
        _ => Err(ApiError::not_found("scope", scope_id.to_owned())),
    }
}

fn parse_commitment_status(value: Option<&str>) -> ApiResult<Option<AgentCommitmentStatus>> {
    value
        .map(|value| {
            AgentCommitmentStatus::from_str(value)
                .map_err(|_| ApiError::bad_request("invalid commitment status"))
        })
        .transpose()
}

fn parse_inbox_status(value: Option<&str>) -> ApiResult<Option<AgentInboxStatus>> {
    value
        .map(|value| {
            AgentInboxStatus::from_str(value)
                .map_err(|_| ApiError::bad_request("invalid inbox status"))
        })
        .transpose()
}

fn parse_question_status(value: Option<&str>) -> ApiResult<Option<AgentQuestionStatus>> {
    value
        .map(|value| {
            AgentQuestionStatus::from_str(value)
                .map_err(|_| ApiError::bad_request("invalid question status"))
        })
        .transpose()
}

fn parse_action_status(value: Option<&str>) -> ApiResult<Option<AgentActionStatus>> {
    value
        .map(|value| {
            AgentActionStatus::from_str(value)
                .map_err(|_| ApiError::bad_request("invalid action status"))
        })
        .transpose()
}

fn bounded_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(50).clamp(1, 100)
}

fn commitment_response(value: AgentCommitment) -> CommitmentResponse {
    CommitmentResponse {
        id: value.id,
        owner_identity_id: value.owner_identity_id,
        scope_type: value.scope_type,
        scope_id: value.scope_id,
        title: value.title,
        description: value.description,
        status: value.status.to_string(),
        due_at: value.due_at,
        correlation_id: value.correlation_id,
        originating_action_id: value.originating_action_id,
        originating_task_id: value.originating_task_id,
        evidence_required: value.evidence_required,
        cancellation_reason: value.cancellation_reason,
        blocked_reason: value.blocked_reason,
        completed_at: value.completed_at,
        cancelled_at: value.cancelled_at,
        version: value.version,
        created_at: value.created_at,
        updated_at: value.updated_at,
    }
}

fn evidence_response(value: AgentCommitmentEvidence) -> CommitmentEvidenceResponse {
    CommitmentEvidenceResponse {
        id: value.id,
        commitment_id: value.commitment_id,
        evidence_type: value.evidence_type,
        evidence_id: value.evidence_id,
        scope_type: value.scope_type,
        scope_id: value.scope_id,
        description: value.description,
        metadata: parse_json(&value.metadata_json),
        authorized_by_type: value.authorized_by_type,
        authorized_by_id: value.authorized_by_id,
        dedupe_key: value.dedupe_key,
        created_at: value.created_at,
    }
}

fn inbox_response(value: AgentInboxItem) -> InboxItemResponse {
    InboxItemResponse {
        id: value.id,
        recipient_identity_id: value.recipient_identity_id,
        scope_type: value.scope_type,
        scope_id: value.scope_id,
        kind: value.kind.to_string(),
        status: value.status.to_string(),
        title: value.title,
        body: value.body,
        payload: parse_json(&value.payload_json),
        source_type: value.source_type,
        source_id: value.source_id,
        correlation_id: value.correlation_id,
        causation_id: value.causation_id,
        dedupe_key: value.dedupe_key,
        read_at: value.read_at,
        acknowledged_at: value.acknowledged_at,
        version: value.version,
        created_at: value.created_at,
        updated_at: value.updated_at,
    }
}

fn question_response(value: AgentQuestion) -> QuestionResponse {
    QuestionResponse {
        id: value.id,
        recipient_identity_id: value.recipient_identity_id,
        scope_type: value.scope_type,
        scope_id: value.scope_id,
        status: value.status.to_string(),
        question: value.question,
        context: parse_json(&value.context_json),
        answer: value.answer,
        asked_by_type: value.asked_by_type,
        asked_by_id: value.asked_by_id,
        answered_by_type: value.answered_by_type,
        answered_by_id: value.answered_by_id,
        inbox_item_id: value.inbox_item_id,
        due_at: value.due_at,
        correlation_id: value.correlation_id,
        version: value.version,
        answered_at: value.answered_at,
        created_at: value.created_at,
        updated_at: value.updated_at,
    }
}

fn action_response(value: AgentAction) -> AgentActionResponse {
    let materialized = action_materialized(
        &value.operation,
        &value.status,
        value.target_type.as_deref(),
        value.target_id.as_deref(),
        value.outcome_json.as_deref(),
    );
    AgentActionResponse {
        id: value.id,
        actor_identity_id: value.actor_identity_id,
        scope_type: value.scope_type,
        scope_id: value.scope_id,
        operation: value.operation,
        payload_hash: value.payload_hash,
        dedupe_key: value.dedupe_key,
        correlation_id: value.correlation_id,
        causation_id: value.causation_id,
        causation_depth: value.causation_depth,
        requested_permission: value.requested_permission,
        policy_result: value.policy_result.to_string(),
        policy_reason: value.policy_reason,
        status: value.status.to_string(),
        target_type: value.target_type,
        target_id: value.target_id,
        outcome: value.outcome_json.as_deref().map(parse_json),
        materialized,
        version: value.version,
        created_at: value.created_at,
        updated_at: value.updated_at,
    }
}

/// Materialization is a derived public projection, not a second authority
/// flag. It becomes true only after a typed executor has transitioned the
/// action to `executed`, retained its server-derived target, and persisted the
/// typed outcome that proves which domain operation completed.
fn action_materialized(
    operation: &str,
    status: &AgentActionStatus,
    target_type: Option<&str>,
    target_id: Option<&str>,
    outcome_json: Option<&str>,
) -> bool {
    if *status != AgentActionStatus::Executed
        || target_type.is_none_or(str::is_empty)
        || target_id.is_none_or(str::is_empty)
    {
        return false;
    }
    let Some(outcome) = outcome_json
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
        .and_then(|value| value.as_object().cloned())
    else {
        return false;
    };

    if operation == "task.propose" {
        return outcome
            .get("task_id")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty());
    }
    outcome
        .get("operation")
        .and_then(Value::as_str)
        .is_some_and(|value| value == operation)
}

fn parse_json(value: &str) -> Value {
    serde_json::from_str(value).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use api_types::PROJECT_DOCUMENT_OPERATION;

    #[test]
    fn action_materialized_is_false_until_typed_outcome_is_persisted() {
        assert!(!action_materialized(
            "task.propose",
            &AgentActionStatus::Proposed,
            Some("project"),
            Some("project-1"),
            None,
        ));
        assert!(!action_materialized(
            "task.propose",
            &AgentActionStatus::Executed,
            Some("project"),
            Some("project-1"),
            None,
        ));
        assert!(!action_materialized(
            "task.propose",
            &AgentActionStatus::Executed,
            None,
            Some("project-1"),
            Some(r#"{"task_id":"task-1"}"#),
        ));
        assert!(!action_materialized(
            "task.propose",
            &AgentActionStatus::Executed,
            Some("project"),
            Some("project-1"),
            Some(r#"{"task_id":""}"#),
        ));
        assert!(action_materialized(
            "task.propose",
            &AgentActionStatus::Executed,
            Some("project"),
            Some("project-1"),
            Some(r#"{"task_id":"task-1"}"#),
        ));
        assert!(action_materialized(
            PROJECT_DOCUMENT_OPERATION,
            &AgentActionStatus::Executed,
            Some("project"),
            Some("project-1"),
            Some(r#"{"operation":"project.document","domain_committed":true}"#),
        ));
        assert!(!action_materialized(
            PROJECT_DOCUMENT_OPERATION,
            &AgentActionStatus::Executed,
            Some("project"),
            Some("project-1"),
            Some(r#"{"operation":"project.decision","domain_committed":true}"#),
        ));
        assert!(!action_materialized(
            "ordinary.action",
            &AgentActionStatus::Executed,
            Some("project"),
            Some("project-1"),
            Some(r#"{"task_id":"task-1"}"#),
        ));
    }
}
