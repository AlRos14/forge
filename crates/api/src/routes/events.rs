use std::{convert::Infallible, sync::Arc};

use api_types::{PublicEventEnvelope, PublicEventType};
use axum::{
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
};
use db::{DomainEvent, DomainEventRepo};
use serde_json::{json, Map, Value};
use tokio_stream::{wrappers::BroadcastStream, StreamExt};

use crate::state::AppState;

pub async fn stream_events(
    State(state): State<AppState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let mut shutdown = state.shutdown_signal.subscribe();
    let shutdown_requested = async move {
        if *shutdown.borrow_and_update() {
            return;
        }

        while shutdown.changed().await.is_ok() {
            if *shutdown.borrow_and_update() {
                return;
            }
        }
    };

    let db = Arc::clone(&state.db);
    let stream = BroadcastStream::new(state.event_bus.subscribe()).then(move |result| {
        let db = Arc::clone(&db);
        async move {
            let public_event = match result {
                Ok(hint) if hint.event_type == "domain_event.committed" => {
                    match DomainEventRepo::get_event(&*db, &hint.entity_id).await {
                        Ok(Some(event)) => project_domain_event(&event),
                        _ => None,
                    }
                }
                Ok(hint) => project_runtime_hint(&hint),
                Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(skipped)) => {
                    Some(PublicEventEnvelope {
                        event_type: PublicEventType::EventsResyncRequired,
                        entity_id: "events.resync_required".to_owned(),
                        timestamp: events::event_timestamp(),
                        event_id: None,
                        sequence: None,
                        entity_type: None,
                        scope_type: None,
                        scope_id: None,
                        payload: Some(json!({
                            "reason": "subscriber_lagged",
                            "skipped": skipped,
                        })),
                    })
                }
            };
            public_event.map(to_sse_event).map(Ok)
        }
    });
    let stream = stream.filter_map(|event| event);

    Sse::new(futures_util::StreamExt::take_until(
        stream,
        shutdown_requested,
    ))
    .keep_alive(KeepAlive::default())
}

fn to_sse_event(event: PublicEventEnvelope) -> Event {
    let event_id = event
        .event_id
        .as_deref()
        .unwrap_or(event.entity_id.as_str())
        .to_owned();
    Event::default()
        .event(event.event_type.as_str())
        .id(event_id)
        .data(serde_json::to_string(&event).expect("public SSE envelope serializes"))
}

fn project_domain_event(event: &DomainEvent) -> Option<PublicEventEnvelope> {
    let source: Value = serde_json::from_str(&event.payload_json).ok()?;
    let event_type = public_domain_event_type(&event.event_type)?;
    let payload = match event.event_type.as_str() {
        "task.lifecycle_changed" => project_fields(
            &source,
            &[
                "task_id",
                "from_state",
                "to_state",
                "from_version",
                "to_version",
                "cause_kind",
                "cause_ref",
                "gate_evaluation_id",
                "reason_kind",
                "reason_ref",
            ],
        ),
        "gate.created" => {
            project_fields(&source, &["gate_id", "gate_kind", "scope_kind", "scope_id"])
        }
        "gate.policy_revised" => project_fields(&source, &["gate_id", "revision", "policy_digest"]),
        "gate.evaluated" => project_fields(
            &source,
            &[
                "evaluation_id",
                "gate_id",
                "task_id",
                "policy_revision",
                "outcome",
                "input_digest",
            ],
        ),
        "execution.started"
        | "execution.completed"
        | "execution.failed"
        | "execution.cancelled"
        | "execution.stalled"
        | "validation_run.started"
        | "validation_run.completed"
        | "evidence.created"
        | "artifact.created"
        | "message.created"
        | "handoff.created"
        | "handoff.status_changed"
        | "proposal.created"
        | "proposal.withdrawn"
        | "decision.recorded" => json!({}),
        // In particular, legacy Review-row events are not current Review
        // authority and never enter the public stream.
        _ => return None,
    };

    Some(PublicEventEnvelope {
        event_type,
        entity_id: event.entity_id.clone(),
        timestamp: event.created_at.clone(),
        event_id: Some(event.id.clone()),
        sequence: Some(u64::try_from(event.sequence).ok()?),
        entity_type: Some(event.entity_type.clone()),
        scope_type: Some(event.scope_type.clone()),
        scope_id: Some(event.scope_id.clone()),
        payload: Some(payload),
    })
}

