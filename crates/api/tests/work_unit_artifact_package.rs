#![allow(dead_code)]

mod common;

use std::time::Duration;

use api_types::{
    ExecutionResponse, ExecutionStatus, ReviewLineResponse, TaskResponse,
    WorkUnitArtifactPackageResponse, WorkUnitResponse,
};
use axum::http::{Method, StatusCode};
use serde_json::{json, Value};
use sha2::Digest;

use common::{
    create_project_and_repo, create_shell_agents, json_request, launch_work_unit_execution,
    setup_git_repo, test_app, TestDir,
};

async fn wait_for_execution_completed(app: &axum::Router, task_id: &str, execution_id: &str) {
    for _ in 0..200 {
        let executions: api_types::PaginatedResponse<ExecutionResponse> = json_request(
            app,
            Method::GET,
            &format!("/api/v1/tasks/{task_id}/executions"),
            json!({}),
            StatusCode::OK,
        )
        .await;
        if executions.items.iter().any(|execution| {
            execution.id == execution_id && execution.status == ExecutionStatus::Completed
        }) {
            return;
        }
        if let Some(execution) = executions.items.iter().find(|execution| {
            execution.id == execution_id
                && matches!(
                    execution.status,
                    ExecutionStatus::Failed | ExecutionStatus::Cancelled
                )
        }) {
            panic!(
                "Execution {} ended in {:?}: {:?}",
                execution_id, execution.status, execution.error
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("Execution {execution_id} did not complete");
}

async fn create_role(app: &axum::Router, task_id: &str, role: &str, configuration: Value) {
    let _: Value = json_request(
        app,
        Method::POST,
        &format!("/api/v1/tasks/{task_id}/task-roles"),
        json!({
            "role": role,
            "coordination_mode": "independent",
            "configuration": configuration,
            "policy": {}
        }),
        StatusCode::OK,
    )
    .await;
}

async fn complete_work_unit(app: &axum::Router, work_unit_id: &str) {
    let current: WorkUnitResponse = json_request(
        app,
        Method::GET,
        &format!("/api/v1/work-units/{work_unit_id}"),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let _: WorkUnitResponse = json_request(
        app,
        Method::POST,
        &format!("/api/v1/work-units/{work_unit_id}/status"),
        json!({"expected_version": current.version, "status": "completed"}),
        StatusCode::OK,
    )
    .await;
}

#[tokio::test]
async fn package_work_unit_pins_five_exact_artifacts_and_exposes_manifest() {
    let repo_dir = TestDir::new("artifact-package-repo");
    let repo_path = setup_git_repo(repo_dir.path());
    let workspaces = TestDir::new("artifact-package-workspaces");
    let harness = test_app(workspaces.path(), "artifact-package").await;
    let (project_id, _) =
        create_project_and_repo(&harness.app, "Artifact package", &repo_path).await;
    let (agent_id, _) =
        create_shell_agents(&harness.app, workspaces.path(), "artifact-package").await;
    let task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({"title": "Package exact delivery artifacts"}),
        StatusCode::OK,
    )
    .await;

    create_role(&harness.app, &task.id, "implementer", json!({})).await;
    create_role(
        &harness.app,
        &task.id,
        "planning-reviewer",
        json!({"capabilities": ["formal_review"]}),
    )
    .await;
    for (role, actor) in [
        ("implementer", json!({"kind": "agent", "id": agent_id})),
        (
            "planning-reviewer",
            json!({"kind": "human", "id": "test-user-id"}),
        ),
    ] {
        let _: Value = json_request(
            &harness.app,
            Method::POST,
            &format!("/api/v1/tasks/{}/task-roles/{role}/members", task.id),
            json!({"actor_ref": actor}),
            StatusCode::OK,
        )
        .await;
    }

    let source: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        json!({
            "title": "Produce D1 to D5",
            "scope": "Produce five exact source documents",
            "role": "implementer",
            "assigned_actor": {"kind": "agent", "id": agent_id},
            "requires_integration": false
        }),
        StatusCode::CREATED,
    )
    .await;
    let source_execution = launch_work_unit_execution(
        &harness.app,
        &task.id,
        &source,
        Some("printf 'source artifacts\\n'"),
    )
    .await;
    wait_for_execution_completed(&harness.app, &task.id, &source_execution.id).await;
    complete_work_unit(&harness.app, &source.id).await;

    let mut source_artifacts = Vec::new();
    for index in 1..=5 {
        let content = format!("D{index} source evidence\n");
        let digest = hex::encode(sha2::Sha256::digest(content.as_bytes()));
        let artifact = harness
            .state
            .collaboration_service
            .create_artifact(
                services::CollaborationActorSource::Execution(source_execution.id.clone()),
                services::CreateArtifactInput {
                    task_id: task.id.clone(),
                    producer_execution_id: source_execution.id.clone(),
                    kind: db::ArtifactKind::DesignDocument,
                    storage_kind: db::ArtifactStorageKind::Inline,
                    content: Some(content),
                    content_ref: None,
                    metadata_json: "{}".to_owned(),
                    digest: Some(digest),
                },
            )
            .await
            .expect("source Artifact is recorded from its exact completed Execution");
        source_artifacts.push(artifact);
    }
    let input_ids: Vec<String> = source_artifacts
        .iter()
        .map(|artifact| artifact.id.clone())
        .collect();
    let package: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        json!({
            "title": "Assemble D1 to D5",
            "scope": "Create one delivery package from these exact five inputs",
            "role": "implementer",
            "assigned_actor": {"kind": "agent", "id": agent_id},
            "requires_integration": false,
            "artifact_inputs": input_ids
        }),
        StatusCode::CREATED,
    )
    .await;
    let package_execution = launch_work_unit_execution(
        &harness.app,
        &task.id,
        &package,
        Some("printf 'compiled package\\n'"),
    )
    .await;
    wait_for_execution_completed(&harness.app, &task.id, &package_execution.id).await;
    let delivery_content = "D1-D5 review package\n";
    let delivery_digest = hex::encode(sha2::Sha256::digest(delivery_content.as_bytes()));
    let delivery = harness
        .state
        .collaboration_service
        .create_artifact(
            services::CollaborationActorSource::Execution(package_execution.id.clone()),
            services::CreateArtifactInput {
                task_id: task.id.clone(),
                producer_execution_id: package_execution.id.clone(),
                kind: db::ArtifactKind::Summary,
                storage_kind: db::ArtifactStorageKind::Inline,
                content: Some(delivery_content.to_owned()),
                content_ref: None,
                metadata_json: "{}".to_owned(),
                digest: Some(delivery_digest.clone()),
            },
        )
        .await
        .expect("delivery Artifact is recorded from the package Execution");

    complete_work_unit(&harness.app, &package.id).await;
    let package: WorkUnitResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/work-units/{}", package.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let reviewer: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        json!({
            "title": "Review package",
            "scope": "Review the exact D1-D5 delivery manifest",
            "role": "planning-reviewer",
            "assigned_actor": {"kind": "human", "id": "test-user-id"},
            "requires_integration": false,
            "review_subject": {"reviewed_work_unit_id": package.id}
        }),
        StatusCode::CREATED,
    )
    .await;

    let execution_inputs: Vec<String> = sqlx::query_scalar(
        "SELECT artifact_id FROM execution_artifact_input WHERE execution_id = ? ORDER BY artifact_id",
    )
    .bind(&package_execution.id)
    .fetch_all(harness.state.db.pool())
    .await
    .expect("package Execution input pins load");
    let mut expected_input_ids = input_ids;
    expected_input_ids.sort();
    assert_eq!(execution_inputs, expected_input_ids);

    let artifact_package: WorkUnitArtifactPackageResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/work-units/{}/artifact-package", package.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(artifact_package.inputs.len(), 5);
    assert_eq!(artifact_package.manifests.len(), 1);
    let manifest = &artifact_package.manifests[0];
    assert_eq!(manifest.work_unit_id, package.id);
    assert_eq!(manifest.result_execution_id, package_execution.id);
    assert_eq!(manifest.delivery_artifact_id, delivery.id);
    assert_eq!(manifest.delivery_artifact_digest, delivery_digest);
    let source_entries = manifest.manifest["source_artifacts"]
        .as_array()
        .expect("manifest carries an immutable list of source Artifacts");
    assert_eq!(source_entries.len(), 5);
    for artifact in &source_artifacts {
        assert!(source_entries.iter().any(|entry| {
            entry["artifact_id"] == artifact.id
                && entry["digest"] == artifact.digest.clone().unwrap_or_default()
                && entry["producer_execution_id"] == source_execution.id
        }));
    }
    assert_eq!(
        manifest.manifest["delivery_artifact"]["producer_execution_id"],
        package_execution.id
    );

    let lines: Vec<ReviewLineResponse> = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/review-lines", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let line = lines
        .iter()
        .find(|line| line.root_work_unit_id == package.id)
        .expect("the exact reviewed package has a durable review line");
    assert_eq!(line.current_work_unit_id, package.id);
    assert_eq!(
        line.current_manifest_id.as_deref(),
        Some(manifest.id.as_str())
    );
    assert_eq!(line.latest_verdict, None);
    assert_eq!(
        reviewer
            .review_subject
            .as_ref()
            .unwrap()
            .reviewed_work_unit_id,
        package.id
    );

    let _: Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/work-units/{}/review-report", reviewer.id),
        json!({
            "expected_version": reviewer.version,
            "report": {
                "verdict": "pass",
                "summary": "The exact D1-D5 package is complete.",
                "criteria": ["all five source Artifacts are present"],
                "findings": [],
                "questions": [],
                "evidence_ids": [],
                "artifact_ids": []
            }
        }),
        StatusCode::OK,
    )
    .await;
    let passing_lines: Vec<ReviewLineResponse> = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/review-lines", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let passing_line = passing_lines
        .iter()
        .find(|candidate| candidate.id == line.id)
        .expect("review line remains stable after the report");
    assert_eq!(passing_line.latest_verdict.as_deref(), Some("pass"));

    let policy = json!({
        "schema_version": 1,
        "review": {
            "mode": "one_acceptable",
            "required_count": 1,
            "human_required": true,
            "allow_humans": true,
            "allow_agents": false,
            "allowed_actor_refs": [],
            "review_line_id": line.id,
            "candidates": []
        },
        "validations": [],
        "decisions": [],
        "work_units": []
    });
    let gate: Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/gates", task.id),
        json!({"gate_kind": "delivery_acceptance", "policy": policy}),
        StatusCode::OK,
    )
    .await;
    let gate_id = gate["gate"]["id"].as_str().expect("Gate ID").to_owned();
    let consumer: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        json!({
            "title": "Deliver to Product Owner",
            "scope": "Deliver only the approved D1-D5 package",
            "role": "implementer",
            "assigned_actor": {"kind": "agent", "id": agent_id},
            "requires_integration": false,
            "review_approval": {"gate_id": gate_id, "review_line_id": line.id}
        }),
        StatusCode::CREATED,
    )
    .await;
    assert!(consumer.readiness.awaiting_review_approval);

    let evaluation: Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/gates/{gate_id}/evaluate"),
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(evaluation["outcome"], "satisfied");
    let ready_consumer: WorkUnitResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/work-units/{}", consumer.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert!(!ready_consumer.readiness.awaiting_review_approval);
    assert!(ready_consumer.readiness.runnable);

    let consumer_execution = launch_work_unit_execution(
        &harness.app,
        &task.id,
        &ready_consumer,
        Some("printf 'approved delivery\\n'"),
    )
    .await;
    wait_for_execution_completed(&harness.app, &task.id, &consumer_execution.id).await;

    let roles: Value = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/task-roles", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let find_role = |name: &str| {
        roles
            .as_array()
            .and_then(|roles| roles.iter().find(|role| role["role"] == name))
            .expect("TaskRole exists")
    };
    let implementer_role_id = find_role("implementer")["id"]
        .as_str()
        .expect("implementer TaskRole ID")
        .to_owned();
    let reviewer_role = find_role("planning-reviewer");
    let reviewer_role_id = reviewer_role["id"]
        .as_str()
        .expect("reviewer TaskRole ID")
        .to_owned();
    let _: Value = json_request(
        &harness.app,
        Method::PATCH,
        &format!("/api/v1/tasks/{}/task-roles/planning-reviewer", task.id),
        json!({
            "expected_version": reviewer_role["version"],
            "configuration": {
                "schema_version": 1,
                "display_label": "Planning reviewer",
                "capabilities": ["formal_review", "request_rework"],
                "rework": {
                    "mode": "manual",
                    "allowed_destination_role_ids": [implementer_role_id],
                    "next_review_role_id": reviewer_role_id,
                    "correction_actor": {
                        "kind": "explicit",
                        "actor_ref": {"kind": "agent", "id": agent_id}
                    },
                    "review_actor": {"kind": "human", "id": "test-user-id"}
                }
            }
        }),
        StatusCode::OK,
    )
    .await;

    let follow_up_review: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        json!({
            "title": "Recheck the approved package",
            "scope": "Check whether the exact D1-D5 package still needs correction",
            "role": "planning-reviewer",
            "assigned_actor": {"kind": "human", "id": "test-user-id"},
            "requires_integration": false,
            "review_subject": {"reviewed_work_unit_id": package.id}
        }),
        StatusCode::CREATED,
    )
    .await;
    let follow_up_report: Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/work-units/{}/review-report", follow_up_review.id),
        json!({
            "expected_version": follow_up_review.version,
            "report": {
                "verdict": "request_changes",
                "summary": "The package needs one more correction.",
                "criteria": ["complete evidence"],
                "findings": ["D4 needs correction"],
                "questions": [],
                "evidence_ids": [],
                "artifact_ids": []
            }
        }),
        StatusCode::OK,
    )
    .await;
    let reviewed_result_version = follow_up_review
        .review_subject
        .as_ref()
        .and_then(|subject| subject.reviewed_work_unit_version)
        .expect("follow-up review pins the exact completed package Execution version");
    let current_task: TaskResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let rework: Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/rework", task.id),
        json!({
            "expected_task_version": current_task.version,
            "expected_lifecycle_version": current_task.lifecycle.version,
            "review_report_artifact_id": follow_up_report["report"]["id"],
            "reviewed_work_unit_id": package.id,
            "reviewed_work_unit_version": reviewed_result_version,
            "correction_title": "Correct D4",
            "correction_scope": "Apply the exact D4 correction",
            "correction_role_id": implementer_role_id,
            "correction_actor": {"kind": "agent", "id": agent_id},
            "review_title": "Review corrected D4",
            "review_scope": "Review the exact corrected package result",
            "review_actor": {"kind": "human", "id": "test-user-id"},
            "idempotency_key": "package-rework-round-1"
        }),
        StatusCode::OK,
    )
    .await;
    let correction_id = rework["correction_work_unit"]["id"]
        .as_str()
        .expect("correction WorkUnit ID")
        .to_owned();
    let corrected_lines: Vec<ReviewLineResponse> = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/review-lines", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let corrected_line = corrected_lines
        .iter()
        .find(|candidate| candidate.id == line.id)
        .expect("correction advances the same durable review line");
    assert_eq!(corrected_line.current_work_unit_id, correction_id);
    assert_eq!(corrected_line.latest_verdict, None);
    assert!(corrected_line.revision > passing_line.revision);
    let old_gate_revision: i64 = sqlx::query_scalar(
        "SELECT input_version FROM gate_evaluation_input
         WHERE evaluation_id = ? AND input_kind = 'review_line' AND input_id = ?",
    )
    .bind(evaluation["id"].as_str().expect("GateEvaluation ID"))
    .bind(&line.id)
    .fetch_one(harness.state.db.pool())
    .await
    .expect("Gate evaluation pins its exact review-line revision");
    assert_eq!(old_gate_revision, passing_line.revision);
    assert_ne!(old_gate_revision, corrected_line.revision);

    let post_correction_consumer: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        json!({
            "title": "Deliver corrected package",
            "scope": "Wait for the latest package line to pass",
            "role": "implementer",
            "assigned_actor": {"kind": "agent", "id": agent_id},
            "requires_integration": false,
            "review_approval": {"gate_id": gate_id, "review_line_id": line.id}
        }),
        StatusCode::CREATED,
    )
    .await;
    assert!(post_correction_consumer.readiness.awaiting_review_approval);
    let preflight: Value = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/dispatch", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert!(preflight["work_units"].as_array().is_some_and(|units| {
        units.iter().any(|unit| {
            unit["work_unit_id"] == post_correction_consumer.id
                && unit["state"] == "awaiting_review_approval"
        })
    }));

    let correction: WorkUnitResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/work-units/{correction_id}"),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let correction_execution = launch_work_unit_execution(
        &harness.app,
        &task.id,
        &correction,
        Some("printf 'corrected D1-D5 package\\n'; exit 0"),
    )
    .await;
    wait_for_execution_completed(&harness.app, &task.id, &correction_execution.id).await;
    let corrected_content = "Corrected D1-D5 package\n";
    let corrected_digest = hex::encode(sha2::Sha256::digest(corrected_content.as_bytes()));
    let corrected_delivery = harness
        .state
        .collaboration_service
        .create_artifact(
            services::CollaborationActorSource::Execution(correction_execution.id.clone()),
            services::CreateArtifactInput {
                task_id: task.id.clone(),
                producer_execution_id: correction_execution.id.clone(),
                kind: db::ArtifactKind::Summary,
                storage_kind: db::ArtifactStorageKind::Inline,
                content: Some(corrected_content.to_owned()),
                content_ref: None,
                metadata_json: "{}".to_owned(),
                digest: Some(corrected_digest.clone()),
            },
        )
        .await
        .expect("corrected delivery Artifact is recorded from C1's exact Execution");
    complete_work_unit(&harness.app, &correction_id).await;

    let corrected_review_id = rework["review_work_unit"]["id"]
        .as_str()
        .expect("correction has its paired review WorkUnit")
        .to_owned();
    let corrected_review: WorkUnitResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/work-units/{corrected_review_id}"),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let _: Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/work-units/{corrected_review_id}/review-report"),
        json!({
            "expected_version": corrected_review.version,
            "report": {
                "verdict": "pass",
                "summary": "The corrected D1-D5 package now contains the exact D4 evidence.",
                "criteria": ["all five source Artifacts remain pinned"],
                "findings": [],
                "questions": [],
                "evidence_ids": [],
                "artifact_ids": []
            }
        }),
        StatusCode::OK,
    )
    .await;

    let corrected_artifact_package: WorkUnitArtifactPackageResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/work-units/{correction_id}/artifact-package"),
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(corrected_artifact_package.inputs.len(), 5);
    assert_eq!(corrected_artifact_package.manifests.len(), 1);
    let corrected_manifest = &corrected_artifact_package.manifests[0];
    assert_eq!(corrected_manifest.work_unit_id, correction_id);
    assert_eq!(
        corrected_manifest.result_execution_id,
        correction_execution.id
    );
    assert_eq!(
        corrected_manifest.delivery_artifact_id,
        corrected_delivery.id
    );
    assert_eq!(
        corrected_manifest.delivery_artifact_digest,
        corrected_digest
    );
    assert_ne!(corrected_manifest.id, manifest.id);
    assert_eq!(
        corrected_manifest.manifest["source_artifacts"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
    let original_artifact_package: WorkUnitArtifactPackageResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/work-units/{}/artifact-package", package.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(original_artifact_package.manifests.len(), 1);
    assert_eq!(original_artifact_package.manifests[0].id, manifest.id);

    let passing_lines: Vec<ReviewLineResponse> = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/review-lines", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let approved_line = passing_lines
        .iter()
        .find(|candidate| candidate.id == line.id)
        .expect("the same review line advances to the corrected package");
    assert_eq!(approved_line.current_work_unit_id, correction_id);
    assert_eq!(approved_line.latest_verdict.as_deref(), Some("pass"));
    assert_eq!(
        approved_line.current_manifest_id.as_deref(),
        Some(corrected_manifest.id.as_str())
    );
    assert!(approved_line.revision > corrected_line.revision);

    let corrected_evaluation: Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/gates/{gate_id}/evaluate"),
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(corrected_evaluation["outcome"], "satisfied");
    let final_consumer: WorkUnitResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/work-units/{}", post_correction_consumer.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert!(!final_consumer.readiness.awaiting_review_approval);
    assert!(final_consumer.readiness.runnable);
    let final_consumer_execution = launch_work_unit_execution(
        &harness.app,
        &task.id,
        &final_consumer,
        Some("printf 'corrected package approved for delivery\\n'"),
    )
    .await;
    wait_for_execution_completed(&harness.app, &task.id, &final_consumer_execution.id).await;
    let corrected_gate_input_version: i64 = sqlx::query_scalar(
        "SELECT input.input_version FROM gate_evaluation_input input
         WHERE input.evaluation_id = ? AND input.input_kind = 'review_line'
           AND input.input_id = ?",
    )
    .bind(
        corrected_evaluation["id"]
            .as_str()
            .expect("corrected GateEvaluation ID"),
    )
    .bind(&line.id)
    .fetch_one(harness.state.db.pool())
    .await
    .expect("corrected Gate evaluation pins the passing review line revision");
    assert_eq!(corrected_gate_input_version, approved_line.revision);
}
