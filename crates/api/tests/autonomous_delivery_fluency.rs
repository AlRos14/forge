mod common;

use api_types::{
    AgentResponse, DaemonFrame, DaemonProtocolCapabilities, ExecutionResponse, ExecutionStatus,
    ExecutionTerminalNotification, GateEvaluationResponse, GateResponse, MergeAfterGateResponse,
    ProjectPlanBatchResponse, ProjectPlanGraphResponse, ProjectTaskRoleDefaultsResponse,
    SubmitReviewReportResponse, TaskLifecycleState, TaskResponse, WorkUnitIntegrationResponse,
    WorkUnitResponse, DAEMON_PROTOCOL_FEATURE_EXECUTION_ROLE_V1,
    DAEMON_PROTOCOL_FEATURE_GENERIC_HARNESS_INVOCATION_V1,
    DAEMON_PROTOCOL_FEATURE_TASK_ROLE_CAPABILITIES_V1, METHOD_EXECUTION_START,
    METHOD_EXECUTION_TERMINAL, METHOD_PROTOCOL_CAPABILITIES,
};
use axum::http::{Method, StatusCode};
use db::{DomainEventRepo, TaskCoordinationRepo};
use futures_util::StreamExt;
use serde_json::{json, Value};
use sha2::Digest;
use std::{fmt::Debug, time::Duration};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use common::fake_daemon::{
    connect_daemon, register_daemon, send_daemon_notification, send_daemon_response,
    wait_until_connected, TestServer,
};
use common::{
    create_project_and_repo, create_shell_agents, empty_request, json_request, setup_git_repo,
    test_app, TestDir,
};

type CoordinatorWakeDiagnostic = (
    String,
    String,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
);
type StaleGateInputDiagnostic = (i64, String, String, i64, String, Option<String>);
type ReviewProducerDiagnostic = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

async fn configure_project_delivery_roles(
    app: &axum::Router,
    project_id: &str,
    coordinator_id: &str,
    implementer_id: &str,
) {
    let defaults: ProjectTaskRoleDefaultsResponse = empty_request(
        app,
        Method::GET,
        &format!("/api/v1/projects/{project_id}/task-role-defaults"),
        StatusCode::OK,
    )
    .await;

    let _: ProjectTaskRoleDefaultsResponse = json_request(
        app,
        Method::PUT,
        &format!("/api/v1/projects/{project_id}/task-role-defaults"),
        json!({
            "expected_version": defaults.version,
            "roles": [
                {
                    "role": "orchestrator",
                    "coordination_mode": "collaborative",
                    "configuration": {
                        "schema_version": 1,
                        "display_label": "Project coordinator",
                        "capabilities": ["orchestration"],
                        "rework": null,
                        "coordinator_actor": {"kind": "agent", "id": coordinator_id}
                    },
                    "members": [{"kind": "agent", "id": coordinator_id}]
                },
                {
                    "role": "quality_reviewer",
                    "coordination_mode": "independent",
                    "configuration": {
                        "schema_version": 1,
                        "display_label": "Quality reviewer",
                        "capabilities": ["formal_review"],
                        "rework": null,
                        "coordinator_actor": null
                    },
                    "members": [{"kind": "human", "id": "test-user-id"}]
                },
                {
                    "role": "implementer",
                    "coordination_mode": "independent",
                    "configuration": {
                        "schema_version": 1,
                        "display_label": "Implementer",
                        "capabilities": [],
                        "rework": null,
                        "coordinator_actor": null
                    },
                    "members": [{"kind": "agent", "id": implementer_id}]
                }
            ]
        }),
        StatusCode::OK,
    )
    .await;
}