fn public_domain_event_type(event_type: &str) -> Option<PublicEventType> {
    Some(match event_type {
        "task.lifecycle_changed" => PublicEventType::TaskLifecycleChanged,
        "gate.created" => PublicEventType::GateCreated,
        "gate.policy_revised" => PublicEventType::GatePolicyRevised,
        "gate.evaluated" => PublicEventType::GateEvaluated,
        "execution.started" => PublicEventType::ExecutionStarted,
        "execution.completed" => PublicEventType::ExecutionCompleted,
        "execution.failed" => PublicEventType::ExecutionFailed,
        "execution.cancelled" => PublicEventType::ExecutionCancelled,
        "execution.stalled" => PublicEventType::ExecutionStalled,
        "validation_run.started" => PublicEventType::ValidationRunStarted,
        "validation_run.completed" => PublicEventType::ValidationRunCompleted,
        "evidence.created" => PublicEventType::EvidenceCreated,
        "artifact.created" => PublicEventType::ArtifactCreated,
        "message.created" => PublicEventType::MessageCreated,
        "handoff.created" => PublicEventType::HandoffCreated,
        "handoff.status_changed" => PublicEventType::HandoffStatusChanged,
        "proposal.created" => PublicEventType::ProposalCreated,
        "proposal.withdrawn" => PublicEventType::ProposalWithdrawn,
        "decision.recorded" => PublicEventType::DecisionRecorded,
        _ => return None,
    })
}

fn project_fields(source: &Value, fields: &[&str]) -> Value {
    let mut projected = Map::new();
    for field in fields {
        if let Some(value) = source.get(*field) {
            projected.insert((*field).to_owned(), value.clone());
        }
    }
    Value::Object(projected)
}

