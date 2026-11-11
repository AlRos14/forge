use super::*;

use db::{
    AgentRepo, AgentStatus, ArtifactKind, CreateAgent, CreateRepo, DaemonRepo, DaemonStatus,
    ProjectTaskRoleDefault, ProjectTaskRoleDefaultsRepo, RepoRepo, TaskCoordinationRepo, TaskRepo,
    UpdateDaemonReport, UpdateProject, UpsertDaemon, WorkMode, WorkUnitRepo,
};
use serde_json::json;
use std::{path::Path, process::Command, sync::Arc, time::Duration};

#[derive(Clone)]
struct ProjectBatchHarnessAdapter {
    plan: String,
    observations: Arc<std::sync::Mutex<Vec<(String, String, String)>>>,
}

#[async_trait::async_trait]
impl executors::HarnessAdapter for ProjectBatchHarnessAdapter {
    fn kind(&self) -> executors::ExecutorKind {
        executors::ExecutorKind::Codex
    }

    async fn discover_options(
        &self,
        _context: executors::DiscoverContext,
    ) -> std::result::Result<executors::DiscoveredOptions, executors::ExecutorError> {
        Ok(executors::DiscoveredOptions::default())
    }

    async fn execute(
        &self,
        context: executors::ExecutionContext,
    ) -> std::result::Result<executors::ExecutionResult, executors::ExecutorError> {
        let output = match context.role.as_str() {
            "orchestrator" => self.plan.clone(),
            "implementer" => "Implementation delivered for the exact WorkUnit scope.".to_owned(),
            "reviewer" => "FORGE_RESULT: {\"schema_version\":1,\"kind\":\"review\",\"verdict\":\"pass\",\"summary\":\"The exact implementation result meets its acceptance criterion.\",\"criteria\":[\"correctness\"],\"findings\":[],\"questions\":[],\"evidence_considered\":[]}".to_owned(),
            role => {
                return Err(executors::ExecutorError::Other(format!(
                    "unexpected Project batch harness role {role:?}"
                )))
            }
        };
        let native_session = format!("fake-native-session-{}", context.execution_id);
        self.observations
            .lock()
            .expect("fake Harness observation mutex locks")
            .push((context.role, context.execution_id, native_session.clone()));
        Ok(executors::ExecutionResult {
            status: executors::ExecutionOutcome::Completed,
            assistant_output: Some(output.clone()),
            summary: Some(output),
            agent_session_id: Some(native_session),
            ..Default::default()
        })
    }

    async fn cancel(
        &self,
        _execution_id: &str,
    ) -> std::result::Result<(), executors::ExecutorError> {
        Ok(())
    }
}

fn project_role_default(
    role: &str,
    coordination_mode: db::CoordinationMode,
    capabilities: Vec<api_types::TaskRoleCapability>,
    agent_id: &str,
    coordinator: bool,
) -> ProjectTaskRoleDefault {
    ProjectTaskRoleDefault {
        role: role.to_owned(),
        coordination_mode,
        configuration_json: serde_json::to_string(&api_types::ProjectTaskRoleConfiguration {
            schema_version: 1,
            display_label: None,
            capabilities,
            rework: None,
            coordinator_actor: coordinator.then(|| api_types::ActorRef::Agent(agent_id.to_owned())),
        })
        .expect("Project TaskRole configuration serializes"),
        members: vec![ActorRef::Agent(agent_id.to_owned())],
    }
}

