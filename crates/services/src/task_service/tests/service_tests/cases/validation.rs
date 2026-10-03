use super::super::*;
use db::{ValidationRunRepo, ValidationRunStatus, WorkspaceRepo, WorkspaceStatus};

#[tokio::test]
async fn pr8_validation_service_records_exact_pass_fail_stale_and_retry_identity() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(32));
    let validation = crate::ValidationService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let temp = TempDir::new().expect("workspace temp directory creates");
    let worktree_path = temp.path().join("worktree");
    std::fs::create_dir_all(&worktree_path).expect("worktree directory creates");
    git::init(&worktree_path)
        .await
        .expect("git repository initializes");
    std::fs::write(worktree_path.join("README.md"), "baseline\n").expect("baseline writes");
    git::commit_all(&worktree_path, "baseline")
        .await
        .expect("baseline commit creates");
    let workspace = WorkspaceRepo::create(
        &*db,
        db::CreateWorkspace {
            id: db::new_uuid_v4(),
            task_id: task.id.clone(),
            repo_id,
            worktree_path: worktree_path.to_string_lossy().into_owned(),
            branch: ::workspace::task_branch_name(&task.id),
            status: WorkspaceStatus::Ready,
            before_sha: None,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("Workspace records its exact Task and worktree");

    let passed = validation
        .run_command(&task.id, &workspace.id, "printf 'PASS\\n'", 0, None, None)
        .await
        .expect("passing deterministic check runs");
    assert_eq!(passed.run.status, ValidationRunStatus::Passed);
    assert_eq!(passed.run.exit_code, Some(0));
    assert_eq!(passed.run.workspace_id, workspace.id);
    assert_eq!(passed.run.commit_sha.len(), 40);
    assert_eq!(passed.evidence.len(), 1);
    assert_eq!(passed.evidence[0].producer_validation_run_id, passed.run.id);
    assert_eq!(
        passed.run.logs_ref.as_deref(),
        Some(format!("validation-evidence://{}", passed.evidence[0].id).as_str())
    );
    let pass_content: serde_json::Value =
        serde_json::from_str(&passed.evidence[0].content_json).expect("Evidence is JSON");
    assert_eq!(pass_content["status"], "passed");
    assert_eq!(pass_content["commit_sha"], passed.run.commit_sha);
    assert_eq!(
        pass_content["workspace_snapshot_digest"],
        passed.run.workspace_snapshot_digest
    );
    let report = ValidationRunRepo::get_validation_run_artifact_output(&*db, &passed.run.id)
        .await
        .expect("validation-report lookup succeeds")
        .expect("validation-report Artifact exists");
    assert!(matches!(
        report.producer,
        db::ArtifactProducer::ValidationRun { validation_run_id } if validation_run_id == passed.run.id
    ));

    let retried = validation
        .run_command(&task.id, &workspace.id, "printf 'PASS\\n'", 0, None, None)
        .await
        .expect("identical check retry reuses its persisted result");
    assert_eq!(retried.run.id, passed.run.id);
    assert_eq!(retried.evidence[0].id, passed.evidence[0].id);

    let failed = validation
        .run_command(
            &task.id,
            &workspace.id,
            "printf 'FAIL\\n'; exit 17",
            1,
            None,
            None,
        )
        .await
        .expect("failing deterministic check is persisted");
    assert_eq!(failed.run.status, ValidationRunStatus::Failed);
    assert_eq!(failed.run.exit_code, Some(17));

    let stale = validation
        .run_command(
            &task.id,
            &workspace.id,
            "printf 'changed\\n' > README.md",
            2,
            None,
            None,
        )
        .await
        .expect("workspace mutation is reported against its frozen subject");
    assert_eq!(stale.run.status, ValidationRunStatus::Stale);
    assert_eq!(stale.run.exit_code, Some(0));
    let stale_content: serde_json::Value =
        serde_json::from_str(&stale.evidence[0].content_json).expect("stale Evidence is JSON");
    assert_eq!(stale_content["status"], "stale");
    assert_ne!(
        stale_content["workspace_snapshot_digest"],
        stale_content["observed_snapshot_digest_after"]
    );

    let events = db::DomainEventRepo::list_events_after(&*db, 0, 100)
        .await
        .expect("ValidationRun event history reads");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "validation_run.started")
            .count(),
        3
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "evidence.created")
            .count(),
        3
    );
    assert_eq!(
        ValidationRunRepo::list_validation_runs_by_task(&*db, &task.id)
            .await
            .expect("ValidationRun list reads")
            .len(),
        3
    );
}