async fn wait_for_task_lifecycle(
    harness: &common::Harness,
    task_id: &str,
    expected: TaskLifecycleState,
) -> TaskResponse {
    let mut last_state = None;
    for _ in 0..200 {
        let task: TaskResponse = empty_request(
            &harness.app,
            Method::GET,
            &format!("/api/v1/tasks/{task_id}"),
            StatusCode::OK,
        )
        .await;
        if task.lifecycle.state == expected {
            return task;
        }
        last_state = Some(format!("{:?}", task.lifecycle.state));
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let latest_gate = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT evaluation.id, evaluation.outcome, currentness.inputs_current
         FROM gate_evaluation evaluation
         JOIN gate_evaluation_currentness currentness
           ON currentness.evaluation_id = evaluation.id
         WHERE evaluation.task_id = ?
         ORDER BY evaluation.evaluated_at DESC, evaluation.id DESC
         LIMIT 1",
    )
    .bind(task_id)
    .fetch_optional(harness.state.db.pool())
    .await
    .expect("latest Gate currentness loads for lifecycle timeout diagnostics");
    let stale_inputs: Vec<StaleGateInputDiagnostic> =
        if let Some((evaluation_id, _, _)) = &latest_gate {
            sqlx::query_as(
                "SELECT input.ordinal, input.input_kind, input.input_id,
                    input.input_version, input.status, input.producer_ref
             FROM gate_evaluation_stale_input stale
             JOIN gate_evaluation_input input
               ON input.evaluation_id = stale.evaluation_id
              AND input.ordinal = stale.ordinal
             WHERE stale.evaluation_id = ?
             ORDER BY input.ordinal",
            )
            .bind(evaluation_id)
            .fetch_all(harness.state.db.pool())
            .await
            .expect("stale Gate inputs load for lifecycle timeout diagnostics")
        } else {
            Vec::new()
        };
    let review_producers: Vec<ReviewProducerDiagnostic> =
        if let Some((evaluation_id, _, _)) = &latest_gate {
            sqlx::query_as(
                "SELECT input.input_id, input.producer_ref, execution.role,
                    execution.status, authority.capabilities_json
             FROM gate_evaluation_input input
             LEFT JOIN execution
               ON execution.id = input.producer_ref AND execution.task_id = input.task_id
             LEFT JOIN execution_task_role_authority authority
               ON authority.execution_id = execution.id
             WHERE input.evaluation_id = ? AND input.input_kind = 'review_report'
             ORDER BY input.ordinal",
            )
            .bind(evaluation_id)
            .fetch_all(harness.state.db.pool())
            .await
            .expect("ReviewReport producer authority loads for lifecycle timeout diagnostics")
        } else {
            Vec::new()
        };
    panic!(
        "Task {task_id} did not reach lifecycle {expected:?}; last state={last_state:?}; latest Gate (id, outcome, inputs_current)={latest_gate:?}; stale inputs={stale_inputs:?}; ReviewReport producers (artifact_id, execution_id, role, status, capabilities_json)={review_producers:?}"
    );
}

