use super::*;

use db::{
    AgentRepo, AgentStatus, ArtifactKind, CreateAgent, CreateRepo, DaemonRepo, DaemonStatus,
    ProjectTaskRoleDefault, ProjectTaskRoleDefaultsRepo, RepoRepo, TaskCoordinationRepo, TaskRepo,
    UpdateDaemonReport, UpdateProject, UpsertDaemon, WorkMode, WorkUnitRepo,
};
use serde_json::json;
use std::{collections::VecDeque, path::Path, process::Command, sync::Arc, time::Duration};

type ObservedWorkUnitExecutionRow = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);
type ObservedWakeRow = (String, String, String, Option<String>, Option<i64>);
type ObservedExecutionDiagnosticRow = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
);

#[derive(Clone)]
struct ProjectBatchHarnessAdapter {
    plan: String,
    orchestrator_turns: Arc<std::sync::atomic::AtomicUsize>,
    implementer_turns: Arc<std::sync::atomic::AtomicUsize>,
    reviewer_turns: Arc<std::sync::atomic::AtomicUsize>,
    triage_dispositions: Arc<std::sync::Mutex<VecDeque<String>>>,
    first_triage_started: Arc<tokio::sync::Notify>,
    release_first_triage: Arc<tokio::sync::Notify>,
    block_first_triage: Arc<std::sync::atomic::AtomicBool>,
    observations: Arc<std::sync::Mutex<Vec<(String, String, String)>>>,
}

fn prompt_json_string(prompt: &str, key: &str) -> String {
    let marker = format!("\"{key}\":");
    let value = prompt
        .split_once(&marker)
        .unwrap_or_else(|| panic!("ReviewFindings prompt has {key:?}"))
        .1;
    let end = value
        .find(|character| matches!(character, ',' | '}'))
        .expect("ReviewFindings prompt JSON string has a delimiter");
    serde_json::from_str(&value[..end]).expect("ReviewFindings prompt field is a JSON string")
}

fn prompt_json_number(prompt: &str, key: &str) -> i64 {
    let marker = format!("\"{key}\":");
    let value = prompt
        .split_once(&marker)
        .unwrap_or_else(|| panic!("ReviewFindings prompt has {key:?}"))
        .1;
    let end = value
        .find(|character| matches!(character, ',' | '}'))
        .expect("ReviewFindings prompt number has a delimiter");
    value[..end]
        .parse()
        .expect("ReviewFindings prompt field is an integer")
}

fn review_findings_output(prompt: &str, disposition: &str) -> String {
    let exact_report = prompt
        .split_once("Exact report: ")
        .expect("ReviewFindings prompt includes its exact report")
        .1;
    let (report_id_json, rest) = exact_report
        .split_once(" (")
        .expect("ReviewFindings prompt report includes its digest");
    let (report_digest_json, _) = rest
        .split_once(')')
        .expect("ReviewFindings prompt digest has a closing delimiter");
    let report_id: String =
        serde_json::from_str(report_id_json).expect("exact report ID is JSON encoded");
    let report_digest: String =
        serde_json::from_str(report_digest_json).expect("exact report digest is JSON encoded");
    json!({
        "schema_version": 1,
        "request_id": prompt_json_string(prompt, "request_id"),
        "task_id": prompt_json_string(prompt, "task_id"),
        "authorization_version": prompt_json_number(prompt, "authorization_version"),
        "review_findings": [{
            "authorization_version": prompt_json_number(prompt, "authorization_version"),
            "review_report_artifact_id": report_id,
            "report_digest": report_digest,
            "disposition": disposition,
            "rationale": format!("The exact report supports the scripted {disposition} decision."),
            "instructions": if disposition == "recheck" {
                "Independently review the same frozen result and report whether the finding is reproducible."
            } else {
                "Correct the exact confirmed finding within the original accepted scope."
            }
        }]
    })
    .to_string()
}