#[tokio::test]
async fn pr8_workspace_snapshot_v2_distinguishes_staged_and_unstaged_state_and_restores() {
    let temp = TempDir::new().expect("workspace temp directory creates");
    let worktree_path = temp.path().join("worktree");
    std::fs::create_dir_all(&worktree_path).expect("worktree directory creates");
    git::init(&worktree_path)
        .await
        .expect("git repository initializes");
    std::fs::write(worktree_path.join("foo.txt"), "HEAD bytes\n").expect("baseline writes");
    git::commit_all(&worktree_path, "baseline")
        .await
        .expect("baseline commit creates");

    std::fs::write(worktree_path.join("foo.txt"), "same worktree bytes\n")
        .expect("working tree change writes");
    run_workspace_git(&worktree_path, &["add", "foo.txt"]);
    std::fs::write(
        worktree_path.join("same-untracked.txt"),
        "stable untracked bytes\n",
    )
    .expect("untracked content writes");
    let staged_digest = crate::ValidationService::snapshot_digest(&worktree_path.to_string_lossy())
        .await
        .expect("staged snapshot v2 digest computes");
    assert_eq!(
        crate::ValidationService::snapshot_digest(&worktree_path.to_string_lossy())
            .await
            .expect("same logical snapshot computes stably"),
        staged_digest
    );

    let snapshot = git::capture_worktree_state(&worktree_path)
        .await
        .expect("exact pre-review staged state captures");
    std::fs::write(worktree_path.join("foo.txt"), "reviewer mutation\n")
        .expect("reviewer mutation writes");
    std::fs::write(worktree_path.join("reviewer-only.txt"), "temporary\n")
        .expect("reviewer untracked file writes");
    snapshot
        .restore(&worktree_path)
        .await
        .expect("exact staged state restores after reviewer mutation");
    assert_eq!(
        crate::ValidationService::snapshot_digest(&worktree_path.to_string_lossy())
            .await
            .expect("restored v2 digest computes"),
        staged_digest
    );

    run_workspace_git(&worktree_path, &["reset", "HEAD", "--", "foo.txt"]);
    assert_eq!(
        std::fs::read_to_string(worktree_path.join("foo.txt")).unwrap(),
        "same worktree bytes\n",
        "reset changes only the index and preserves the final worktree bytes"
    );
    let unstaged_digest =
        crate::ValidationService::snapshot_digest(&worktree_path.to_string_lossy())
            .await
            .expect("unstaged snapshot v2 digest computes");
    assert_ne!(staged_digest, unstaged_digest);
    assert_eq!(
        crate::ValidationService::snapshot_digest(&worktree_path.to_string_lossy())
            .await
            .expect("same unstaged logical snapshot computes stably"),
        unstaged_digest
    );
}

#[tokio::test]
async fn pr8_validation_idempotency_identity_includes_staged_workspace_state() {
    let db = Arc::new(sqlite_db().await);
    let validation = crate::ValidationService::new(Arc::clone(&db), Arc::new(EventBus::new(32)));
    let (project_id, repo_id, _) = seed_project_repo(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let temp = TempDir::new().expect("workspace temp directory creates");
    let worktree_path = temp.path().join("worktree");
    std::fs::create_dir_all(&worktree_path).expect("worktree directory creates");
    git::init(&worktree_path)
        .await
        .expect("git repository initializes");
    std::fs::write(worktree_path.join("foo.txt"), "HEAD bytes\n").expect("baseline writes");
    git::commit_all(&worktree_path, "baseline")
        .await
        .expect("baseline commit creates");
    let workspace = WorkspaceRepo::create(
        &*db,
        db::CreateWorkspace {
            id: db::new_uuid_v4(),
            task_id: task.id.clone(),
            repo_id,
            worktree_path: worktree_path.to_string_lossy().into_owned(),
            branch: "validation-snapshot-v2".to_owned(),
            status: WorkspaceStatus::Ready,
            before_sha: None,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("Workspace records the validation repository");

    std::fs::write(worktree_path.join("foo.txt"), "same worktree bytes\n")
        .expect("working tree change writes");
    run_workspace_git(&worktree_path, &["add", "foo.txt"]);
    let staged = validation
        .run_command(
            &task.id,
            &workspace.id,
            "printf 'same check\\n'",
            7,
            None,
            None,
        )
        .await
        .expect("staged-state ValidationRun completes");
    run_workspace_git(&worktree_path, &["reset", "HEAD", "--", "foo.txt"]);
    let unstaged = validation
        .run_command(
            &task.id,
            &workspace.id,
            "printf 'same check\\n'",
            7,
            None,
            None,
        )
        .await
        .expect("same check on unstaged-state input creates a new ValidationRun");
    assert_ne!(
        staged.run.workspace_snapshot_digest,
        unstaged.run.workspace_snapshot_digest
    );
    assert_ne!(staged.run.idempotency_key, unstaged.run.idempotency_key);
    assert_ne!(staged.run.id, unstaged.run.id);
}