async fn wait_for_execution_completed(app: &axum::Router, task_id: &str, execution_id: &str) {
    for _ in 0..200 {
        let execution: ExecutionResponse = empty_request(
            app,
            Method::GET,
            &format!("/api/v1/executions/{execution_id}"),
            StatusCode::OK,
        )
        .await;
        if execution.status == ExecutionStatus::Completed {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("Task {task_id} Execution {execution_id} did not complete");
}

async fn scheduler_request_for(
    harness: &common::Harness,
    batch_id: &str,
    task_id: &str,
) -> Option<db::TaskCoordinationRequest> {
    let key = format!("project-plan-batch:{batch_id}:turn:{task_id}");
    TaskCoordinationRepo::get_task_coordination_request_by_key(&*harness.state.db, task_id, &key)
        .await
        .expect("batch Task request lookup succeeds")
}

async fn move_orchestrator_cursor_to_head(harness: &common::Harness) {
    let head: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM domain_event")
        .fetch_one(harness.state.db.pool())
        .await
        .expect("fixture event head loads");
    let now = db::now_rfc3339();
    sqlx::query(
        "INSERT INTO event_consumer_cursor (consumer_name,last_sequence,version,updated_at)
         VALUES ('task-orchestrator-wakes',?,1,?)
         ON CONFLICT(consumer_name) DO UPDATE SET last_sequence=excluded.last_sequence,
           version=event_consumer_cursor.version+1,updated_at=excluded.updated_at",
    )
    .bind(head)
    .bind(now)
    .execute(harness.state.db.pool())
    .await
    .expect("fixture cursor excludes setup events");
}

async fn coordinator_start_diagnostics(
    harness: &common::Harness,
    request_id: &str,
    task_id: &str,
) -> String {
    let request =
        TaskCoordinationRepo::get_task_coordination_request(&*harness.state.db, request_id)
            .await
            .expect("batch request diagnostic loads");
    let wake_rows: Vec<CoordinatorWakeDiagnostic> = sqlx::query_as(
        "SELECT wake.id,wake.state,wake.last_error,wake.current_attempt,
                attempt.execution_id,attempt.state,attempt.last_error
         FROM orchestrator_wake wake
         JOIN domain_event event ON event.id=wake.event_id
         LEFT JOIN orchestrator_wake_execution attempt ON attempt.wake_id=wake.id
         WHERE event.event_type='task.coordination_requested' AND event.entity_id=?
         ORDER BY wake.created_at,wake.id,attempt.attempt_number",
    )
    .bind(request_id)
    .fetch_all(harness.state.db.pool())
    .await
    .expect("batch coordinator wake diagnostics load");
    let executions: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT id,status,error FROM execution
         WHERE task_id=? AND role='orchestrator' ORDER BY created_at,id",
    )
    .bind(task_id)
    .fetch_all(harness.state.db.pool())
    .await
    .expect("batch coordinator Execution diagnostics load");
    format!("request={request:?}; wakes_and_attempts={wake_rows:?}; coordinator_executions={executions:?}")
}

async fn next_coordinator_start_or_runtime_exit<T: Debug + Send + 'static>(
    socket: &mut common::fake_daemon::ClientSocket,
    coordinator_start: &mut tokio::task::JoinHandle<T>,
    harness: &common::Harness,
    request_id: &str,
    task_id: &str,
) -> (String, Value) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        tokio::select! {
            result = &mut *coordinator_start => {
                let diagnostics = coordinator_start_diagnostics(harness, request_id, task_id).await;
                panic!("OrchestratorRuntime finished before daemon execution.start arrived: result={result:?}; {diagnostics}");
            }
            incoming = tokio::time::timeout_at(deadline, socket.next()) => {
                let message = match incoming {
                    Ok(Some(Ok(message))) => message,
                    Ok(Some(Err(error))) => {
                        let diagnostics = coordinator_start_diagnostics(harness, request_id, task_id).await;
                        coordinator_start.abort();
                        panic!("fake Harness daemon socket failed before execution.start: {error}; {diagnostics}");
                    }
                    Ok(None) => {
                        let diagnostics = coordinator_start_diagnostics(harness, request_id, task_id).await;
                        coordinator_start.abort();
                        panic!("fake Harness daemon socket closed before execution.start; {diagnostics}");
                    }
                    Err(_) => {
                        let diagnostics = coordinator_start_diagnostics(harness, request_id, task_id).await;
                        coordinator_start.abort();
                        panic!("30-second deadline elapsed before daemon execution.start; {diagnostics}");
                    }
                };
                let WsMessage::Text(text) = message else {
                    continue;
                };
                let frame: DaemonFrame = serde_json::from_str(text.as_ref())
                    .expect("fake Harness daemon frame parses before coordinator start");
                match frame {
                    DaemonFrame::Request {
                        id,
                        method,
                        params: _,
                    }
                        if method == METHOD_PROTOCOL_CAPABILITIES =>
                    {
                        send_daemon_response(
                            socket,
                            id,
                            DaemonProtocolCapabilities {
                                schema_version: 1,
                                features: vec![
                                    DAEMON_PROTOCOL_FEATURE_GENERIC_HARNESS_INVOCATION_V1.to_owned(),
                                    DAEMON_PROTOCOL_FEATURE_EXECUTION_ROLE_V1.to_owned(),
                                    DAEMON_PROTOCOL_FEATURE_TASK_ROLE_CAPABILITIES_V1.to_owned(),
                                ],
                            },
                        )
                        .await;
                    }
                    DaemonFrame::Request { id, method, params } => {
                        assert_eq!(method, METHOD_EXECUTION_START);
                        return (id, params);
                    }
                    DaemonFrame::Heartbeat { .. } => {}
                    other => panic!("expected coordinator Harness request, got {other:?}"),
                }
            }
        }
    }
}

