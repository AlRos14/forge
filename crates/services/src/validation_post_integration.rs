//! Post-integration check trigger (plan coordinator-verification-and-pr-loop,
//! node C3). After a WorkUnit integration succeeds, every
//! `trigger = after_integration` catalog pin of the Task's current coordination
//! envelope is enqueued against the exact integration Workspace, before any
//! merge-readiness evaluation can consult it. Enqueue is idempotent per exact
//! subject: a new integration commit yields a new request and the previous run
//! stays as history.

use db::{
    EnvelopeCheckTrigger, SqliteDb, TaskCoordinationMode, TaskCoordinationRepo, TaskRepo,
    ValidationCheckRequestTrigger, ValidationCheckSubjectKind,
};
use events::EventBus;

use crate::{
    task_coordination::CoordinationEnvelope,
    validation_check_runner::{enqueue_check_request, EnqueueCheckRequest},
    Result,
};

/// Enqueue the Task's `after_integration` checks. Returns how many requests
/// exist for the current subject (new or replayed). A Task without current
/// coordination authority or pins is a no-op. Per-pin failures are logged and
/// skipped so one stale pin cannot block the others.
pub(crate) async fn enqueue_after_integration(
    db: &SqliteDb,
    event_bus: &EventBus,
    task_id: &str,
) -> Result<usize> {
    let Some(coordination) = TaskCoordinationRepo::get_task_coordination(db, task_id)
        .await?
        .filter(|coordination| {
            coordination.revoked_at.is_none()
                && coordination.mode != TaskCoordinationMode::Manual
                && !coordination.team_stale
        })
    else {
        return Ok(0);
    };
    let Ok(envelope) = serde_json::from_str::<CoordinationEnvelope>(&coordination.envelope_json)
    else {
        return Ok(0);
    };
    let pins: Vec<_> = envelope
        .validation_checks
        .iter()
        .filter(|pin| pin.trigger == EnvelopeCheckTrigger::AfterIntegration)
        .collect();
    if pins.is_empty() {
        return Ok(0);
    }
    let Some(task) = TaskRepo::get_by_id(db, task_id, false).await? else {
        return Ok(0);
    };
    let mut enqueued = 0;
    for pin in pins {
        let outcome = enqueue_check_request(
            db,
            event_bus,
            EnqueueCheckRequest {
                task_id: task.id.clone(),
                check_key: pin.check_key.clone(),
                trigger: ValidationCheckRequestTrigger::AfterIntegration,
                subject_kind: ValidationCheckSubjectKind::TaskIntegration,
                subject_id: task.id.clone(),
                subject_version: task.version,
                baseline: false,
                requested_by_request_id: None,
                requested_by_execution_id: None,
                reason: None,
            },
        )
        .await;
        match outcome {
            Ok(_) => enqueued += 1,
            Err(error) => tracing::warn!(
                task_id = %task.id,
                check_key = %pin.check_key,
                %error,
                "after-integration validation check was not enqueued"
            ),
        }
    }
    Ok(enqueued)
}