fn project_runtime_hint(event: &events::ForgeEvent) -> Option<PublicEventEnvelope> {
    // These in-process events have no durable DomainEvent receipt. Keep their
    // public payload to identifiers and status fields; never serialize the
    // internal EventContext wholesale.
    let (event_type, payload) = match (event.event_type.as_str(), &event.context) {
        ("project.created", events::EventContext::ProjectCreated { .. }) => {
            (PublicEventType::ProjectCreated, json!({}))
        }
        ("project.updated", events::EventContext::ProjectUpdated {}) => {
            (PublicEventType::ProjectUpdated, json!({}))
        }
        ("project.deleted", events::EventContext::ProjectDeleted {}) => {
            (PublicEventType::ProjectDeleted, json!({}))
        }
        ("project.paused", events::EventContext::ProjectPaused { .. }) => {
            (PublicEventType::ProjectPaused, json!({}))
        }
        ("project.resumed", events::EventContext::ProjectResumed {}) => {
            (PublicEventType::ProjectResumed, json!({}))
        }
        (
            "project_hook.run_changed",
            events::EventContext::ProjectHookRunChanged {
                project_id,
                run_id,
                status,
                ..
            },
        ) => (
            PublicEventType::ProjectHookRunChanged,
            json!({"project_id": project_id, "run_id": run_id, "status": status}),
        ),
        (
            "notification.created",
            events::EventContext::NotificationCreated {
                notification_id,
                project_id,
                task_id,
                ..
            },
        ) => (
            PublicEventType::NotificationCreated,
            json!({"notification_id": notification_id, "project_id": project_id, "task_id": task_id}),
        ),
        (
            "operations.status_changed",
            events::EventContext::OperationsStatusChanged { trigger },
        ) => (
            PublicEventType::OperationsStatusChanged,
            json!({"trigger": trigger}),
        ),
        ("events.resync_required", _) => (
            PublicEventType::EventsResyncRequired,
            json!({"reason": "internal_resync_signal"}),
        ),
        _ => return None,
    };

    Some(PublicEventEnvelope {
        event_type,
        entity_id: event.entity_id.clone(),
        timestamp: event.timestamp.clone(),
        event_id: None,
        sequence: None,
        entity_type: None,
        scope_type: None,
        scope_id: None,
        payload: Some(payload),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domain_event(event_type: &str, payload: &str) -> DomainEvent {
        DomainEvent {
            sequence: 12,
            id: "event-12".into(),
            event_type: event_type.into(),
            entity_type: "task".into(),
            entity_id: "task-1".into(),
            actor_type: "human".into(),
            actor_id: Some("human-1".into()),
            scope_type: "task".into(),
            scope_id: "task-1".into(),
            correlation_id: "corr-1".into(),
            causation_id: None,
            causation_depth: 1,
            dedupe_key: None,
            payload_json: payload.into(),
            created_at: "2026-10-10T12:00:00Z".into(),
        }
    }

    #[test]
    fn lifecycle_event_is_exact_and_excludes_unapproved_payload_fields() {
        let event = domain_event(
            "task.lifecycle_changed",
            r#"{"task_id":"task-1","from_state":"ready","to_state":"active","from_version":2,"to_version":3,"cause_kind":"actor","prompt":"private"}"#,
        );
        let projected = project_domain_event(&event).expect("target lifecycle event");
        assert_eq!(projected.event_type, PublicEventType::TaskLifecycleChanged);
        assert_eq!(projected.payload.as_ref().unwrap()["to_state"], "active");
        assert!(projected.payload.as_ref().unwrap().get("prompt").is_none());
    }

    #[test]
    fn legacy_review_and_agent_chat_events_are_not_public() {
        assert!(project_domain_event(&domain_event("review.status_changed", "{}")).is_none());
        assert!(project_domain_event(&domain_event("agent_chat.message_created", "{}")).is_none());
    }

    #[test]
    fn runtime_task_status_and_chat_hints_are_not_public() {
        let task_status = events::ForgeEvent {
            event_type: "task.status_changed".to_owned(),
            entity_id: "task-1".to_owned(),
            timestamp: "2026-10-10T12:00:00Z".to_owned(),
            context: events::EventContext::TaskStatusChanged {
                project_id: "project-1".to_owned(),
                old_status: "todo".to_owned(),
                new_status: "in_progress".to_owned(),
            },
        };
        assert!(project_runtime_hint(&task_status).is_none());

        let chat = events::ForgeEvent {
            event_type: "agent_chat.turn_progress".to_owned(),
            entity_id: "chat-1".to_owned(),
            timestamp: "2026-10-10T12:00:00Z".to_owned(),
            context: events::EventContext::AgentChatTurnProgress {
                chat_id: "chat-1".to_owned(),
                turn_job_id: "turn-1".to_owned(),
                delta: "private model output".to_owned(),
            },
        };
        assert!(project_runtime_hint(&chat).is_none());
    }

    #[test]
    fn project_hook_hint_contains_only_its_public_identifiers_and_status() {
        let event = events::ForgeEvent {
            event_type: "project_hook.run_changed".to_owned(),
            entity_id: "run-1".to_owned(),
            timestamp: "2026-10-10T12:00:00Z".to_owned(),
            context: events::EventContext::ProjectHookRunChanged {
                project_id: "project-1".to_owned(),
                run_id: "run-1".to_owned(),
                rule_id: "rule-1".to_owned(),
                trigger_type: "task_created".to_owned(),
                dedupe_key: "private-dedupe-key".to_owned(),
                status: "succeeded".to_owned(),
                source_task_id: Some("task-1".to_owned()),
                automation_task_id: Some("automation-task".to_owned()),
                execution_id: Some("execution-1".to_owned()),
                agent_id: Some("agent-1".to_owned()),
                reason: Some("private diagnostic".to_owned()),
            },
        };
        let projected = project_runtime_hint(&event).expect("target Project Hook event");
        assert_eq!(projected.event_type, PublicEventType::ProjectHookRunChanged);
        let payload = projected.payload.expect("public payload");
        assert_eq!(
            payload,
            json!({"project_id":"project-1","run_id":"run-1","status":"succeeded"})
        );
    }
}