#[tokio::test]
async fn successor_starts_only_after_predecessor_task_merge_and_exact_handoff() {
    let repo_dir = TestDir::new("autonomous-delivery-repo");
    let repo_path = setup_git_repo(repo_dir.path());
    let workspaces = TestDir::new("autonomous-delivery-workspaces");
    let harness = test_app(workspaces.path(), "autonomous-delivery").await;
    let (project_id, _repo_id) =
        create_project_and_repo(&harness.app, "Autonomous delivery", &repo_path).await;
    let (implementer_id, _) =
        create_shell_agents(&harness.app, workspaces.path(), "autonomous-delivery").await;
    let registration = register_daemon(
        &harness.app,
        "autonomous-delivery-fake-harness",
        "autonomous-delivery",
    )
    .await;
    let fake_server = TestServer::start(harness.state.clone()).await;
    let mut fake_harness = connect_daemon(
        &fake_server,
        &registration.daemon_id,
        Some(&registration.registration_token),
    )
    .await
    .expect("fake Harness daemon connects");
    wait_until_connected(&harness.state, &registration.daemon_id).await;
    let _: Value = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/daemons/{}/report", registration.daemon_id),
        &registration.registration_token,
        json!({
            "detected_clis": [{"kind":"codex","availability":"authenticated"}],
            "runtimes": [{"kind":"local","workspace_root":workspaces.path().to_string_lossy(),"status":"ready"}],
            "labels": {"suite":"autonomous-delivery"}
        }),
        StatusCode::OK,
    )
    .await;
    let coordinator: AgentResponse = json_request(
        &harness.app,
        Method::POST,
        "/api/v1/agents",
        json!({
            "name":"Autonomous delivery fake Harness coordinator",
            "executor_type":"codex",
            "daemon_id":registration.daemon_id
        }),
        StatusCode::OK,
    )
    .await;
    configure_project_delivery_roles(&harness.app, &project_id, &coordinator.id, &implementer_id)
        .await;

    let predecessor: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({"title":"A: accepted implementation"}),
        StatusCode::OK,
    )
    .await;
    let successor: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({"title":"B: dependent follow-up"}),
        StatusCode::OK,
    )
    .await;

    let graph: ProjectPlanGraphResponse = empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{project_id}/plan"),
        StatusCode::OK,
    )
    .await;
    let edge = common::raw_json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/dependencies", successor.id),
        json!({
            "depends_on_id": predecessor.id,
            "expected_graph_version": graph.version,
            "idempotency_key": "autonomous-delivery-a-before-b"
        }),
    )
    .await;
    assert_eq!(edge.status(), StatusCode::CREATED);

    let graph: ProjectPlanGraphResponse = empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{project_id}/plan"),
        StatusCode::OK,
    )
    .await;
    let batch: ProjectPlanBatchResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/plan/batches"),
        json!({
            "expected_version": graph.version,
            "task_ids": [predecessor.id, successor.id],
            "max_parallel_tasks": 2,
            "idempotency_key": "autonomous-delivery-phase-once"
        }),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(batch.authorized_by, "test-user-id");

    move_orchestrator_cursor_to_head(&harness).await;
    let scheduler = services::project_plan::ProjectPlanService::new(
        harness.state.db.clone(),
        harness.state.event_bus.clone(),
    );
    assert_eq!(
        scheduler
            .check_once()
            .await
            .expect("initial batch scheduling tick"),
        1
    );
    assert!(scheduler_request_for(&harness, &batch.id, &predecessor.id)
        .await
        .is_some());
    assert!(scheduler_request_for(&harness, &batch.id, &successor.id)
        .await
        .is_none());

    let request = scheduler_request_for(&harness, &batch.id, &predecessor.id)
        .await
        .expect("A has the exact batch coordination request");
    let coordination =
        db::TaskCoordinationRepo::get_task_coordination(&*harness.state.db, &predecessor.id)
            .await
            .expect("A coordination authorization loads")
            .expect("A coordination authorization exists");
    let current_task = db::TaskRepo::get_by_id(&*harness.state.db, &predecessor.id, false)
        .await
        .expect("A Task loads")
        .expect("A Task exists");
    let coordinator_plan = json!({
        "schema_version": 1,
        "request_id": request.id,
        "task_id": predecessor.id,
        "task_version": current_task.version,
        "authorization_version": coordination.authorization_version,
        "team_digest": coordination.team_digest,
        "envelope_digest": coordination.envelope_digest,
        "objective_digest": request.objective_digest,
        "work_units": [{
            "key":"implementation",
            "title":"Implement A",
            "scope":"Create and commit accepted-a.txt for the exact authorized delivery.",
            "role":"implementer",
            "assigned_actor":{"kind":"agent","id":implementer_id},
            "parent_key":null,
            "depends_on":[],
            "requires_integration":true,
            "execution_instructions":"printf 'accepted A\\n' > accepted-a.txt && git add accepted-a.txt && git commit -m 'accept A'",
            "review_subject_key":null,
            "acceptance":["accepted-a.txt is committed to the Task branch."],
            "artifact_inputs":[]
        }],
        "delivery":null,
        "review":null,
        "limits":null
    })
    .to_string();
    let runtime = services::OrchestratorRuntime::new(
        harness.state.db.clone(),
        harness.state.event_bus.clone(),
        harness.state.task_service.clone(),
    );
    let runtime_for_start = runtime.clone();
    let mut coordinator_start = tokio::spawn(async move { runtime_for_start.run_once(100).await });
    let (start_id, start_params) = next_coordinator_start_or_runtime_exit(
        &mut fake_harness,
        &mut coordinator_start,
        &harness,
        &request.id,
        &predecessor.id,
    )
    .await;
    let coordinator_execution_id = start_params["execution_id"]
        .as_str()
        .expect("fake Harness start has an Execution id")
        .to_owned();
    send_daemon_response(
        &mut fake_harness,
        start_id,
        api_types::ExecutionStartResult {
            execution_id: coordinator_execution_id.clone(),
            accepted: true,
        },
    )
    .await;
    let run = coordinator_start
        .await
        .expect("coordinator runtime task joins")
        .expect("coordinator wake dispatches through fake Harness");
    assert_eq!(run.admitted_wakes, 1);
    assert_eq!(run.dispatched_wakes, 1);
    let fake_native_session = format!("fake-native-session-{coordinator_execution_id}");
    send_daemon_notification(
        &mut fake_harness,
        METHOD_EXECUTION_TERMINAL,
        ExecutionTerminalNotification {
            execution_id: coordinator_execution_id,
            exit_code: Some(0),
            signal: None,
            error: None,
            ts: db::now_rfc3339(),
            status: Some("completed".to_owned()),
            agent_session_id: Some(fake_native_session),
            summary: Some(coordinator_plan.clone()),
            assistant_output: Some(coordinator_plan),
            after_sha: None,
            usage: None,
            account_usage: None,
            failure_class: None,
            retry_at: None,
            resolved_candidate: None,
            route_attempts: None,
        },
    )
    .await;
    let mut materialized_request = request.clone();
    for _ in 0..200 {
        runtime
            .run_once(100)
            .await
            .expect("runtime reconciles fake Harness terminal result");
        materialized_request = db::TaskCoordinationRepo::get_task_coordination_request(
            &*harness.state.db,
            &request.id,
        )
        .await
        .expect("batch request reloads")
        .expect("batch request remains durable");
        if materialized_request.state == db::TaskCoordinationRequestState::Materialized {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        materialized_request.kind,
        db::TaskCoordinationRequestKind::WorkUnitPlan
    );
    assert_eq!(
        materialized_request.state,
        db::TaskCoordinationRequestState::Materialized,
        "the fake Harness output must be materialized through the exact batch plan"
    );
    let plan_artifact_id = materialized_request
        .plan_artifact_id
        .as_deref()
        .expect("materialized request owns its WorkUnitPlan Artifact");
    let materialized_work_unit =
        db::WorkUnitRepo::list_by_task(&*harness.state.db, &predecessor.id)
            .await
            .expect("materialized A WorkUnits load")
            .into_iter()
            .find(|unit| {
                unit.provenance
                    == Some(db::WorkUnitProvenance::Artifact(
                        plan_artifact_id.to_owned(),
                    ))
            })
            .expect("A implementer WorkUnit comes from the exact batch plan Artifact");
    let work_unit: WorkUnitResponse = empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/work-units/{}", materialized_work_unit.id),
        StatusCode::OK,
    )
    .await;
    let execution = common::launch_work_unit_execution(
        &harness.app,
        &predecessor.id,
        &work_unit,
        Some("printf 'accepted A\\n' > accepted-a.txt && git add accepted-a.txt && git commit -m 'accept A'"),
    )
    .await;
    wait_for_execution_completed(&harness.app, &predecessor.id, &execution.id).await;

    let current_work_unit: WorkUnitResponse = empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/work-units/{}", work_unit.id),
        StatusCode::OK,
    )
    .await;
    let completed_work_unit: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/work-units/{}/status", work_unit.id),
        json!({
            "expected_version": current_work_unit.version,
            "status": "completed"
        }),
        StatusCode::OK,
    )
    .await;
    let integration: WorkUnitIntegrationResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/work-units/{}/integrations", work_unit.id),
        json!({
            "execution_id": execution.id,
            "idempotency_key": "autonomous-delivery-a-integration"
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        integration.outcome,
        api_types::WorkUnitIntegrationOutcome::Success
    );

    let reviewer_role = predecessor
        .task_roles
        .iter()
        .find(|role| role.role == "quality_reviewer")
        .expect("Task has its configured formal-review TaskRole");
    assert!(reviewer_role
        .configuration
        .capabilities
        .contains(&api_types::TaskRoleCapability::FormalReview));
    let review_execution: api_types::ReviewExecutionResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/review-executions", predecessor.id),
        json!({
            "task_role_id": reviewer_role.id,
            "workspace_id": integration.target_workspace_id
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        review_execution.execution.workspace_id.as_deref(),
        Some(integration.target_workspace_id.as_str()),
        "Human Review Execution is scoped to the exact integration Workspace"
    );
    let review: SubmitReviewReportResponse = json_request(
        &harness.app,
        Method::POST,
        &format!(
            "/api/v1/review-executions/{}",
            review_execution.execution.id
        ),
        json!({
            "verdict": "pass",
            "summary": "The exact integrated result for A satisfies its acceptance criteria.",
            "criteria": ["The integrated implementation is present."],
            "findings": [],
            "questions": []
        }),
        StatusCode::OK,
    )
    .await;
    let report: Value = serde_json::from_str(
        review
            .report
            .content
            .as_deref()
            .expect("ReviewReport content is present"),
    )
    .expect("ReviewReport content is JSON");
    let subject = &report["subject"];
    assert_eq!(
        subject["workspace_id"].as_str(),
        Some(integration.target_workspace_id.as_str())
    );
    assert_eq!(
        subject["head_commit_sha"].as_str(),
        integration.target_after_sha.as_deref(),
        "ReviewReport binds the exact integrated target commit"
    );
    assert!(subject["workspace_snapshot_digest"].as_str().is_some());

    let dependency_digest = hex::encode(sha2::Sha256::digest(b"[]"));
    let gate_policy = json!({
        "schema_version": 1,
        "review": {
            "mode": "one_acceptable",
            "required_count": 1,
            "human_required": true,
            "allow_humans": true,
            "allow_agents": false,
            "allowed_actor_refs": [{"kind":"human","id":"test-user-id"}],
            "candidates": [{
                "artifact_id": review.report.id,
                "digest": review.report.digest,
                "expected_actor": {"kind":"human","id":"test-user-id"},
                "required": true,
                "subject": {
                    "workspace_id": subject["workspace_id"],
                    "base_commit_sha": subject["base_commit_sha"],
                    "head_commit_sha": subject["head_commit_sha"],
                    "workspace_snapshot_digest": subject["workspace_snapshot_digest"]
                }
            }]
        },
        "validations": [],
        "decisions": [],
        "work_units": [{
            "work_unit_id": work_unit.id,
            "version": completed_work_unit.version,
            "dependency_digest": dependency_digest,
            "execution_id": execution.id,
            "execution_result_sha": integration.source_sha,
            "require_integration": true,
            "integration_id": integration.id,
            "integration_version": integration.version
        }]
    });
    let gate: GateResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/gates", predecessor.id),
        json!({"gate_kind":"merge_readiness","policy":gate_policy}),
        StatusCode::OK,
    )
    .await;
    let evaluation: GateEvaluationResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/gates/{}/evaluate", gate.gate.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(evaluation.outcome, "satisfied");
    let events = DomainEventRepo::list_events_after(&*harness.state.db, 0, 200)
        .await
        .expect("GateEvaluation events load");
    let evaluation_event = events
        .iter()
        .find(|event| event.event_type == "gate.evaluated" && event.entity_id == evaluation.id)
        .expect("the exact satisfied GateEvaluation has a durable event");
    services::gate_engine::GateEngine::new(
        harness.state.db.clone(),
        harness.state.event_bus.clone(),
    )
    .process_domain_event(evaluation_event)
    .await
    .expect("exact GateEvaluation applies its lifecycle transition");
    wait_for_task_lifecycle(&harness, &predecessor.id, TaskLifecycleState::ReadyToMerge).await;

    assert!(scheduler_request_for(&harness, &batch.id, &successor.id)
        .await
        .is_none());
    let not_done: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM task_integration_operation WHERE task_id=? AND kind='task_merge')",
    )
    .bind(&predecessor.id)
    .fetch_one(harness.state.db.pool())
    .await
    .expect("no TaskMerge exists before explicit merge");
    assert!(!not_done, "Gate PASS alone does not release the successor");
    assert_eq!(
        scheduler
            .check_once()
            .await
            .expect("pre-merge scheduler tick"),
        0
    );
    assert!(scheduler_request_for(&harness, &batch.id, &successor.id)
        .await
        .is_none());

    let merge: MergeAfterGateResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/merge", predecessor.id),
        json!({"gate_evaluation_id": evaluation.id}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(merge.outcome, "done");
    assert!(merge.after_sha.is_some());
    let accepted =
        wait_for_task_lifecycle(&harness, &predecessor.id, TaskLifecycleState::Done).await;
    assert_eq!(accepted.lifecycle.state, TaskLifecycleState::Done);

    let successful_task_merges: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_integration_operation WHERE task_id=? AND kind='task_merge' AND status='succeeded'",
    )
    .bind(&predecessor.id)
    .fetch_one(harness.state.db.pool())
    .await
    .expect("successful TaskMerge receipt loads");
    assert_eq!(successful_task_merges, 1);

    assert_eq!(
        scheduler
            .check_once()
            .await
            .expect("post-merge scheduler tick"),
        1
    );
    let successor_request = scheduler_request_for(&harness, &batch.id, &successor.id)
        .await
        .expect("successor request appears after the real TaskMerge");
    assert_eq!(
        successor_request.kind,
        db::TaskCoordinationRequestKind::WorkUnitPlan
    );
    let handoff: String = sqlx::query_scalar(
        "SELECT context_json FROM project_plan_handoff WHERE batch_id=? AND task_id=? AND predecessor_task_id=?",
    )
    .bind(&batch.id)
    .bind(&successor.id)
    .bind(&predecessor.id)
    .fetch_one(harness.state.db.pool())
    .await
    .expect("batch pins the exact accepted predecessor handoff");
    let handoff: Value = serde_json::from_str(&handoff).expect("handoff context is JSON");
    let accepted_a = &handoff["direct_predecessors"][0];
    assert_eq!(accepted_a["task_id"], predecessor.id);
    assert_eq!(accepted_a["lifecycle_state"], "done");
    let accepted_unit = accepted_a["work_units"]
        .as_array()
        .and_then(|units| {
            units
                .iter()
                .find(|unit| unit["work_unit_id"] == work_unit.id.as_str())
        })
        .unwrap_or_else(|| panic!("handoff pins the exact implementer WorkUnit: {handoff}"));
    assert_eq!(
        accepted_unit["execution"]["execution_id"], execution.id,
        "handoff: {handoff}"
    );
    assert_eq!(
        accepted_unit["integration"]["commit_sha"].as_str(),
        integration.target_after_sha.as_deref(),
        "handoff: {handoff}"
    );
    // A was reviewed at the exact integrated Task Workspace, not through a
    // WorkUnit review line, so the WorkUnit entry carries no review line.
    assert!(accepted_unit["review"].is_null(), "handoff: {handoff}");
}
