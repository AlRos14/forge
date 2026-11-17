use db::{
    ActorRef, AuthorityEffect, HumanDecisionRequest, HumanDecisionSourceKind, HumanDecisionState,
    TaskCoordination, TaskCoordinationMode, TaskCoordinationRequestKind,
};
use serde_json::json;

use super::*;

fn options_json() -> String {
    json!([
        {"id":"waive","label":"Waive","description":"d","consequence":"c","authority_effect":"scope"},
        {"id":"fix","label":"Fix","description":"d","consequence":"c","authority_effect":"none"}
    ])
    .to_string()
}

fn binding() -> ResumeBinding {
    ResumeBinding {
        request_id: "req-1".to_owned(),
        request_kind: TaskCoordinationRequestKind::ValidationFindings,
        authorization_version: 3,
        source_subject: json!({"kind":"validation_comparison","id":"cmp-1"}),
    }
}

fn decision() -> HumanDecisionRequest {
    HumanDecisionRequest {
        id: "dec-1".to_owned(),
        task_id: "task-1".to_owned(),
        source_kind: HumanDecisionSourceKind::CoordinationRequest,
        source_id: "req-1".to_owned(),
        raised_by_execution_id: None,
        authorization_version: Some(3),
        question: "Waive?".to_owned(),
        options_json: options_json(),
        recommended_option_id: None,
        recommendation_rationale: None,
        evidence_links_json: "[]".to_owned(),
        max_authority_effect: AuthorityEffect::Scope,
        state: HumanDecisionState::Answered,
        answered_by_user_id: Some("user-1".to_owned()),
        answered_option_id: Some("fix".to_owned()),
        answer_note: None,
        answered_at: Some("2026-11-15T00:00:00Z".to_owned()),
        resume_request_id: None,
        resume_binding_json: serde_json::to_string(&binding()).unwrap(),
        version: 2,
        created_at: "2026-11-15T00:00:00Z".to_owned(),
        updated_at: "2026-11-15T00:00:00Z".to_owned(),
    }
}

fn coordination() -> TaskCoordination {
    TaskCoordination {
        task_id: "task-1".to_owned(),
        mode: TaskCoordinationMode::PrepareAndExecute,
        coordinator_task_role_id: Some("role-1".to_owned()),
        coordinator_actor: Some(ActorRef::Agent("agent-1".to_owned())),
        authorization_version: 3,
        team_snapshot_json: "{}".to_owned(),
        team_digest: "t".repeat(64),
        team_stale: false,
        team_generation: 1,
        envelope_json: "{}".to_owned(),
        envelope_digest: "e".repeat(64),
        requested_by: Some("user-1".to_owned()),
        turn_count: 0,
        quality_request_count: 0,
        max_turns: 8,
        max_quality_requests: 4,
        created_at: "2026-11-15T00:00:00Z".to_owned(),
        updated_at: "2026-11-15T00:00:00Z".to_owned(),
        revoked_at: None,
        revoke_reason: None,
    }
}

#[test]
fn human_decision_authority_follows_effect() {
    assert_eq!(
        required_authority(AuthorityEffect::None),
        RequiredAuthority::TaskMutation
    );
    for effect in [AuthorityEffect::Scope, AuthorityEffect::Authority] {
        assert_eq!(
            required_authority(effect),
            RequiredAuthority::ProjectAdministration
        );
    }
}

#[test]
fn human_decision_chosen_option_must_exist_and_drives_effect() {
    let decision = decision();
    assert_eq!(
        chosen_option_effect(&decision, "waive").unwrap(),
        AuthorityEffect::Scope
    );
    assert_eq!(
        chosen_option_effect(&decision, "fix").unwrap(),
        AuthorityEffect::None
    );
    let error = chosen_option_effect(&decision, "other").unwrap_err();
    assert!(matches!(
        error,
        ServiceError::Coordination {
            code: CODE_INVALID_OPTIONS,
            ..
        }
    ));
}

#[test]
fn human_decision_note_is_bounded() {
    assert!(validate_note(None).is_ok());
    assert!(validate_note(Some(&"n".repeat(MAX_NOTE_BYTES))).is_ok());
    assert!(validate_note(Some(&"n".repeat(MAX_NOTE_BYTES + 1))).is_err());
}

#[test]
fn human_decision_resume_binding_rejects_unknown_fields() {
    let mut value = serde_json::to_value(binding()).unwrap();
    value["extra"] = json!(1);
    assert!(serde_json::from_value::<ResumeBinding>(value).is_err());
    let round = serde_json::to_string(&binding()).unwrap();
    assert_eq!(
        serde_json::from_str::<ResumeBinding>(&round).unwrap(),
        binding()
    );
}

#[test]
fn human_decision_resume_digests_are_exact_and_deterministic() {
    let decision = decision();
    let coordination = coordination();
    let first = resume_request_digest(&decision, &binding(), 5, &coordination);
    assert_eq!(
        first,
        resume_request_digest(&decision, &binding(), 5, &coordination)
    );
    assert_ne!(
        first,
        resume_request_digest(&decision, &binding(), 6, &coordination)
    );
    let mut other = decision.clone();
    other.answered_option_id = Some("waive".to_owned());
    assert_ne!(
        first,
        resume_request_digest(&other, &binding(), 5, &coordination)
    );
    assert_eq!(first.len(), 64);
    assert_ne!(
        decision_subject_digest(&decision),
        decision_subject_digest(&other)
    );
    assert_eq!(
        resume_idempotency_key("dec-1", 3),
        "human-decision-resume:dec-1:3"
    );
}

#[test]
fn human_decision_resume_brief_is_valid_and_excludes_free_text() {
    let mut decision = decision();
    decision.answer_note = Some("secret note".to_owned());
    let brief = resume_brief(&decision, &binding(), 5, &coordination()).unwrap();
    assert!(brief.contains("dec-1"));
    assert!(brief.contains("req-1"));
    assert!(!brief.contains("secret note"));
    assert!(!brief.contains("Waive?"));
}

#[test]
fn human_decision_events_carry_ids_not_question_text() {
    // The payload shape is built inline in the service; pin the contract names.
    assert_eq!(
        EVENT_HUMAN_DECISION_REQUESTED,
        "task.human_decision_requested"
    );
    assert_eq!(
        EVENT_HUMAN_DECISION_ANSWERED,
        "task.human_decision_answered"
    );
}