fn review_report_output(verdict: &str) -> String {
    let finding = if verdict == "request_changes" {
        vec!["The delivered result is missing the required acceptance evidence."]
    } else {
        Vec::new()
    };
    format!(
        "FORGE_RESULT: {}",
        json!({
            "schema_version": 1,
            "kind": "review",
            "verdict": verdict,
            "summary": if verdict == "request_changes" {
                "The exact reviewed result is missing required acceptance evidence."
            } else {
                "The exact corrected result satisfies the acceptance criterion."
            },
            "criteria": ["The exact result satisfies the requested acceptance criterion."],
            "findings": finding,
            "questions": [],
            "evidence_considered": []
        })
    )
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
            "orchestrator" => {
                let turn = self
                    .orchestrator_turns
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if turn == 0 {
                    self.plan.clone()
                } else if context.description.contains("Exact report: ") {
                    let disposition = self
                        .triage_dispositions
                        .lock()
                        .expect("fake Harness triage script locks")
                        .pop_front()
                        .expect("each exact ReviewFindings turn has one scripted decision");
                    if self
                        .block_first_triage
                        .swap(false, std::sync::atomic::Ordering::SeqCst)
                    {
                        self.first_triage_started.notify_one();
                        self.release_first_triage.notified().await;
                    }
                    review_findings_output(&context.description, &disposition)
                } else {
                    r#"{"actions":[]}"#.to_owned()
                }
            }
            "implementer" => {
                let turn = self
                    .implementer_turns
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                format!("Implementation pass {turn} delivered for the exact WorkUnit scope.")
            }
            "reviewer" => {
                let turn = self
                    .reviewer_turns
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                review_report_output(if turn < 2 { "request_changes" } else { "pass" })
            }
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
    members: Vec<ActorRef>,
    coordinator_actor: Option<&str>,
    rework: Option<api_types::ProjectTaskRoleReworkConfiguration>,
) -> ProjectTaskRoleDefault {
    ProjectTaskRoleDefault {
        role: role.to_owned(),
        coordination_mode,
        configuration_json: serde_json::to_string(&api_types::ProjectTaskRoleConfiguration {
            schema_version: 1,
            display_label: None,
            capabilities,
            rework,
            coordinator_actor: coordinator_actor
                .map(|actor_id| api_types::ActorRef::Agent(actor_id.to_owned())),
        })
        .expect("Project TaskRole configuration serializes"),
        members,
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

async fn add_embedded_codex_agent(
    database: &Arc<db::SqliteDb>,
    owner_id: &str,
) -> (String, String) {
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
    let agent_id = add_codex_agent_to_daemon(
        database,
        owner_id,
        &daemon_id,
        "Project batch coordinator fake Harness",
    )
    .await;
    (daemon_id, agent_id)
}

async fn add_codex_agent_to_daemon(
    database: &Arc<db::SqliteDb>,
    owner_id: &str,
    daemon_id: &str,
    name: &str,
) -> String {
    let now = now_rfc3339();
    let agent_id = new_uuid_v4();
    AgentRepo::create(
        &**database,
        CreateAgent {
            id: agent_id.clone(),
            name: name.to_owned(),
            description: None,
            executor_type: "codex".to_owned(),
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            prompt_template: None,
            capabilities_json: "[]".to_owned(),
            config_json: "{}".to_owned(),
            credential_ref: None,
            daemon_id: Some(daemon_id.to_owned()),
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
    let owner_id = test_admin_user(&database, "Project batch owner and embedded host admin").await;
    let project_id = test_project(&database, &owner_id).await;
    let repo_root = tempfile::tempdir().expect("isolated repository root creates");
    add_project_repo(&database, &project_id, repo_root.path()).await;
    let (daemon_id, agent_id) = add_embedded_codex_agent(&database, &owner_id).await;
    let reviewer_agent_id = add_codex_agent_to_daemon(
        &database,
        &owner_id,
        &daemon_id,
        "Project batch independent reviewer fake Harness",
    )
    .await;

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
                vec![ActorRef::Agent(agent_id.clone())],
                Some(&agent_id),
                None,
            ),
            project_role_default(
                "implementer",
                db::CoordinationMode::Independent,
                Vec::new(),
                vec![ActorRef::Agent(agent_id.clone())],
                None,
                None,
            ),
            project_role_default(
                "reviewer",
                db::CoordinationMode::Independent,
                vec![
                    api_types::TaskRoleCapability::FormalReview,
                    api_types::TaskRoleCapability::RequestRework,
                ],
                vec![ActorRef::Agent(reviewer_agent_id.clone())],
                None,
                Some(api_types::ProjectTaskRoleReworkConfiguration {
                    mode: api_types::TaskRoleReworkMode::Automatic,
                    allowed_destination_roles: vec!["implementer".to_owned()],
                    next_review_role: Some("reviewer".to_owned()),
                    correction_actor: Some(api_types::ReworkActorSelection::OriginalContributor),
                    review_actor: Some(ActorRef::Agent(reviewer_agent_id.clone())),
                }),
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
                "Use a bounded implementation, an independent formal review, and exact review triage."
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
                "assigned_actor": {"kind": "agent", "id": reviewer_agent_id},
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
    let first_triage_started = Arc::new(tokio::sync::Notify::new());
    let release_first_triage = Arc::new(tokio::sync::Notify::new());
    let mut registry = executors::HarnessAdapterRegistry::new();
    registry.register(Box::new(ProjectBatchHarnessAdapter {
        plan,
        orchestrator_turns: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        implementer_turns: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        reviewer_turns: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        triage_dispositions: Arc::new(std::sync::Mutex::new(VecDeque::from([
            "recheck".to_owned(),
            "correct".to_owned(),
        ]))),
        first_triage_started: Arc::clone(&first_triage_started),
        release_first_triage: Arc::clone(&release_first_triage),
        block_first_triage: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        observations: Arc::clone(&observations),
    }));
    let registry = Arc::new(registry);
    let workspace_root = tempfile::tempdir().expect("isolated workspace root creates");
    let task_service = Arc::new(
        TaskService::new(Arc::clone(&database), Arc::clone(&event_bus))
            .with_workspace_root(workspace_root.path().to_path_buf())
            .with_workspace_exec_locks(Arc::new(crate::WorkspaceExecutionLockManager::new()))
            .with_repo_cache_locks(Arc::new(::workspace::RepoCacheLockManager::new()))
            .with_task_executor(Arc::new(executors::AdapterExecutor::new(Arc::clone(
                &registry,
            ))))
            // The fake controls the Harness result; the installed Codex
            // adapter supplies the real capability evidence required by
            // automatic orchestration admission.
            .with_adapter_registry(Arc::new(cli_adapters::default_registry())),
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
    let wake_diagnostic: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT w.state,w.last_error
         FROM orchestrator_wake w
         JOIN domain_event e ON e.id=w.event_id
         WHERE e.event_type='task.coordination_requested' AND e.entity_id=? AND w.task_id=?",
    )
    .bind(&request_id)
    .bind(&task.id)
    .fetch_optional(database.pool())
    .await
    .expect("exact batch coordinator wake diagnostic loads");
    assert_eq!(
        run.dispatched_wakes, 1,
        "the batch coordinator starts once; run={run:?}; durable wake state/error={wake_diagnostic:?}"
    );

    let mut materialized_plan_id = None;
    let mut coordinator_execution_id = None;
    let mut observed_request = request.clone();
    for _ in 0..200 {
        let current = TaskCoordinationRepo::get_task_coordination_request(&*database, &request_id)
            .await
            .expect("batch request status loads")
            .expect("batch request remains durable");
        observed_request = current.clone();
        if current.state == db::TaskCoordinationRequestState::Materialized {
            materialized_plan_id = current.plan_artifact_id;
            coordinator_execution_id = current.execution_id;
            break;
        }
        if let Some(execution_id) = current.execution_id.as_deref() {
            let execution = db::ExecutionRepo::get_by_id(&*database, execution_id)
                .await
                .expect("coordinator Execution status loads")
                .expect("running request has its exact coordinator Execution");
            if execution.status != db::ExecutionStatus::Running {
                // run_once dispatches the coordinator after its event scan.
                // Drive the durable terminal event in a subsequent pass as
                // the normal OrchestratorRuntime loop does.
                runtime
                    .run_once(100)
                    .await
                    .expect("OrchestratorRuntime reconciles the coordinator terminal event");
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let execution_diagnostic = match observed_request.execution_id.as_deref() {
        Some(execution_id) => db::ExecutionRepo::get_by_id(&*database, execution_id)
            .await
            .expect("coordinator Execution diagnostic loads")
            .map(|execution| {
                (
                    execution.id,
                    execution.status,
                    execution.error,
                    execution.summary,
                    execution.harness_session_id,
                )
            }),
        None => None,
    };
    let terminal_wake_diagnostic: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT w.state,w.last_error
         FROM orchestrator_wake w
         JOIN domain_event e ON e.id=w.event_id
         WHERE e.event_type='task.coordination_requested' AND e.entity_id=? AND w.task_id=?",
    )
    .bind(&request_id)
    .bind(&task.id)
    .fetch_optional(database.pool())
    .await
    .expect("terminal batch coordinator wake diagnostic loads");
    let plan_artifact_id = materialized_plan_id.unwrap_or_else(|| {
        panic!(
            "fake cognitive plan was not persisted and materialized by TaskCoordinationService; \
             request={observed_request:?}; coordinator_execution={execution_diagnostic:?}; \
             durable_wake={terminal_wake_diagnostic:?}"
        )
    });
    let coordinator_execution_id = coordinator_execution_id
        .expect("materialized WorkUnitPlan identifies its coordinator Execution");
    let plan_record =
        TaskCoordinationRepo::get_task_coordination_plan(&*database, &plan_artifact_id)
            .await
            .expect("materialized plan record loads")
            .expect("real WorkUnitPlan record persists");
    assert_eq!(plan_record.task_id, task.id);
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
    let pause_database = Arc::clone(&database);
    let pause_event_bus = Arc::clone(&event_bus);
    let pause_started = Arc::clone(&first_triage_started);
    let pause_release = Arc::clone(&release_first_triage);
    let pause_project_id = project_id.clone();
    let pause_batch_id = batch.id.clone();
    let pause_owner_id = owner_id.clone();
    let pause_task = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(45), pause_started.notified())
            .await
            .expect("the first exact ReviewFindings turn reaches the fake Harness");
        let plan_service =
            crate::project_plan::ProjectPlanService::new(pause_database.clone(), pause_event_bus);
        let version: i64 = sqlx::query_scalar("SELECT version FROM project_plan_batch WHERE id=?")
            .bind(&pause_batch_id)
            .fetch_one(pause_database.pool())
            .await
            .expect("current batch version loads at the provider boundary");
        let paused = plan_service
            .update_batch(
                &pause_project_id,
                &pause_owner_id,
                &pause_batch_id,
                version,
                "pause",
                "project-batch-review-pause",
            )
            .await
            .expect("owner pauses the batch after fake provider Start");
        pause_release.notify_one();
        paused
    });
    let mut execution_events = event_bus.subscribe();
    let workflow_deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let report_artifact_id: Option<String>;
    let mut observed_execution_rows: Vec<ObservedWorkUnitExecutionRow>;
    let mut observed_work_units: Vec<db::WorkUnit>;
    let mut observed_wakes: Vec<ObservedWakeRow>;
    let mut observed_wake_attempts: Vec<(String, i64, String, String, Option<String>, String)>;
    loop {
        let dispatch_outcome = dispatcher
            .check_once()
            .await
            .expect("TaskDispatcher admits the next ready materialized WorkUnit");
        let runtime_outcome = runtime
            .run_once(100)
            .await
            .expect("OrchestratorRuntime reconciles terminal events under batch authority");
        observed_execution_rows = sqlx::query_as(
            "SELECT e.id,e.role,e.status,e.error,e.summary,e.work_unit_id,e.work_unit_version,
                    e.workspace_id,e.harness_session_id,hs.external_session_id,hs.status,
                    report.id AS report_artifact_id
             FROM execution e
             JOIN work_unit w ON w.id=e.work_unit_id
             LEFT JOIN harness_session hs ON hs.id=e.harness_session_id
             LEFT JOIN artifact_execution_producer producer
               ON producer.execution_id=e.id AND producer.task_id=e.task_id
             LEFT JOIN artifact report
               ON report.id=producer.artifact_id AND report.task_id=e.task_id
              AND report.kind='review_report'
             WHERE e.task_id=? AND w.provenance_kind='artifact' AND w.provenance_id=?
             ORDER BY e.created_at,e.id",
        )
        .bind(&task.id)
        .bind(&plan_artifact_id)
        .fetch_all(database.pool())
        .await
        .expect("materialized WorkUnit Executions and report links load");
        let work_units = WorkUnitRepo::list_by_task(&*database, &task.id)
            .await
            .expect("materialized WorkUnits load")
            .into_iter()
            .filter(|unit| {
                unit.provenance == Some(db::WorkUnitProvenance::Artifact(plan_artifact_id.clone()))
            })
            .collect::<Vec<_>>();
        observed_work_units = work_units;
        observed_wakes = sqlx::query_as(
            "SELECT w.id,w.state,e.event_type,w.last_error,w.current_attempt
             FROM orchestrator_wake w JOIN domain_event e ON e.id=w.event_id
             WHERE w.task_id=? ORDER BY w.event_sequence,w.id",
        )
        .bind(&task.id)
        .fetch_all(database.pool())
        .await
        .expect("batch orchestration wake states load");
        observed_wake_attempts = sqlx::query_as(
            "SELECT attempt.wake_id,attempt.attempt_number,attempt.execution_id,
                    attempt.state,attempt.last_error,execution.status
             FROM orchestrator_wake_execution attempt
             JOIN orchestrator_wake wake ON wake.id=attempt.wake_id
             JOIN execution ON execution.id=attempt.execution_id
             WHERE wake.task_id=? ORDER BY attempt.wake_id,attempt.attempt_number",
        )
        .bind(&task.id)
        .fetch_all(database.pool())
        .await
        .expect("durable coordinator wake attempt receipts load");
        let work_unit_executions_complete = observed_execution_rows.len() == 2
            && observed_execution_rows
                .iter()
                .all(|row| row.2 == "completed")
            && observed_execution_rows
                .iter()
                .any(|row| row.1 == "implementer")
            && observed_execution_rows
                .iter()
                .any(|row| row.1 == "reviewer");
        let all_work_units_complete = observed_work_units.len() == 2
            && observed_work_units
                .iter()
                .all(|unit| unit.status == db::WorkUnitStatus::Completed);
        let has_review_report = observed_execution_rows
            .iter()
            .find(|row| row.1 == "reviewer")
            .and_then(|row| row.11.clone());
        let terminally_blocked_wakes = observed_wakes
            .iter()
            .filter(|wake| matches!(wake.1.as_str(), "failed" | "awaiting_human" | "uncertain"))
            .count();
        if work_unit_executions_complete && all_work_units_complete && has_review_report.is_some() {
            report_artifact_id = has_review_report;
            break;
        }

        let failed_work_execution = observed_execution_rows
            .iter()
            .any(|row| matches!(row.2.as_str(), "failed" | "cancelled"));
        if failed_work_execution
            || terminally_blocked_wakes > 0
            || tokio::time::Instant::now() >= workflow_deadline
        {
            report_artifact_id = None;
            break;
        }

        let running_executions: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM execution WHERE task_id=? AND status='running'",
        )
        .bind(&task.id)
        .fetch_one(database.pool())
        .await
        .expect("active batch execution count loads");
        let made_progress = dispatch_outcome > 0
            || runtime_outcome.processed_events > 0
            || runtime_outcome.admitted_wakes > 0
            || runtime_outcome.dispatched_wakes > 0
            || runtime_outcome.retried_wakes > 0;

        // DomainEventCommitted is a delivery hint only. The following pass
        // reloads authoritative WorkUnit, Execution, Artifact, and Wake rows.
        let wait_deadline =
            (tokio::time::Instant::now() + Duration::from_secs(5)).min(workflow_deadline);
        let next_event = tokio::time::timeout_at(wait_deadline, async {
            loop {
                match execution_events.recv().await {
                    Ok(event)
                        if matches!(
                            &event.context,
                            events::EventContext::DomainEventCommitted {
                                entity_type,
                                scope_type,
                                scope_id,
                                ..
                            } if scope_type == "task"
                                && scope_id == &task.id
                                && matches!(entity_type.as_str(), "execution" | "work_unit")
                        ) =>
                    {
                        break true;
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => break true,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break false,
                }
            }
        })
        .await;
        if matches!(next_event, Ok(false))
            || (next_event.is_err()
                && running_executions == 0
                && observed_wakes.iter().all(|wake| wake.1 == "completed")
                && !made_progress)
        {
            report_artifact_id = None;
            break;
        }
    }
    if report_artifact_id.is_none() {
        let unit_diagnostics = {
            let mut diagnostics = Vec::new();
            for unit in &observed_work_units {
                let dependencies = WorkUnitRepo::list_dependencies(&*database, &unit.id)
                    .await
                    .expect("WorkUnit dependency diagnostics load");
                let active_execution_ids =
                    WorkUnitRepo::list_active_execution_ids(&*database, &unit.id)
                        .await
                        .expect("WorkUnit active Execution diagnostics load");
                let workspace =
                    db::WorkUnitWorkspaceRepo::get_by_work_unit_id(&*database, &unit.id)
                        .await
                        .expect("WorkUnit Workspace diagnostics load");
                let lease = db::WorkspaceLeaseRepo::get_active_for_work_unit(&*database, &unit.id)
                    .await
                    .expect("WorkUnit WorkspaceLease diagnostics load");
                diagnostics.push((
                    unit.clone(),
                    dependencies,
                    active_execution_ids,
                    workspace,
                    lease,
                ));
            }
            diagnostics
        };
        let produced_artifacts: Vec<(String, String, Option<String>, String)> = sqlx::query_as(
            "SELECT a.id,a.kind,a.digest,p.execution_id
             FROM artifact a JOIN artifact_execution_producer p
               ON p.artifact_id=a.id AND p.task_id=a.task_id
             JOIN execution e ON e.id=p.execution_id AND e.task_id=p.task_id
             WHERE e.task_id=? AND (e.role='orchestrator' OR e.work_unit_id IN (
                 SELECT id FROM work_unit WHERE task_id=?
                   AND provenance_kind='artifact' AND provenance_id=?
             )) ORDER BY e.created_at,e.id,a.kind,a.id",
        )
        .bind(&task.id)
        .bind(&task.id)
        .bind(&plan_artifact_id)
        .fetch_all(database.pool())
        .await
        .expect("batch plan, delivery, and review artifacts diagnose");
        let all_execution_diagnostics: Vec<ObservedExecutionDiagnosticRow> = sqlx::query_as(
            "SELECT e.id,e.role,e.status,e.error,e.summary,e.work_unit_id,
                    e.work_unit_version,e.workspace_id,hs.external_session_id,hs.status
             FROM execution e LEFT JOIN harness_session hs ON hs.id=e.harness_session_id
             WHERE e.task_id=? ORDER BY e.created_at,e.id",
        )
        .bind(&task.id)
        .fetch_all(database.pool())
        .await
        .expect("all coordinator, Worker, and reviewer Execution diagnostics load");
        panic!(
            "batch Worker/reviewer lifecycle did not reach exact completion; \
             work_units={unit_diagnostics:?}; executions={observed_execution_rows:?}; \
             all_executions={all_execution_diagnostics:?}; \
             produced_artifacts={produced_artifacts:?}; orchestration_wakes={observed_wakes:?}; \
             orchestration_attempts={observed_wake_attempts:?}; \
             deadline={workflow_deadline:?}"
        );
    }
    let initial_report_artifact_id =
        report_artifact_id.expect("first formal review produces its exact report Artifact");
    let paused_batch = tokio::time::timeout(Duration::from_secs(10), pause_task)
        .await
        .expect("batch pause completes at the first ReviewFindings provider boundary")
        .expect("batch pause task joins");
    assert_eq!(paused_batch.state, "paused");
    let first_review_request_key = format!(
        "review-findings:{}:{initial_report_artifact_id}",
        coordination.authorization_version
    );
    let first_review_request = TaskCoordinationRepo::get_task_coordination_request_by_key(
        &*database,
        &task.id,
        &first_review_request_key,
    )
    .await
    .expect("first typed ReviewFindings request lookup succeeds")
    .expect("first request is durably keyed to the exact report and authorization");
    assert_eq!(
        first_review_request.kind,
        db::TaskCoordinationRequestKind::ReviewFindings
    );
    assert_eq!(
        first_review_request.state,
        db::TaskCoordinationRequestState::Running,
        "pause preserves the provider-started exact request"
    );
    let first_triage_execution_id = first_review_request
        .execution_id
        .clone()
        .expect("started typed request records its coordinator Execution");
    let provider_result = db::ExecutionRepo::get_by_id(&*database, &first_triage_execution_id)
        .await
        .expect("first ReviewFindings Execution reloads after the fake provider returns")
        .expect("first ReviewFindings Execution remains durable");
    assert_eq!(provider_result.status, db::ExecutionStatus::Completed);
    let triage_count_while_paused: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task_review_triage WHERE task_id=?")
            .bind(&task.id)
            .fetch_one(database.pool())
            .await
            .expect("triage receipt count loads while the batch is paused");
    assert_eq!(triage_count_while_paused, 0);
    // Simulate a long pause after the provider already completed. Recovery
    // must retain this exact response even when the original wake lease ends.
    sqlx::query("UPDATE orchestrator_wake SET lease_until='2000-01-01T00:00:00Z' WHERE id=?")
        .bind(
            first_review_request
                .wake_id
                .as_deref()
                .expect("exact typed wake"),
        )
        .execute(database.pool())
        .await
        .expect("expire only the disposable completed-response wake lease");
    runtime
        .run_once(100)
        .await
        .expect("runtime defers the completed exact response while the batch is paused");
    let still_pending =
        TaskCoordinationRepo::get_task_coordination_request(&*database, &first_review_request.id)
            .await
            .expect("paused typed request reloads")
            .expect("paused typed request remains durable");
    assert_eq!(
        still_pending.state,
        db::TaskCoordinationRequestState::Running
    );
    let no_paused_triage: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task_review_triage WHERE task_id=?")
            .bind(&task.id)
            .fetch_one(database.pool())
            .await
            .expect("paused triage receipt count reloads");
    assert_eq!(no_paused_triage, 0);
    let coordinator_starts_while_paused: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution WHERE task_id=? AND role='orchestrator'",
    )
    .bind(&task.id)
    .fetch_one(database.pool())
    .await
    .expect("coordinator Execution count loads while paused");
    assert_eq!(
        coordinator_starts_while_paused, 2,
        "a completed provider response is retained without another Start while paused"
    );
    let plan_service =
        crate::project_plan::ProjectPlanService::new(Arc::clone(&database), Arc::clone(&event_bus));
    let resumed_batch = plan_service
        .update_batch(
            &project_id,
            &owner_id,
            &batch.id,
            paused_batch.version,
            "resume",
            "project-batch-review-resume",
        )
        .await
        .expect("owner resumes the same prepared batch");
    assert_eq!(resumed_batch.state, "running");
    let mut resumed_request = still_pending;
    for _ in 0..200 {
        runtime
            .run_once(100)
            .await
            .expect("runtime recovers the exact completed ReviewFindings Execution after resume");
        resumed_request = TaskCoordinationRepo::get_task_coordination_request(
            &*database,
            &first_review_request.id,
        )
        .await
        .expect("resumed ReviewFindings request reloads")
        .expect("resumed ReviewFindings request remains durable");
        if resumed_request.state == db::TaskCoordinationRequestState::Triaged {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        resumed_request.state,
        db::TaskCoordinationRequestState::Triaged,
        "resume applies the persisted recheck decision"
    );
    assert_eq!(
        resumed_request.execution_id.as_deref(),
        Some(first_triage_execution_id.as_str())
    );
    let triage_starts_after_resume: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution WHERE task_id=? AND role='orchestrator'",
    )
    .bind(&task.id)
    .fetch_one(database.pool())
    .await
    .expect("coordinator Execution count reloads after resume");
    assert_eq!(triage_starts_after_resume, 2);
    let first_triage_row: (String, String, Option<String>) = sqlx::query_as(
        "SELECT disposition,review_report_artifact_id,review_work_unit_id
         FROM task_review_triage WHERE task_id=? AND review_report_artifact_id=?",
    )
    .bind(&task.id)
    .bind(&initial_report_artifact_id)
    .fetch_one(database.pool())
    .await
    .expect("first exact triage receipt loads");
    assert_eq!(first_triage_row.0, "recheck");
    assert_eq!(first_triage_row.1, initial_report_artifact_id);
    let recheck_work_unit_id = first_triage_row
        .2
        .expect("recheck disposition creates a formal review WorkUnit");
    let original_subject: (String, i64, String, Option<String>, Option<String>, Option<String>, Option<String>) =
        sqlx::query_as(
            "SELECT reviewed_work_unit_id,reviewed_work_unit_version,result_execution_id,
                    result_artifact_id,result_commit_sha,result_workspace_id,workspace_snapshot_digest
             FROM work_unit_review_subject subject
             WHERE subject.review_work_unit_id=(
                 SELECT review_execution.work_unit_id
                 FROM artifact report
                 JOIN artifact_execution_producer producer ON producer.artifact_id=report.id
                 JOIN execution review_execution ON review_execution.id=producer.execution_id
                 WHERE report.id=? AND report.kind='review_report')",
        )
        .bind(&initial_report_artifact_id)
        .fetch_one(database.pool())
        .await
        .expect("initial ReviewReport frozen subject loads");
    let recheck_subject: (String, i64, String, Option<String>, Option<String>, Option<String>, Option<String>) =
        sqlx::query_as(
            "SELECT reviewed_work_unit_id,reviewed_work_unit_version,result_execution_id,
                    result_artifact_id,result_commit_sha,result_workspace_id,workspace_snapshot_digest
             FROM work_unit_review_subject WHERE review_work_unit_id=?",
        )
        .bind(&recheck_work_unit_id)
        .fetch_one(database.pool())
        .await
        .expect("recheck formal review freezes its subject");
    assert_eq!(recheck_subject, original_subject);

    let full_workflow_deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    loop {
        dispatcher
            .check_once()
            .await
            .expect("dispatcher admits the reassessment and correction WorkUnits");
        runtime
            .run_once(100)
            .await
            .expect("runtime advances the exact recheck and correction requests");
        let units = WorkUnitRepo::list_by_task(&*database, &task.id)
            .await
            .expect("all original and rework WorkUnits load");
        let review_reports: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM artifact WHERE task_id=? AND kind='review_report'",
        )
        .bind(&task.id)
        .fetch_one(database.pool())
        .await
        .expect("fresh formal reports count loads");
        let review_requests: Vec<(String, String)> = sqlx::query_as(
            "SELECT id,state FROM task_coordination_request
             WHERE task_id=? AND kind='review_findings' ORDER BY created_at,id",
        )
        .bind(&task.id)
        .fetch_all(database.pool())
        .await
        .expect("exact typed ReviewFindings request states load");
        let failed_work: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM execution WHERE task_id=? AND status IN ('failed','cancelled')",
        )
        .bind(&task.id)
        .fetch_one(database.pool())
        .await
        .expect("failed WorkUnit Execution count loads");
        let blocked_wakes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM orchestrator_wake
             WHERE task_id=? AND state IN ('failed','awaiting_human','uncertain')",
        )
        .bind(&task.id)
        .fetch_one(database.pool())
        .await
        .expect("blocked wake count loads");
        let completed = units.len() == 5
            && units
                .iter()
                .all(|unit| unit.status == db::WorkUnitStatus::Completed)
            && review_reports == 3
            && review_requests.len() == 2
            && review_requests.iter().all(|(_, state)| state == "triaged");
        if completed {
            break;
        }
        if failed_work > 0
            || blocked_wakes > 0
            || tokio::time::Instant::now() >= full_workflow_deadline
        {
            panic!(
                "review/rework workflow did not converge: work_units={units:?}; \
                 review_reports={review_reports}; review_requests={review_requests:?}; \
                 failed_work={failed_work}; blocked_wakes={blocked_wakes}"
            );
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let report_artifact_id = initial_report_artifact_id;
    let wake_origins: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT event.event_type,event.entity_type,event.entity_id
         FROM orchestrator_wake wake JOIN domain_event event ON event.id=wake.event_id
         WHERE wake.task_id=? ORDER BY event.sequence,event.id",
    )
    .bind(&task.id)
    .fetch_all(database.pool())
    .await
    .expect("exact and generic Agent wake origins load");
    assert_eq!(wake_origins.len(), 3);
    assert!(wake_origins.iter().all(|origin| {
        origin.0 == "task.coordination_requested" && origin.1 == "task_coordination_request"
    }));
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
        .expect("original, recheck, correction, and fresh-review WorkUnits load");
    assert_eq!(work_units.len(), 5);
    assert!(work_units
        .iter()
        .all(|unit| unit.status == db::WorkUnitStatus::Completed));

    let typed_review_requests: Vec<(String, String, String, String, String, i64, Option<String>)> =
        sqlx::query_as(
            "SELECT request.id,request.state,request.review_report_artifact_id,
                    request.review_report_digest,request.review_producer_execution_id,
                    request.authorization_version,request.execution_id
             FROM task_coordination_request request
             WHERE request.task_id=? AND request.kind='review_findings'
             ORDER BY request.created_at,request.id",
        )
        .bind(&task.id)
        .fetch_all(database.pool())
        .await
        .expect("both exact ReviewFindings requests load");
    assert_eq!(typed_review_requests.len(), 2);
    assert!(typed_review_requests
        .iter()
        .all(|request| request.1 == "triaged"));
    let first_request_index = typed_review_requests
        .iter()
        .position(|request| request.2 == report_artifact_id)
        .expect("first typed request is bound to the original ReviewReport");
    let second_request_index = (0..typed_review_requests.len())
        .find(|index| *index != first_request_index)
        .expect("a second exact typed request exists");
    let first_typed_request = &typed_review_requests[first_request_index];
    let second_typed_request = &typed_review_requests[second_request_index];
    assert_ne!(second_typed_request.2, first_typed_request.2);

    let triage_rows: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT review_report_artifact_id,disposition,execution_id,review_work_unit_id
         FROM task_review_triage WHERE task_id=? ORDER BY created_at,review_report_artifact_id",
    )
    .bind(&task.id)
    .fetch_all(database.pool())
    .await
    .expect("immutable exact review triage receipts load");
    assert_eq!(triage_rows.len(), 2);
    let recheck_triage = triage_rows
        .iter()
        .find(|row| row.0 == first_typed_request.2)
        .expect("original report has its exact recheck triage receipt");
    assert_eq!(recheck_triage.1, "recheck");
    assert_eq!(
        recheck_triage.3.as_deref(),
        Some(recheck_work_unit_id.as_str())
    );
    let correction_triage = triage_rows
        .iter()
        .find(|row| row.0 == second_typed_request.2)
        .expect("fresh recheck report has its exact correction triage receipt");
    assert_eq!(correction_triage.1, "correct");
    assert!(correction_triage.3.is_none());

    let report_rows: Vec<(String, Option<String>, String)> = sqlx::query_as(
        "SELECT report.id,report.digest,report.content
         FROM artifact report
         WHERE report.task_id=? AND report.kind='review_report'
         ORDER BY report.created_at,report.id",
    )
    .bind(&task.id)
    .fetch_all(database.pool())
    .await
    .expect("all immutable formal ReviewReports load");
    assert_eq!(report_rows.len(), 3);
    let recheck_report_id: String = sqlx::query_scalar(
        "SELECT report.id FROM artifact report
         JOIN artifact_execution_producer producer ON producer.artifact_id=report.id
         JOIN execution reviewer_execution ON reviewer_execution.id=producer.execution_id
         WHERE report.task_id=? AND report.kind='review_report'
           AND reviewer_execution.work_unit_id=?",
    )
    .bind(&task.id)
    .bind(&recheck_work_unit_id)
    .fetch_one(database.pool())
    .await
    .expect("fresh recheck report is linked to its formal review WorkUnit");
    assert_eq!(second_typed_request.2, recheck_report_id);
    let first_report_row = report_rows
        .iter()
        .find(|report| report.0 == report_artifact_id)
        .expect("original ReviewReport remains in the report set");
    let recheck_report_row = report_rows
        .iter()
        .find(|report| report.0 == recheck_report_id)
        .expect("fresh recheck ReviewReport remains in the report set");
    let first_report: Value =
        serde_json::from_str(&first_report_row.2).expect("first ReviewReport content is JSON");
    let second_report: Value =
        serde_json::from_str(&recheck_report_row.2).expect("recheck ReviewReport content is JSON");
    assert_eq!(first_report["verdict"], "request_changes");
    assert_eq!(second_report["verdict"], "request_changes");
    assert_eq!(first_report["subject"], second_report["subject"]);

    let rework_receipt: (String, String, String, String) = sqlx::query_as(
        "SELECT review_report_artifact_id,reviewed_work_unit_id,
                correction_work_unit_id,review_work_unit_id
         FROM task_rework_receipt WHERE task_id=?",
    )
    .bind(&task.id)
    .fetch_one(database.pool())
    .await
    .expect("correct decision creates one exact correction/review receipt");
    assert_eq!(rework_receipt.0, second_typed_request.2);
    assert_eq!(
        rework_receipt.1,
        first_report["subject"]["reviewed_work_unit_id"]
            .as_str()
            .expect("initial report subject has a WorkUnit")
    );
    let final_report_id: String = sqlx::query_scalar(
        "SELECT report.id FROM artifact report
         JOIN artifact_execution_producer producer ON producer.artifact_id=report.id
         JOIN execution reviewer_execution ON reviewer_execution.id=producer.execution_id
         WHERE report.task_id=? AND report.kind='review_report'
           AND reviewer_execution.work_unit_id=?",
    )
    .bind(&task.id)
    .bind(&rework_receipt.3)
    .fetch_one(database.pool())
    .await
    .expect("fresh correction review produces its own exact ReviewReport");
    let final_report_row = report_rows
        .iter()
        .find(|report| report.0 == final_report_id)
        .expect("fresh correction ReviewReport remains in the report set");
    let final_report: Value =
        serde_json::from_str(&final_report_row.2).expect("fresh ReviewReport content is JSON");
    assert_eq!(final_report["verdict"], "pass");
    assert_eq!(
        final_report["subject"]["reviewed_work_unit_id"],
        rework_receipt.2
    );
    let correction_execution_id: String =
        sqlx::query_scalar("SELECT id FROM execution WHERE work_unit_id=? AND status='completed'")
            .bind(&rework_receipt.2)
            .fetch_one(database.pool())
            .await
            .expect("correction WorkUnit has its completed exact result");
    assert_eq!(
        final_report["subject"]["reviewed_result"]["execution_id"],
        correction_execution_id
    );
    let current_review_line: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM work_unit_review_line
         WHERE task_id=? AND latest_report_artifact_id=? AND latest_verdict='pass'",
    )
    .bind(&task.id)
    .bind(&final_report_id)
    .fetch_one(database.pool())
    .await
    .expect("latest formal-review line lookup succeeds");
    assert_eq!(current_review_line, 1);

    let batch_sources: Vec<(String, String, String, i64, String, String)> = sqlx::query_as(
        "SELECT request.id,scope.origin_kind,scope.source_artifact_id,
                scope.authorization_version,scope.producer_execution_id,scope.batch_id
         FROM task_coordination_request request
         JOIN project_plan_batch_coordination_request scope ON scope.request_id=request.id
         WHERE request.task_id=? AND request.kind='review_findings'
         ORDER BY request.created_at,request.id",
    )
    .bind(&task.id)
    .fetch_all(database.pool())
    .await
    .expect("ReviewFindings requests carry immutable batch source bindings");
    assert_eq!(batch_sources.len(), 2);
    for source in &batch_sources {
        let request = typed_review_requests
            .iter()
            .find(|request| request.0 == source.0)
            .expect("batch source belongs to one exact typed request");
        assert_eq!(source.1, "review_triage");
        assert_eq!(source.2, request.2);
        assert_eq!(source.3, coordination.authorization_version);
        assert_eq!(source.4, request.4);
        assert_eq!(source.5, batch.id);
        assert_eq!(request.5, coordination.authorization_version);
        assert_eq!(
            request.3,
            report_rows
                .iter()
                .find(|report| report.0 == source.2)
                .and_then(|report| report.1.clone())
                .expect("typed request digest matches its exact source report")
        );
        assert_eq!(
            request.1, "triaged",
            "only the exact request source reaches its terminal triage state"
        );
        assert_eq!(
            db::TaskCoordinationRepo::get_task_coordination_request_by_key(
                &*database,
                &task.id,
                &format!(
                    "review-findings:{}:{}",
                    coordination.authorization_version, source.2
                ),
            )
            .await
            .expect("auth-version/report idempotency key lookup succeeds")
            .as_ref()
            .map(|request| request.id.as_str()),
            Some(request.0.as_str())
        );
    }

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

    let turn_count_before_replay: i64 =
        sqlx::query_scalar("SELECT turn_count FROM task_coordination WHERE task_id=?")
            .bind(&task.id)
            .fetch_one(database.pool())
            .await
            .expect("turn budget counter loads before replay");
    let coordination_service =
        crate::TaskCoordinationService::new(Arc::clone(&database), Arc::clone(&event_bus));
    for request in &typed_review_requests {
        let (replayed, created) = coordination_service
            .request_review_findings_for_report(&task.id, &request.2)
            .await
            .expect("exact triage request replay is safe");
        let replayed = replayed.expect("exact source preserves its durable request");
        assert_eq!(replayed.id, request.0);
        assert!(!created, "replay does not create another typed request");
    }
    let turn_count_after_replay: i64 =
        sqlx::query_scalar("SELECT turn_count FROM task_coordination WHERE task_id=?")
            .bind(&task.id)
            .fetch_one(database.pool())
            .await
            .expect("turn budget counter loads after replay");
    assert_eq!(turn_count_after_replay, turn_count_before_replay);
    assert_eq!(turn_count_after_replay, 3);

    let typed_request_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_coordination_request WHERE task_id=? AND kind='review_findings'",
    )
    .bind(&task.id)
    .fetch_one(database.pool())
    .await
    .expect("typed request count reloads after replay");
    assert_eq!(typed_request_count, 2);
    let typed_execution_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution WHERE task_id=? AND role='orchestrator'",
    )
    .bind(&task.id)
    .fetch_one(database.pool())
    .await
    .expect("orchestrator Start count reloads after replay");
    assert_eq!(typed_execution_count, 3);
    let duplicate_provider_starts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event
         WHERE scope_id=? AND event_type='execution.provider_start_attempted'
           AND entity_id IN (SELECT execution_id FROM task_coordination_request
                             WHERE task_id=? AND kind='review_findings')",
    )
    .bind(&task.id)
    .bind(&task.id)
    .fetch_one(database.pool())
    .await
    .expect("typed coordinator Provider Start receipt count loads");
    assert_eq!(duplicate_provider_starts, 2);

    let task_work_unit_execution_rows: Vec<(String, String, String, String, i64)> = sqlx::query_as(
        "SELECT e.id,e.role,e.status,link.batch_id,link.authorization_version
             FROM execution e
             JOIN project_plan_batch_execution link ON link.execution_id=e.id
             WHERE e.task_id=? AND e.work_unit_id IS NOT NULL
             ORDER BY e.created_at,e.id",
    )
    .bind(&task.id)
    .fetch_all(database.pool())
    .await
    .expect("all five source-bound batch WorkUnit Executions load");
    assert_eq!(task_work_unit_execution_rows.len(), 5);
    for row in &task_work_unit_execution_rows {
        assert_eq!(row.2, "completed");
        assert_eq!(row.3, batch.id);
        assert_eq!(row.4, coordination.authorization_version);
        assert!(matches!(row.1.as_str(), "implementer" | "reviewer"));
    }

    let all_executions: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT id,role,status,summary FROM execution WHERE task_id=? ORDER BY created_at,id",
    )
    .bind(&task.id)
    .fetch_all(database.pool())
    .await
    .expect("all coordinator, Worker, recheck, and correction Executions load");
    assert_eq!(all_executions.len(), 8);
    assert!(all_executions
        .iter()
        .all(|execution| execution.2 == "completed"));
    let coordinator_execution_rows = all_executions
        .iter()
        .filter(|execution| execution.1 == "orchestrator")
        .collect::<Vec<_>>();
    assert_eq!(coordinator_execution_rows.len(), 3);
    assert!(coordinator_execution_rows
        .iter()
        .any(|execution| execution.0 == coordinator_execution_id));
    for request in &typed_review_requests {
        let execution_id = request
            .6
            .as_deref()
            .expect("typed request owns one coordinator Execution");
        assert!(coordinator_execution_rows
            .iter()
            .any(|execution| execution.0 == execution_id));
        let output: Value = serde_json::from_str(
            coordinator_execution_rows
                .iter()
                .find(|execution| execution.0 == execution_id)
                .and_then(|execution| execution.3.as_deref())
                .expect("typed decision Execution has a durable structured result"),
        )
        .expect("typed decision result is JSON");
        assert_eq!(output["review_findings"].as_array().map(Vec::len), Some(1));
    }

    let observations = observations
        .lock()
        .expect("fake Harness observations remain readable")
        .clone();
    assert_eq!(observations.len(), all_executions.len());
    for (role, count) in [("orchestrator", 3), ("implementer", 2), ("reviewer", 3)] {
        assert_eq!(
            observations
                .iter()
                .filter(|(observed_role, _, _)| observed_role == role)
                .count(),
            count,
            "each {role} turn is admitted once"
        );
    }
    assert!(observations.iter().any(|(observed_role, execution_id, _)| {
        observed_role == "orchestrator" && execution_id == &coordinator_execution_id
    }));
    let distinct_native_sessions = observations
        .iter()
        .map(|(_, _, session)| session.as_str())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(distinct_native_sessions.len(), observations.len());

    let observation_sessions = observations
        .iter()
        .map(|(_, execution_id, session)| (execution_id.as_str(), session.as_str()))
        .collect::<std::collections::HashMap<_, _>>();
    let persisted_sessions = sqlx::query(
        "SELECT e.id,e.harness_session_id,hs.id AS persisted_session_id,
                hs.external_session_id,hs.status,e.status AS execution_status
         FROM execution e
         JOIN harness_session hs ON hs.id=e.harness_session_id
         WHERE e.task_id=? ORDER BY e.created_at,e.id",
    )
    .bind(&task.id)
    .fetch_all(database.pool())
    .await
    .expect("all coordinator, Worker, and review HarnessSessions load");
    assert_eq!(persisted_sessions.len(), observations.len());
    let mut persisted_native_sessions = std::collections::HashSet::new();
    for row in &persisted_sessions {
        let execution_id: String = row.try_get("id").expect("Execution ID reads");
        let execution_status: String = row
            .try_get("execution_status")
            .expect("Execution terminal status reads");
        let harness_session_id: Option<String> = row
            .try_get("harness_session_id")
            .expect("Execution HarnessSession ID reads");
        let persisted_session_id: String = row
            .try_get("persisted_session_id")
            .expect("joined HarnessSession ID reads");
        let external_session_id: String = row
            .try_get::<Option<String>, _>("external_session_id")
            .expect("HarnessSession external ID reads")
            .expect("completed fake Harness turn persists its native session");
        let session_status: String = row.try_get("status").expect("session status reads");
        assert_eq!(execution_status, "completed");
        assert_eq!(session_status, "active");
        assert_eq!(
            harness_session_id.as_deref(),
            Some(persisted_session_id.as_str())
        );
        assert_eq!(
            observation_sessions.get(execution_id.as_str()).copied(),
            Some(external_session_id.as_str())
        );
        assert!(persisted_native_sessions.insert(external_session_id));
    }
    assert_eq!(persisted_native_sessions.len(), observations.len());
}