fn init_git_repo(path: &Path) -> String {
    std::fs::create_dir_all(path).expect("temporary repository directory creates");
    for args in [
        vec!["init"],
        vec!["config", "user.email", "forge-test@example.test"],
        vec!["config", "user.name", "Forge Test"],
    ] {
        let output = Command::new("git")
            .args(&args)
            .current_dir(path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .expect("git fixture command starts");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    std::fs::write(path.join("README.md"), "# Project batch fixture\n")
        .expect("temporary repository initial file writes");
    for args in [vec!["add", "README.md"], vec!["commit", "-m", "initial"]] {
        let output = Command::new("git")
            .args(&args)
            .current_dir(path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .expect("git fixture commit command starts");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(path)
        .output()
        .expect("git branch lookup starts");
    assert!(output.status.success(), "git branch lookup succeeds");
    String::from_utf8(output.stdout)
        .expect("git branch is UTF-8")
        .trim()
        .to_owned()
}

async fn add_project_repo(
    database: &Arc<db::SqliteDb>,
    project_id: &str,
    repository_root: &Path,
) -> String {
    let branch = init_git_repo(repository_root);
    let repo_id = new_uuid_v4();
    let now = now_rfc3339();
    RepoRepo::create(
        &**database,
        CreateRepo {
            id: repo_id.clone(),
            project_id: project_id.to_owned(),
            name: "project-batch-fixture".to_owned(),
            remote_url: repository_root.to_string_lossy().into_owned(),
            local_path: Some(repository_root.to_string_lossy().into_owned()),
            work_mode: WorkMode::DirectMerge,
            default_branch: branch,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("temporary test Repo persists");
    db::ProjectRepo::update(
        &**database,
        UpdateProject {
            id: project_id.to_owned(),
            name: None,
            settings: None,
            primary_repo_id: Some(Some(repo_id.clone())),
            paused_at: None,
            updated_at: now,
        },
    )
    .await
    .expect("test Project selects its isolated Repo");
    repo_id
}

async fn add_embedded_codex_agent(database: &Arc<db::SqliteDb>, owner_id: &str) -> String {
    let now = now_rfc3339();
    let daemon_id = new_uuid_v4();
    DaemonRepo::upsert_by_machine_id(
        &**database,
        UpsertDaemon {
            id: daemon_id.clone(),
            machine_id: crate::embedded_daemon::embedded_machine_id(),
            hostname: "project-batch-fixture".to_owned(),
            os: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            agent_version: None,
            labels_json: "{}".to_owned(),
            status: DaemonStatus::Online,
            registration_token_hash: None,
            owner_id: Some(owner_id.to_owned()),
            visibility: "global".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("embedded test execution host creates");
    DaemonRepo::update_report(
        &**database,
        UpdateDaemonReport {
            id: daemon_id.clone(),
            detected_clis_json: r#"[{"kind":"codex","availability":"authenticated"}]"#.to_owned(),
            labels_json: None,
            status: DaemonStatus::Online,
            last_report_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("embedded host advertises the fake Codex route");
    let agent_id = new_uuid_v4();
    AgentRepo::create(
        &**database,
        CreateAgent {
            id: agent_id.clone(),
            name: "Project batch fake Harness".to_owned(),
            description: None,
            executor_type: "codex".to_owned(),
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            prompt_template: None,
            capabilities_json: "[]".to_owned(),
            config_json: "{}".to_owned(),
            credential_ref: None,
            daemon_id: Some(daemon_id),
            max_concurrent_tasks: 4,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: AgentStatus::Idle,
            last_heartbeat_at: None,
            is_default: false,
            paused: false,
            owner_id: Some(owner_id.to_owned()),
            visibility: "global".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("fake Harness Agent creates");
    agent_id
}

#[tokio::test]
async fn project_batch_coordinator_materializes_work_and_binds_worker_review_and_reports_to_exact_origin(
) {
    let pool = db::create_sqlite_pool("sqlite::memory:")
        .await
        .expect("isolated SQLite pool creates");
    db::run_migrations(&pool)
        .await
        .expect("isolated schema migrates");
    let database = Arc::new(db::SqliteDb::new(pool));
    let event_bus = Arc::new(EventBus::new(64));
    let owner_id = test_user(&database, "Project batch owner").await;
    let project_id = test_project(&database, &owner_id).await;
    let repo_root = tempfile::tempdir().expect("isolated repository root creates");
    add_project_repo(&database, &project_id, repo_root.path()).await;
    let agent_id = add_embedded_codex_agent(&database, &owner_id).await;

    let defaults = db::ProjectTaskRoleDefaultsRepo::get_task_role_defaults(&*database, &project_id)
        .await
        .expect("Project TaskRole defaults load");
    ProjectTaskRoleDefaultsRepo::replace_task_role_defaults(
        &*database,
        &project_id,
        &owner_id,
        defaults.version,
        vec![
            project_role_default(
                "orchestrator",
                db::CoordinationMode::Collaborative,
                vec![api_types::TaskRoleCapability::Orchestration],
                &agent_id,
                true,
            ),
            project_role_default(
                "implementer",
                db::CoordinationMode::Independent,
                Vec::new(),
                &agent_id,
                false,
            ),
            project_role_default(
                "reviewer",
                db::CoordinationMode::Independent,
                vec![api_types::TaskRoleCapability::FormalReview],
                &agent_id,
                false,
            ),
        ],
        &now_rfc3339(),
    )
    .await
    .expect("Project stores exact coordinator, implementer, and reviewer defaults");

    let task_creation_service = TaskService::new(Arc::clone(&database), Arc::clone(&event_bus));
    let task = task_creation_service
        .create_task(
            project_id.clone(),
            "Deliver a reviewed Project packet",
            Some(
                "Use one Agent for the bounded implementation and its independent formal review."
                    .to_owned(),
            ),
            None,
            Some(0),
            Some(db::TaskLifecycleState::Ready),
            None,
        )
        .await
        .expect("TaskService creates a Repo-bound Task from Project defaults");
    let initial_coordination = TaskCoordinationRepo::get_task_coordination(&*database, &task.id)
        .await
        .expect("initial TaskCoordination loads")
        .expect("TaskService creates manual TaskCoordination");
    assert_eq!(initial_coordination.mode, db::TaskCoordinationMode::Manual);
    assert!(initial_coordination.requested_by.is_none());
    assert!(initial_coordination.revoked_at.is_none());
    assert!(initial_coordination.team_stale);
    assert_eq!(
        initial_coordination.coordinator_actor,
        Some(ActorRef::Agent(agent_id.clone()))
    );
    let initial_envelope: crate::task_coordination::CoordinationEnvelope =
        serde_json::from_str(&initial_coordination.envelope_json)
            .expect("new TaskCoordination stores its default envelope");
    assert_eq!(
        initial_envelope,
        crate::task_coordination::CoordinationEnvelope::default(),
        "the copied Project role defaults do not create a Human-customized envelope"
    );

    let project_plan =
        crate::project_plan::ProjectPlanService::new(Arc::clone(&database), Arc::clone(&event_bus));
    let graph = project_plan
        .graph(&project_id)
        .await
        .expect("Project plan graph loads before authorization");
    let batch = project_plan
        .authorize_batch(
            &project_id,
            &owner_id,
            graph.version,
            vec![task.id.clone()],
            1,
            "project-batch-positive-runtime",
        )
        .await
        .expect("Project owner authorizes exact Task batch");
    assert_eq!(batch.state, "running");

    // Task/default setup and batch authorization are fixture inputs. The
    // scheduler's request is the first event this runtime processes.
    set_orchestrator_cursor_to_head(&database).await;
    assert_eq!(
        project_plan
            .check_once()
            .await
            .expect("batch scheduler creates its exact coordination request"),
        1
    );
    let request_id: String = sqlx::query_scalar(
        "SELECT request_id FROM project_plan_batch_coordination_request WHERE batch_id=? AND task_id=?",
    )
    .bind(&batch.id)
    .bind(&task.id)
    .fetch_one(database.pool())
    .await
    .expect("scheduler request is durably linked to the batch Task");
    let request = TaskCoordinationRepo::get_task_coordination_request(&*database, &request_id)
        .await
        .expect("scheduled request loads")
        .expect("scheduled request exists");
    assert_eq!(
        request.authorization_version,
        initial_coordination.authorization_version + 1
    );

    let task = TaskRepo::get_by_id(&*database, &task.id, false)
        .await
        .expect("Task lookup succeeds")
        .expect("batch Task remains current");
    let coordination = TaskCoordinationRepo::get_task_coordination(&*database, &task.id)
        .await
        .expect("authorized TaskCoordination loads")
        .expect("batch TaskCoordination exists");
    let plan = json!({
        "schema_version": 1,
        "request_id": request.id,
        "task_id": task.id,
        "task_version": task.version,
        "authorization_version": coordination.authorization_version,
        "team_digest": coordination.team_digest,
        "envelope_digest": coordination.envelope_digest,
        "objective_digest": request.objective_digest,
        "work_units": [
            {
                "key": "implementation",
                "title": "Implement the bounded change",
                "scope": "Produce one exact implementation result for independent review.",
                "role": "implementer",
                "assigned_actor": {"kind": "agent", "id": agent_id},
                "parent_key": null,
                "depends_on": [],
                "requires_integration": false,
                "execution_instructions": "Implement the accepted scope and return a concise result.",
                "review_subject_key": null,
                "acceptance": ["The requested behavior is present."],
                "artifact_inputs": []
            },
            {
                "key": "formal-review",
                "title": "Review the exact implementation result",
                "scope": "Check the implementation against its acceptance criterion and return a structured ReviewReport.",
                "role": "reviewer",
                "assigned_actor": {"kind": "agent", "id": agent_id},
                "parent_key": null,
                "depends_on": ["implementation"],
                "requires_integration": false,
                "execution_instructions": "Review the exact implementation result and report only supported findings.",
                "review_subject_key": "implementation",
                "acceptance": ["The exact implementation result meets its acceptance criterion."],
                "artifact_inputs": []
            }
        ],
        "delivery": null,
        "review": null,
        "limits": null
    })
    .to_string();

    let observations = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut registry = executors::HarnessAdapterRegistry::new();
    registry.register(Box::new(ProjectBatchHarnessAdapter {
        plan,
        observations: Arc::clone(&observations),
    }));
    let registry = Arc::new(registry);
    let workspace_root = tempfile::tempdir().expect("isolated workspace root creates");
    let task_service = Arc::new(
        TaskService::new(Arc::clone(&database), Arc::clone(&event_bus))
            .with_workspace_root(workspace_root.path().to_path_buf())
            .with_workspace_exec_locks(Arc::new(crate::WorkspaceExecutionLockManager::new()))
            .with_task_executor(Arc::new(executors::AdapterExecutor::new(Arc::clone(
                &registry,
            ))))
            .with_adapter_registry(registry),
    );
    let runtime = OrchestratorRuntime::new(
        Arc::clone(&database),
        Arc::clone(&event_bus),
        Arc::clone(&task_service),
    );
    let run = runtime.run_once(100).await.expect(
        "OrchestratorRuntime dispatches the batch coordinator turn through the fake Harness",
    );
    assert_eq!(
        run.admitted_wakes, 1,
        "only the exact batch request is admitted"
    );
    assert_eq!(run.dispatched_wakes, 1, "the batch coordinator starts once");

    let mut materialized_plan_id = None;
    let mut coordinator_execution_id = None;
    for _ in 0..200 {
        let current = TaskCoordinationRepo::get_task_coordination_request(&*database, &request_id)
            .await
            .expect("batch request status loads")
            .expect("batch request remains durable");
        if current.state == db::TaskCoordinationRequestState::Materialized {
            materialized_plan_id = current.plan_artifact_id;
            coordinator_execution_id = current.execution_id;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let plan_artifact_id = materialized_plan_id
        .expect("fake cognitive plan is persisted and materialized by TaskCoordinationService");
    let coordinator_execution_id = coordinator_execution_id
        .expect("materialized WorkUnitPlan identifies its coordinator Execution");
    let plan_record =
        TaskCoordinationRepo::get_task_coordination_plan(&*database, &plan_artifact_id)
            .await
            .expect("materialized plan record loads")
            .expect("real WorkUnitPlan record persists");
    assert_eq!(
        plan_record.state,
        db::TaskCoordinationPlanState::Materialized
    );
    assert_eq!(plan_record.request_id, request_id);
    assert_eq!(
        plan_record.authorization_version,
        coordination.authorization_version
    );
    let plan_artifact = db::CollaborationRepo::get_artifact(&*database, &plan_artifact_id)
        .await
        .expect("WorkUnitPlan artifact lookup succeeds")
        .expect("fake Harness output is materialized as an Artifact");
    assert_eq!(plan_artifact.kind, ArtifactKind::WorkUnitPlan);
    let materialization =
        TaskCoordinationRepo::get_task_coordination_materialization(&*database, &plan_artifact_id)
            .await
            .expect("materialization receipt lookup succeeds")
            .expect("real WorkUnits have a durable plan materialization receipt");
    assert_eq!(materialization.task_id, task.id);
    assert_eq!(materialization.request_id, request_id);
    assert_eq!(materialization.plan_artifact_id, plan_artifact_id);
    assert_eq!(materialization.plan_digest, plan_record.plan_digest);
    assert_eq!(
        materialization.authorization_version,
        coordination.authorization_version
    );
    assert_eq!(
        plan_artifact
            .execution_producer()
            .map(|(id, _)| id.to_owned()),
        Some(coordinator_execution_id.clone()),
        "the exact coordinator Execution produces the persisted plan Artifact"
    );

    let task_after_materialization = TaskRepo::get_by_id(&*database, &task.id, false)
        .await
        .expect("Task version loads after materialization")
        .expect("batch Task remains current");
    let human_replan_binding = crate::TaskCoordinationService::new(
        Arc::clone(&database),
        Arc::clone(&event_bus),
    )
    .project_batch_binding_for_approved_plan(
        &task.id,
        &owner_id,
        &plan_artifact_id,
        task_after_materialization.version,
        coordination.authorization_version,
    )
    .await
    .expect("exact materialized plan binding lookup succeeds")
    .expect(
        "the explicitly selected plan has approval and receipt exact to this current authorization",
    );
    assert_eq!(human_replan_binding.batch_id, batch.id);
    assert_eq!(human_replan_binding.task_id, task.id);
    assert_eq!(
        human_replan_binding.expected_coordination_version,
        coordination.authorization_version
    );
    assert!(matches!(
        human_replan_binding.origin,
        db::ProjectPlanBatchRequestOrigin::HumanReplan { approved_plan_artifact_id: id }
            if id == plan_artifact_id
    ));

    let dispatcher = crate::TaskDispatcher::new(
        Arc::clone(&database),
        Arc::clone(&event_bus),
        Arc::clone(&task_service),
    );
    let mut report_artifact = None;
    for _ in 0..200 {
        let _ = dispatcher
            .check_once()
            .await
            .expect("TaskDispatcher admits the next ready materialized WorkUnit");
        let execution_rows = sqlx::query(
            "SELECT e.id,e.role,e.status,a.id AS artifact_id
             FROM execution e
             JOIN work_unit w ON w.id=e.work_unit_id
             LEFT JOIN artifact_execution_producer producer ON producer.execution_id=e.id
             LEFT JOIN artifact a ON a.id=producer.artifact_id AND a.kind='review_report'
             WHERE e.task_id=? AND w.provenance_kind='artifact' AND w.provenance_id=?
             ORDER BY e.created_at,e.id",
        )
        .bind(&task.id)
        .bind(&plan_artifact_id)
        .fetch_all(database.pool())
        .await
        .expect("materialized WorkUnit Executions and report links load");
        report_artifact = execution_rows.iter().find_map(|row| {
            let role: String = row.try_get("role").expect("Execution role reads");
            (role == "reviewer")
                .then(|| {
                    row.try_get::<Option<String>, _>("artifact_id")
                        .expect("report Artifact ID reads")
                })
                .flatten()
        });
        if report_artifact.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let report_artifact_id = report_artifact
        .expect("Worker and reviewer Executions complete through the fake Harness and produce a ReviewReport");

    let execution_rows = sqlx::query(
        "SELECT e.id,e.role,e.status,e.work_unit_id,link.batch_id,link.authorization_version
         FROM execution e
         JOIN work_unit w ON w.id=e.work_unit_id
         JOIN project_plan_batch_execution link ON link.execution_id=e.id
         WHERE e.task_id=? AND w.provenance_kind='artifact' AND w.provenance_id=?
         ORDER BY e.role",
    )
    .bind(&task.id)
    .bind(&plan_artifact_id)
    .fetch_all(database.pool())
    .await
    .expect("exact batch-origin Worker and reviewer Executions load");
    assert_eq!(
        execution_rows.len(),
        2,
        "one Worker and one formal reviewer execute"
    );
    let mut work_unit_execution_ids = Vec::new();
    for row in &execution_rows {
        let role: String = row.try_get("role").expect("Execution role reads");
        let status: String = row.try_get("status").expect("Execution status reads");
        let linked_batch: String = row.try_get("batch_id").expect("batch provenance reads");
        let authorization_version: i64 = row
            .try_get("authorization_version")
            .expect("batch authorization version reads");
        let execution_id: String = row.try_get("id").expect("Execution ID reads");
        assert!(matches!(role.as_str(), "implementer" | "reviewer"));
        assert_eq!(status, "completed");
        assert_eq!(linked_batch, batch.id);
        assert_eq!(authorization_version, coordination.authorization_version);
        work_unit_execution_ids.push(execution_id);
    }
    let roles = execution_rows
        .iter()
        .map(|row| {
            row.try_get::<String, _>("role")
                .expect("Execution role reads")
        })
        .collect::<std::collections::HashSet<_>>();
    assert!(roles.contains("implementer"));
    assert!(roles.contains("reviewer"));

    let report = db::CollaborationRepo::get_artifact(&*database, &report_artifact_id)
        .await
        .expect("ReviewReport Artifact loads")
        .expect("formal reviewer produced a durable report");
    assert_eq!(report.kind, ArtifactKind::ReviewReport);
    let report_producer_id = report
        .execution_producer()
        .map(|(id, _)| id.to_owned())
        .expect("ReviewReport names its exact producer Execution");
    assert!(work_unit_execution_ids.contains(&report_producer_id));

    let current_task = TaskRepo::get_by_id(&*database, &task.id, false)
        .await
        .expect("current Task loads after delivery and review")
        .expect("batch Task still exists");
    let current_coordination = TaskCoordinationRepo::get_task_coordination(&*database, &task.id)
        .await
        .expect("current TaskCoordination loads")
        .expect("batch authorization remains current");
    let quality_binding =
        crate::TaskCoordinationService::new(Arc::clone(&database), Arc::clone(&event_bus))
            .project_batch_binding_for_quality_source(
                &task.id,
                &owner_id,
                &report_artifact_id,
                current_task.version,
                current_coordination.authorization_version,
            )
            .await
            .expect("exact ReviewReport batch-origin lookup succeeds")
            .expect("ReviewReport resolves to the running batch that admitted its producer");
    assert_eq!(quality_binding.batch_id, batch.id);
    assert_eq!(quality_binding.task_id, task.id);
    assert_eq!(
        quality_binding.expected_coordination_version,
        coordination.authorization_version
    );
    assert!(matches!(
        quality_binding.origin,
        db::ProjectPlanBatchRequestOrigin::QualityReplan {
            source_artifact_id,
            producer_execution_id,
        } if source_artifact_id == report_artifact_id
            && producer_execution_id == report_producer_id
    ));

    let scoped_request: (String, Option<String>, i64, String, String) = sqlx::query_as(
        "SELECT origin_kind,claim_token,authorization_version,batch_id,task_id
         FROM project_plan_batch_coordination_request WHERE request_id=?",
    )
    .bind(&request_id)
    .fetch_one(database.pool())
    .await
    .expect("scheduler origin and authorization version remain inspectable");
    assert_eq!(scoped_request.0, "scheduler");
    assert!(scoped_request.1.is_some_and(|token| !token.is_empty()));
    assert_eq!(scoped_request.2, coordination.authorization_version);
    assert_eq!(scoped_request.3, batch.id);
    assert_eq!(scoped_request.4, task.id);

    let work_units = WorkUnitRepo::list_by_task(&*database, &task.id)
        .await
        .expect("materialized Worker and reviewer WorkUnits load");
    assert_eq!(work_units.len(), 2);
    let observations = observations
        .lock()
        .expect("fake Harness observations remain readable")
        .clone();
    for role in ["orchestrator", "implementer", "reviewer"] {
        assert_eq!(
            observations
                .iter()
                .filter(|(observed_role, _, _)| observed_role == role)
                .count(),
            1,
            "the fake Harness handles one fresh {role} Execution"
        );
    }
    let distinct_native_sessions = observations
        .iter()
        .map(|(_, _, session)| session.as_str())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        distinct_native_sessions.len(),
        3,
        "coordinator, Worker, and reviewer receive separate native sessions"
    );

    let observation_sessions = observations
        .iter()
        .map(|(_, execution_id, session)| (execution_id.as_str(), session.as_str()))
        .collect::<std::collections::HashMap<_, _>>();
    let persisted_sessions = sqlx::query(
        "SELECT e.id,e.harness_session_id,hs.id AS persisted_session_id,hs.external_session_id
         FROM execution e
         JOIN harness_session hs ON hs.id=e.harness_session_id
         WHERE e.id IN (?,?,?)",
    )
    .bind(&coordinator_execution_id)
    .bind(&work_unit_execution_ids[0])
    .bind(&work_unit_execution_ids[1])
    .fetch_all(database.pool())
    .await
    .expect("persisted coordinator, Worker, and reviewer HarnessSessions load");
    assert_eq!(persisted_sessions.len(), 3);
    let mut persisted_native_sessions = std::collections::HashSet::new();
    for row in &persisted_sessions {
        let execution_id: String = row.try_get("id").expect("Execution ID reads");
        let harness_session_id: Option<String> = row
            .try_get("harness_session_id")
            .expect("Execution HarnessSession ID reads");
        let persisted_session_id: String = row
            .try_get("persisted_session_id")
            .expect("joined HarnessSession ID reads");
        let external_session_id: Option<String> = row
            .try_get("external_session_id")
            .expect("HarnessSession external ID reads");
        assert_eq!(
            harness_session_id.as_deref(),
            Some(persisted_session_id.as_str())
        );
        let external_session_id = external_session_id
            .expect("completed fake Harness turn persists an external session ID");
        assert_eq!(
            observation_sessions.get(execution_id.as_str()).copied(),
            Some(external_session_id.as_str()),
            "the persisted native session matches the ID returned by this Execution's fake Harness"
        );
        persisted_native_sessions.insert(external_session_id);
    }
    assert_eq!(
        persisted_native_sessions.len(),
        3,
        "coordinator, Worker, and reviewer HarnessSessions persist distinct native identities"
    );
}
