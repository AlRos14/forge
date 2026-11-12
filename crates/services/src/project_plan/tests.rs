use super::*;
use db::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, ActorKind, ActorRef, AgentRepo,
    AgentStatus, CreateAgent, CreateDomainEvent, CreateOrchestratorWake, CreateProject,
    CreateProjectMember, CreateRepo, CreateRoleMembership, CreateTask, CreateTaskRole,
    CreateWorkUnit, CreateWorkUnitIntegration, CreateWorkUnitWorkspace, CreateWorkspace,
    ExecutionRepo, ExecutionStatus, OrchestratorWakeRepo, ProjectMemberRepo,
    ProjectPlanBatchRequestBinding, ProjectPlanBatchRequestOrigin, ProjectRepo,
    RecordWorkUnitIntegration, RepoRepo, RoleMembershipRepo, RoleMembershipStatus,
    TaskCoordinationMode, TaskCoordinationRepo, TaskDependencyRepo, TaskLifecycleRepo,
    TaskLifecycleState, TaskRepo, TaskRoleRepo, TransitionWorkUnit, User, UserRepo, WorkMode,
    WorkUnitIntegrationOutcome, WorkUnitRepo, WorkUnitStatus, WorkUnitWorkspaceRepo, WorkspaceRepo,
    WorkspaceStatus,
};
use events::EventBus;
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};
use tempfile::TempDir;

struct Fixture {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
    service: ProjectPlanService,
    owner_id: String,
    project_id: String,
}

async fn fixture() -> Fixture {
    let pool = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("pool creates");
    run_migrations(&pool).await.expect("migrations run");
    let db = Arc::new(SqliteDb::new(pool));
    let now = now_rfc3339();
    let owner_id = new_uuid_v4();
    UserRepo::create_user(
        db.as_ref(),
        &User {
            id: owner_id.clone(),
            email: format!("{owner_id}@example.test"),
            password_hash: "test-hash".to_owned(),
            display_name: Some("Plan owner".to_owned()),
            is_admin: false,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("owner user creates");
    let project_id = new_uuid_v4();
    ProjectRepo::create(
        db.as_ref(),
        CreateProject {
            id: project_id.clone(),
            name: "Project plan fixture".to_owned(),
            settings: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: Some(owner_id.clone()),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("Project and canonical owner membership create");
    let event_bus = Arc::new(EventBus::new(16));
    let service = ProjectPlanService::new(Arc::clone(&db), Arc::clone(&event_bus));
    Fixture {
        db,
        event_bus,
        service,
        owner_id,
        project_id,
    }
}

async fn seed_task(db: &SqliteDb, project_id: &str, title: &str) -> String {
    let now = now_rfc3339();
    let id = new_uuid_v4();
    TaskRepo::create(
        db,
        CreateTask {
            id: id.clone(),
            project_id: project_id.to_owned(),
            repo_id: None,
            parent_task_id: None,
            title: title.to_owned(),
            description: None,
            lifecycle_state: TaskLifecycleState::Backlog,
            is_automation: false,
            priority: 0,
            subtask_order: None,
            merge_config: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("Task creates");
    id
}

async fn seed_repo_task(
    db: &SqliteDb,
    project_id: &str,
    repo_id: &str,
    title: &str,
    lifecycle_state: TaskLifecycleState,
) -> String {
    let now = now_rfc3339();
    let id = new_uuid_v4();
    TaskRepo::create(
        db,
        CreateTask {
            id: id.clone(),
            project_id: project_id.to_owned(),
            repo_id: Some(repo_id.to_owned()),
            parent_task_id: None,
            title: title.to_owned(),
            description: None,
            lifecycle_state,
            is_automation: false,
            priority: 0,
            subtask_order: None,
            merge_config: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("repository Task creates");
    id
}

fn domain_event(
    event_type: &str,
    entity_type: &str,
    entity_id: &str,
    task_id: &str,
    actor_id: &str,
) -> CreateDomainEvent {
    let id = new_uuid_v4();
    CreateDomainEvent {
        id: id.clone(),
        event_type: event_type.to_owned(),
        entity_type: entity_type.to_owned(),
        entity_id: entity_id.to_owned(),
        actor_type: "human".to_owned(),
        actor_id: Some(actor_id.to_owned()),
        scope_type: "task".to_owned(),
        scope_id: task_id.to_owned(),
        correlation_id: id,
        causation_id: None,
        causation_depth: 0,
        dedupe_key: None,
        payload_json: "{}".to_owned(),
        created_at: now_rfc3339(),
    }
}

fn run_git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("Git command starts");
    assert!(
        output.status.success(),
        "git {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("Git output is UTF-8")
        .trim()
        .to_owned()
}

fn create_git_history(root: &Path) -> (PathBuf, String, String, String, String) {
    let repo_path = root.join("repository");
    std::fs::create_dir_all(&repo_path).expect("temporary repository directory creates");
    run_git(&repo_path, &["init"]);
    run_git(&repo_path, &["checkout", "-b", "main"]);
    run_git(
        &repo_path,
        &["config", "user.email", "project-plan-test@forge.invalid"],
    );
    run_git(&repo_path, &["config", "user.name", "Project Plan Test"]);
    std::fs::write(repo_path.join("README.md"), "initial project state\n")
        .expect("initial project file writes");
    run_git(&repo_path, &["add", "README.md"]);
    run_git(&repo_path, &["commit", "-m", "initial project state"]);
    let initial_sha = run_git(&repo_path, &["rev-parse", "HEAD"]);

    std::fs::write(
        repo_path.join("predecessor.txt"),
        "accepted integrated result\n",
    )
    .expect("integrated predecessor result writes");
    run_git(&repo_path, &["add", "predecessor.txt"]);
    run_git(
        &repo_path,
        &["commit", "-m", "integrate predecessor result"],
    );
    let integrated_sha = run_git(&repo_path, &["rev-parse", "HEAD"]);

    std::fs::write(
        repo_path.join("successor-base.txt"),
        "successor starts here\n",
    )
    .expect("successor base file writes");
    run_git(&repo_path, &["add", "successor-base.txt"]);
    run_git(&repo_path, &["commit", "-m", "advance successor base"]);
    let successor_base_sha = run_git(&repo_path, &["rev-parse", "HEAD"]);

    run_git(
        &repo_path,
        &["checkout", "-b", "stale-successor-base", &initial_sha],
    );
    std::fs::write(
        repo_path.join("stale-base.txt"),
        "diverged from accepted result\n",
    )
    .expect("diverged base file writes");
    run_git(&repo_path, &["add", "stale-base.txt"]);
    run_git(
        &repo_path,
        &["commit", "-m", "diverge before predecessor integration"],
    );
    let stale_base_sha = run_git(&repo_path, &["rev-parse", "HEAD"]);
    run_git(&repo_path, &["checkout", "main"]);

    (
        repo_path,
        initial_sha,
        integrated_sha,
        successor_base_sha,
        stale_base_sha,
    )
}

async fn seed_implementer_role(db: &SqliteDb, task_id: &str) -> String {
    let now = now_rfc3339();
    let role_id = new_uuid_v4();
    TaskRoleRepo::create_with_configuration(
        db,
        CreateTaskRole {
            id: role_id.clone(),
            task_id: task_id.to_owned(),
            role: "implementer".to_owned(),
            coordination_mode: Some(db::CoordinationMode::Collaborative),
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
        r#"{"schema_version":1,"display_label":null,"capabilities":[],"rework":null}"#,
    )
    .await
    .expect("implementer TaskRole creates");
    role_id
}

async fn seed_repository_work_unit(
    db: &SqliteDb,
    task_id: &str,
    owner_id: &str,
    title: &str,
    assigned_actor: Option<ActorRef>,
) -> String {
    let work_unit_id = new_uuid_v4();
    WorkUnitRepo::create(
        db,
        CreateWorkUnit {
            id: work_unit_id.clone(),
            task_id: task_id.to_owned(),
            parent_work_unit_id: None,
            title: title.to_owned(),
            scope: title.to_owned(),
            execution_instructions: None,
            role: "implementer".to_owned(),
            assigned_actor,
            requires_integration: true,
            provenance: None,
            artifact_input_ids: Vec::new(),
            review_approval_gate_id: None,
            review_approval_line_id: None,
            created_by: ActorRef::Human(owner_id.to_owned()),
            created_at: now_rfc3339(),
        },
        domain_event(
            "work_unit.created",
            "work_unit",
            &work_unit_id,
            task_id,
            owner_id,
        ),
    )
    .await
    .expect("repository WorkUnit creates through the DB writer");
    work_unit_id
}

async fn seed_work_unit_workspace(
    db: &SqliteDb,
    task_id: &str,
    work_unit_id: &str,
    repo_id: &str,
    path: &Path,
    branch: &str,
    before_sha: &str,
) -> db::Workspace {
    let now = now_rfc3339();
    WorkUnitWorkspaceRepo::create_for_work_unit(
        db,
        CreateWorkUnitWorkspace {
            workspace: CreateWorkspace {
                id: new_uuid_v4(),
                task_id: task_id.to_owned(),
                repo_id: repo_id.to_owned(),
                worktree_path: path.to_string_lossy().into_owned(),
                branch: branch.to_owned(),
                status: WorkspaceStatus::Ready,
                before_sha: Some(before_sha.to_owned()),
                created_at: now.clone(),
                updated_at: now,
            },
            work_unit_id: work_unit_id.to_owned(),
        },
    )
    .await
    .expect("WorkUnit Workspace and exact scope create")
}

fn add_git_worktree(repo_path: &Path, path: &Path, branch: &str, commit_sha: &str) {
    let path = path.to_string_lossy().into_owned();
    run_git(
        repo_path,
        &["worktree", "add", "-b", branch, &path, commit_sha],
    );
}

async fn seed_project(fixture: &Fixture, owner_id: &str, name: &str) -> String {
    let now = now_rfc3339();
    let project_id = new_uuid_v4();
    ProjectRepo::create(
        fixture.db.as_ref(),
        CreateProject {
            id: project_id.clone(),
            name: name.to_owned(),
            settings: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: Some(owner_id.to_owned()),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("second Project creates");
    project_id
}

async fn seed_agent(fixture: &Fixture, agent_id: &str) {
    let now = now_rfc3339();
    if AgentRepo::get_by_id(&*fixture.db, agent_id)
        .await
        .expect("Agent lookup succeeds")
        .is_none()
    {
        AgentRepo::create(
            &*fixture.db,
            CreateAgent {
                id: agent_id.to_owned(),
                name: "Project plan test coordinator".to_owned(),
                description: None,
                executor_type: "codex".to_owned(),
                model: None,
                reasoning_effort: None,
                permission_policy: None,
                prompt_template: None,
                capabilities_json: "[]".to_owned(),
                config_json: "{}".to_owned(),
                credential_ref: None,
                daemon_id: None,
                max_concurrent_tasks: 2,
                heartbeat_interval_seconds: 30,
                max_missed_heartbeats: 3,
                status: AgentStatus::Idle,
                last_heartbeat_at: None,
                is_default: false,
                paused: false,
                owner_id: Some(fixture.owner_id.clone()),
                visibility: "global".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("test Agent creates without starting a provider");
    }
}

async fn seed_agent_coordinator(fixture: &Fixture, task_id: &str, agent_id: &str) {
    seed_agent(fixture, agent_id).await;
    let now = now_rfc3339();
    let role_id = new_uuid_v4();
    TaskRoleRepo::create_with_configuration(
        &*fixture.db,
        CreateTaskRole {
            id: role_id.clone(),
            task_id: task_id.to_owned(),
            role: "orchestrator".to_owned(),
            coordination_mode: Some(db::CoordinationMode::Collaborative),
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        r#"{"schema_version":1,"display_label":null,"capabilities":["orchestration"],"rework":null}"#,
    )
    .await
    .expect("orchestrator TaskRole with explicit capability creates");
    RoleMembershipRepo::add(
        &*fixture.db,
        CreateRoleMembership {
            id: new_uuid_v4(),
            task_role_id: role_id.clone(),
            actor_kind: ActorKind::Agent,
            actor_id: agent_id.to_owned(),
            status: RoleMembershipStatus::Active,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("Agent becomes an active member of the exact role");

    let task_service = TaskService::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus));
    task_service
        .create_task_role_with_configuration(
            task_id,
            "reviewer",
            db::CoordinationMode::Independent,
            "{}".to_owned(),
            r#"{"schema_version":1,"display_label":null,"capabilities":["formal_review"],"rework":null}"#.to_owned(),
        )
        .await
        .expect("reviewer TaskRole with formal_review capability creates through TaskService");
    task_service
        .add_task_role_member(
            task_id,
            "reviewer",
            api_types::ActorRef::Agent(agent_id.to_owned()),
        )
        .await
        .expect("Agent becomes an active member of the formal reviewer role");

    let task = TaskRepo::get_by_id(&*fixture.db, task_id, false)
        .await
        .expect("Task lookup succeeds")
        .expect("Task exists");
    let coordination = TaskCoordinationRepo::get_task_coordination(&*fixture.db, task_id)
        .await
        .expect("coordination lookup succeeds")
        .expect("new Task has manual coordination state");
    crate::task_coordination::TaskCoordinationService::new(
        Arc::clone(&fixture.db),
        Arc::clone(&fixture.event_bus),
    )
    .authorize(
        task_id,
        &fixture.owner_id,
        crate::task_coordination::AuthorizationInput {
            expected_task_version: task.version,
            expected_coordination_version: coordination.authorization_version,
            mode: TaskCoordinationMode::PrepareAndExecute,
            coordinator_task_role_id: Some(role_id),
            coordinator_actor: Some(ActorRef::Agent(agent_id.to_owned())),
            envelope: None,
            idempotency_key: format!("test-coordinator-{task_id}"),
        },
    )
    .await
    .expect("owner authorizes the exact Task role and Actor");
}

fn node(key: &str, task_id: Option<&str>, title: &str) -> ProjectPlanNode {
    ProjectPlanNode {
        key: key.to_owned(),
        task_id: task_id.map(str::to_owned),
        title: title.to_owned(),
        description: None,
        acceptance_criteria: Vec::new(),
        phase: None,
        estimate_min_minutes: None,
        estimate_max_minutes: None,
        source_ids: Vec::new(),
    }
}

fn document(nodes: Vec<ProjectPlanNode>, edges: Vec<ProjectPlanEdge>) -> ProjectPlanDocument {
    ProjectPlanDocument {
        nodes,
        edges,
        sources: Vec::new(),
    }
}

#[tokio::test]
async fn graph_without_an_applied_plan_includes_all_tasks_and_manual_dependencies() {
    let fixture = fixture().await;
    let prerequisite = seed_task(&fixture.db, &fixture.project_id, "Manual prerequisite").await;
    let dependent = seed_task(&fixture.db, &fixture.project_id, "Manual dependent").await;

    let empty_plan = fixture
        .service
        .graph(&fixture.project_id)
        .await
        .expect("graph loads without an applied plan");
    assert_eq!(empty_plan.nodes.len(), 2);
    assert!(
        empty_plan.version >= 3,
        "each Task insert advances the graph version"
    );
    assert!(empty_plan.edges.is_empty());

    TaskDependencyRepo::add_dependency(&*fixture.db, &dependent, &prerequisite, &now_rfc3339())
        .await
        .expect("manual Task dependency adds");

    let graph = fixture
        .service
        .graph(&fixture.project_id)
        .await
        .expect("graph includes canonical dependencies");
    assert_eq!(graph.nodes.len(), 2);
    assert_eq!(graph.edges.len(), 1);
    assert!(
        graph.version > empty_plan.version,
        "manual dependencies advance the graph version"
    );
    let edge = &graph.edges[0];
    assert_eq!(edge.predecessor_key, format!("@task:{prerequisite}"));
    assert_eq!(edge.successor_key, format!("@task:{dependent}"));
    assert!(!edge.inferred);
    assert!(edge.source_ids.is_empty());
    let dependent_node = graph
        .nodes
        .iter()
        .find(|node| node.task_id == dependent)
        .expect("dependent Task is present");
    assert_eq!(dependent_node.readiness, "blocked");
    assert_eq!(dependent_node.unsatisfied_task_ids, vec![prerequisite]);
}

#[tokio::test]
async fn applying_a_plan_preserves_manual_edges_maps_existing_tasks_and_replays_without_duplicates()
{
    let fixture = fixture().await;
    let first = seed_task(&fixture.db, &fixture.project_id, "Existing A").await;
    let second = seed_task(&fixture.db, &fixture.project_id, "Existing B").await;
    let manual_successor = seed_task(&fixture.db, &fixture.project_id, "Manual successor").await;
    TaskDependencyRepo::add_dependency(&*fixture.db, &manual_successor, &second, &now_rfc3339())
        .await
        .expect("manual edge adds");

    let initial = fixture
        .service
        .graph(&fixture.project_id)
        .await
        .expect("graph loads");
    let plan = ProjectPlanDocument {
        nodes: vec![
            {
                let mut value = node("plan-a", Some(&first), "Existing A");
                value.source_ids.push("brief".to_owned());
                value
            },
            node("plan-b", Some(&second), "Existing B"),
            node("manual-task", Some(&manual_successor), "Manual successor"),
            node("new-task", None, "New planned Task"),
        ],
        edges: vec![ProjectPlanEdge {
            predecessor_key: "plan-a".to_owned(),
            successor_key: "plan-b".to_owned(),
            inferred: true,
            source_ids: vec!["brief".to_owned()],
        }],
        sources: vec![ProjectPlanSource {
            id: "brief".to_owned(),
            label: "Project brief".to_owned(),
            kind: "document".to_owned(),
            reference: Some("brief.md".to_owned()),
            revision: Some("r4".to_owned()),
            excerpt: Some("Ship A before B.".to_owned()),
        }],
    };
    let proposal = fixture
        .service
        .propose(
            &fixture.project_id,
            &fixture.owner_id,
            initial.version,
            plan,
            "proposal-once",
        )
        .await
        .expect("proposal records");
    assert_eq!(proposal.state, "proposed");

    let applied = fixture
        .service
        .apply(
            &fixture.project_id,
            &fixture.owner_id,
            &proposal.id,
            initial.version,
            "apply-once",
        )
        .await
        .expect("proposal applies");
    assert_eq!(applied.nodes.len(), 4);
    assert_eq!(applied.edges.len(), 2);
    assert!(applied.nodes.iter().any(|node| node.task_id == first));
    assert!(applied.nodes.iter().any(|node| node.task_id == second));
    assert!(applied
        .nodes
        .iter()
        .any(|node| node.task_id == manual_successor));
    let added_task = applied
        .nodes
        .iter()
        .find(|node| node.title == "New planned Task")
        .expect("unmapped plan node creates a Task");
    assert!(!added_task.task_id.is_empty());
    let added_task_id = added_task.task_id.clone();
    let planned_edge = applied
        .edges
        .iter()
        .find(|edge| edge.predecessor_key == "plan-a" && edge.successor_key == "plan-b")
        .expect("plan dependency is projected");
    assert!(planned_edge.inferred);
    assert_eq!(planned_edge.source_ids, vec!["brief".to_owned()]);
    let retained_manual_edge = applied
        .edges
        .iter()
        .find(|edge| edge.predecessor_key == "plan-b" && edge.successor_key == "manual-task")
        .expect("manual dependency remains in the canonical graph");
    assert!(!retained_manual_edge.inferred);
    assert_eq!(applied.sources[0].revision.as_deref(), Some("r4"));
    let applied_proposal = applied
        .proposals
        .iter()
        .find(|candidate| candidate.id == proposal.id)
        .expect("proposal history remains available");
    assert_eq!(applied_proposal.state, "applied");
    assert_eq!(applied_proposal.applied_version, Some(applied.version));

    let replay = fixture
        .service
        .apply(
            &fixture.project_id,
            &fixture.owner_id,
            &proposal.id,
            initial.version,
            "apply-once",
        )
        .await
        .expect("same import receipt replays");
    assert_eq!(replay.nodes.len(), 4);
    assert_eq!(
        replay
            .nodes
            .iter()
            .filter(|node| node.title == "New planned Task")
            .count(),
        1,
    );
    assert_eq!(
        replay
            .nodes
            .iter()
            .find(|node| node.key == "new-task")
            .map(|node| node.task_id.as_str()),
        Some(added_task_id.as_str()),
        "an apply receipt replays the same imported Task after its graph revision advanced"
    );
    assert_eq!(replay.version, applied.version);

    let updated = document(
        vec![
            node("plan-a", Some(&first), "Existing A"),
            node("plan-b", Some(&second), "Existing B"),
            node("manual-task", Some(&manual_successor), "Manual successor"),
            node("new-task", Some(&added_task_id), "New planned Task"),
        ],
        vec![ProjectPlanEdge {
            predecessor_key: "plan-a".to_owned(),
            successor_key: "plan-b".to_owned(),
            inferred: true,
            source_ids: Vec::new(),
        }],
    );
    let update_proposal = fixture
        .service
        .propose(
            &fixture.project_id,
            &fixture.owner_id,
            replay.version,
            updated,
            "proposal-update-existing-mapping",
        )
        .await
        .expect("a later revision may map the same existing Task identities explicitly");
    let updated_graph = fixture
        .service
        .apply(
            &fixture.project_id,
            &fixture.owner_id,
            &update_proposal.id,
            replay.version,
            "apply-update-existing-mapping",
        )
        .await
        .expect("an explicit mapping applies without replacing Task identity");
    assert!(updated_graph.nodes.iter().any(|node| node.task_id == first));
    assert!(updated_graph
        .nodes
        .iter()
        .any(|node| node.task_id == second));
    assert!(updated_graph
        .nodes
        .iter()
        .any(|node| node.task_id == manual_successor));
    assert!(updated_graph
        .nodes
        .iter()
        .any(|node| node.task_id == added_task_id));

    let ambiguous_version = updated_graph.version;
    let ambiguous = fixture
        .service
        .propose(
            &fixture.project_id,
            &fixture.owner_id,
            ambiguous_version,
            document(
                vec![node("new-task", None, "Ambiguous reused key")],
                Vec::new(),
            ),
            "proposal-ambiguous-reused-key",
        )
        .await;
    assert!(
        matches!(ambiguous, Err(ServiceError::InvalidOperation { .. })),
        "a key with durable Task history cannot be remapped by creating an implicit duplicate"
    );
}

#[tokio::test]
async fn proposal_rejects_cycles_and_cross_project_task_mappings() {
    let fixture = fixture().await;
    let first = seed_task(&fixture.db, &fixture.project_id, "First").await;
    let second = seed_task(&fixture.db, &fixture.project_id, "Second").await;
    let initial = fixture
        .service
        .graph(&fixture.project_id)
        .await
        .expect("initial graph loads");
    let cyclic = document(
        vec![
            node("a", Some(&first), "First"),
            node("b", Some(&second), "Second"),
        ],
        vec![
            ProjectPlanEdge {
                predecessor_key: "a".to_owned(),
                successor_key: "b".to_owned(),
                inferred: false,
                source_ids: Vec::new(),
            },
            ProjectPlanEdge {
                predecessor_key: "b".to_owned(),
                successor_key: "a".to_owned(),
                inferred: false,
                source_ids: Vec::new(),
            },
        ],
    );
    let cycle_error = fixture
        .service
        .propose(
            &fixture.project_id,
            &fixture.owner_id,
            initial.version,
            cyclic,
            "cycle-proposal",
        )
        .await
        .expect_err("cycle is rejected before proposal persistence");
    assert!(matches!(
        cycle_error,
        ServiceError::Db(db::DbError::CycleDetected)
    ));

    let other_project = seed_project(&fixture, &fixture.owner_id, "Other Project").await;
    let foreign_task = seed_task(&fixture.db, &other_project, "Foreign Task").await;
    let foreign_mapping = document(
        vec![node("foreign", Some(&foreign_task), "Foreign Task")],
        vec![],
    );
    let mapping_error = fixture
        .service
        .propose(
            &fixture.project_id,
            &fixture.owner_id,
            initial.version,
            foreign_mapping,
            "cross-project-proposal",
        )
        .await
        .expect_err("Task mapping must stay within its Project");
    assert!(matches!(
        mapping_error,
        ServiceError::NotFound { entity: "task", .. }
    ));
    assert!(fixture
        .service
        .graph(&fixture.project_id)
        .await
        .expect("graph reloads")
        .proposals
        .is_empty());
}

#[tokio::test]
async fn apply_rejects_a_cycle_between_new_plan_edges_and_existing_manual_edges() {
    let fixture = fixture().await;
    let first = seed_task(&fixture.db, &fixture.project_id, "First").await;
    let second = seed_task(&fixture.db, &fixture.project_id, "Second").await;
    let third = seed_task(&fixture.db, &fixture.project_id, "Third").await;
    // Existing edge: Second -> Third. The proposed plan adds Third -> First -> Second.
    TaskDependencyRepo::add_dependency(&*fixture.db, &third, &second, &now_rfc3339())
        .await
        .expect("manual edge adds");
    let initial = fixture
        .service
        .graph(&fixture.project_id)
        .await
        .expect("graph loads");
    let acyclic_proposal = document(
        vec![
            node("first", Some(&first), "First"),
            node("second", Some(&second), "Second"),
            node("third", Some(&third), "Third"),
        ],
        vec![
            ProjectPlanEdge {
                predecessor_key: "third".to_owned(),
                successor_key: "first".to_owned(),
                inferred: false,
                source_ids: Vec::new(),
            },
            ProjectPlanEdge {
                predecessor_key: "first".to_owned(),
                successor_key: "second".to_owned(),
                inferred: false,
                source_ids: Vec::new(),
            },
        ],
    );
    let proposal = fixture
        .service
        .propose(
            &fixture.project_id,
            &fixture.owner_id,
            initial.version,
            acyclic_proposal,
            "combined-cycle-proposal",
        )
        .await
        .expect("proposal alone is acyclic");

    let error = fixture
        .service
        .apply(
            &fixture.project_id,
            &fixture.owner_id,
            &proposal.id,
            initial.version,
            "combined-cycle-apply",
        )
        .await
        .expect_err("manual and proposed edges cannot form a cycle");
    assert!(matches!(
        error,
        ServiceError::Db(db::DbError::CycleDetected)
    ));
    let after_rejection = fixture
        .service
        .graph(&fixture.project_id)
        .await
        .expect("graph reloads after rolled back apply");
    assert_eq!(after_rejection.edges.len(), 1);
    assert_eq!(
        after_rejection
            .proposals
            .iter()
            .find(|candidate| candidate.id == proposal.id)
            .expect("rejected proposal remains inspectable")
            .state,
        "proposed"
    );
}

#[tokio::test]
async fn proposal_replay_is_idempotent_and_graph_version_is_fenced() {
    let fixture = fixture().await;
    let first = seed_task(&fixture.db, &fixture.project_id, "First").await;
    let second = seed_task(&fixture.db, &fixture.project_id, "Second").await;
    let initial = fixture
        .service
        .graph(&fixture.project_id)
        .await
        .expect("initial graph loads");
    let plan = document(vec![node("first", Some(&first), "First")], vec![]);
    let original = fixture
        .service
        .propose(
            &fixture.project_id,
            &fixture.owner_id,
            initial.version,
            plan.clone(),
            "proposal-idempotency",
        )
        .await
        .expect("proposal records");
    let replay = fixture
        .service
        .propose(
            &fixture.project_id,
            &fixture.owner_id,
            initial.version,
            plan,
            "proposal-idempotency",
        )
        .await
        .expect("same proposal request replays");
    assert_eq!(replay.id, original.id);

    let conflicting = document(vec![node("first", Some(&first), "Changed title")], vec![]);
    let idempotency_error = fixture
        .service
        .propose(
            &fixture.project_id,
            &fixture.owner_id,
            initial.version,
            conflicting,
            "proposal-idempotency",
        )
        .await
        .expect_err("same key cannot stand for a different proposal");
    assert!(matches!(
        idempotency_error,
        ServiceError::Db(db::DbError::IdempotencyConflict)
    ));

    TaskDependencyRepo::add_dependency(&*fixture.db, &second, &first, &now_rfc3339())
        .await
        .expect("canonical dependency advances graph version");
    let stale_error = fixture
        .service
        .propose(
            &fixture.project_id,
            &fixture.owner_id,
            initial.version,
            document(vec![node("second", Some(&second), "Second")], vec![]),
            "stale-version-proposal",
        )
        .await
        .expect_err("stale graph version is rejected");
    assert!(matches!(
        stale_error,
        ServiceError::Db(db::DbError::VersionConflict)
    ));
}

#[tokio::test]
async fn project_write_membership_is_revalidated_before_proposal_creation() {
    let fixture = fixture().await;
    let now = now_rfc3339();
    let outsider_id = new_uuid_v4();
    UserRepo::create_user(
        &*fixture.db,
        &User {
            id: outsider_id.clone(),
            email: format!("{outsider_id}@example.test"),
            password_hash: "test-hash".to_owned(),
            display_name: None,
            is_admin: false,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("outsider user creates");
    let graph = fixture
        .service
        .graph(&fixture.project_id)
        .await
        .expect("graph loads");
    let error = fixture
        .service
        .propose(
            &fixture.project_id,
            &outsider_id,
            graph.version,
            ProjectPlanDocument::default(),
            "outsider-proposal",
        )
        .await
        .expect_err("Project visibility alone does not grant write authority");
    assert!(matches!(error, ServiceError::AuthorizationDenied { .. }));
}

#[tokio::test]
async fn batch_authorization_requires_project_admin_and_controls_only_the_selected_tasks() {
    let fixture = fixture().await;
    let task_id = seed_task(&fixture.db, &fixture.project_id, "Independent manual Task").await;

    let now = now_rfc3339();
    let member_id = new_uuid_v4();
    UserRepo::create_user(
        &*fixture.db,
        &User {
            id: member_id.clone(),
            email: format!("{member_id}@example.test"),
            password_hash: "test-hash".to_owned(),
            display_name: Some("Project member".to_owned()),
            is_admin: false,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("member user creates");
    ProjectMemberRepo::add_member(
        &*fixture.db,
        CreateProjectMember {
            id: new_uuid_v4(),
            project_id: fixture.project_id.clone(),
            user_id: member_id.clone(),
            role: "member".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("ordinary Project member is added");

    let graph = fixture
        .service
        .graph(&fixture.project_id)
        .await
        .expect("initial graph loads");
    let denied = fixture
        .service
        .authorize_batch(
            &fixture.project_id,
            &member_id,
            graph.version,
            vec![task_id.clone()],
            2,
            "member-batch-denied",
        )
        .await
        .expect_err("a Project member cannot authorize execution");
    assert!(matches!(denied, ServiceError::AuthorizationDenied { .. }));

    let batch = fixture
        .service
        .authorize_batch(
            &fixture.project_id,
            &fixture.owner_id,
            graph.version,
            vec![task_id.clone()],
            2,
            "owner-batch-once",
        )
        .await
        .expect("owner authorizes the selected Task batch");
    assert_eq!(batch.state, "running");
    assert_eq!(batch.max_parallel_tasks, 2);
    assert_eq!(batch.task_ids, vec![task_id.clone()]);

    let persisted_task_state: (String, Option<String>) = sqlx::query_as(
        "SELECT state,blocker FROM project_plan_batch_task WHERE batch_id=? AND task_id=?",
    )
    .bind(&batch.id)
    .bind(&task_id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("batch task preparation remains inspectable");
    assert_eq!(persisted_task_state.0, "blocked");
    assert_eq!(
        persisted_task_state.1.as_deref(),
        Some("coordination:coordinator_role_missing")
    );

    let replay = fixture
        .service
        .authorize_batch(
            &fixture.project_id,
            &fixture.owner_id,
            graph.version,
            vec![task_id.clone()],
            2,
            "owner-batch-once",
        )
        .await
        .expect("exact batch authorization request replays");
    assert_eq!(replay.id, batch.id);

    assert_eq!(
        fixture
            .service
            .check_once()
            .await
            .expect("scheduler leaves unconfigured coordinator blocked"),
        0,
    );
    let request_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task_coordination_request WHERE task_id=?")
            .bind(&task_id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("coordination request count loads");
    assert_eq!(
        request_count, 0,
        "batch authorization does not start a model turn without a configured coordinator"
    );

    let paused = fixture
        .service
        .update_batch(
            &fixture.project_id,
            &fixture.owner_id,
            &batch.id,
            batch.version,
            "pause",
            "owner-batch-pause",
        )
        .await
        .expect("owner can pause the authorized batch");
    assert_eq!(paused.state, "paused");
    let resumed = fixture
        .service
        .update_batch(
            &fixture.project_id,
            &fixture.owner_id,
            &batch.id,
            paused.version,
            "resume",
            "owner-batch-resume",
        )
        .await
        .expect("owner can resume the prepared batch");
    assert_eq!(resumed.state, "running");
    let revoked = fixture
        .service
        .update_batch(
            &fixture.project_id,
            &fixture.owner_id,
            &batch.id,
            resumed.version,
            "revoke",
            "owner-batch-revoke",
        )
        .await
        .expect("owner can revoke the batch");
    assert_eq!(revoked.state, "revoked");
}

#[tokio::test]
async fn batch_scoped_coordination_requires_fresh_human_reauthorization_after_revoke() {
    let fixture = fixture().await;
    let task_id = seed_task(
        &fixture.db,
        &fixture.project_id,
        "Task reauthorized after batch revoke",
    )
    .await;
    let agent_id = new_uuid_v4();
    seed_agent_coordinator(&fixture, &task_id, &agent_id).await;

    let graph = fixture
        .service
        .graph(&fixture.project_id)
        .await
        .expect("graph with a configured coordinator loads");
    let batch = fixture
        .service
        .authorize_batch(
            &fixture.project_id,
            &fixture.owner_id,
            graph.version,
            vec![task_id.clone()],
            1,
            "reauthorization-after-revoke-batch",
        )
        .await
        .expect("owner authorizes the exact Task");
    let active_task = TaskRepo::get_by_id(&*fixture.db, &task_id, false)
        .await
        .expect("Task reloads while its batch is running")
        .expect("Task remains available");
    let active_coordination = TaskCoordinationRepo::get_task_coordination(&*fixture.db, &task_id)
        .await
        .expect("batch-scoped coordination loads while running")
        .expect("Task has a durable coordination record");
    let active_role_id = active_coordination
        .coordinator_task_role_id
        .clone()
        .expect("batch coordinator role remains exact");
    let active_actor = active_coordination
        .coordinator_actor
        .clone()
        .expect("batch coordinator Actor remains exact");
    let coordination_service = crate::task_coordination::TaskCoordinationService::new(
        Arc::clone(&fixture.db),
        Arc::clone(&fixture.event_bus),
    );
    let human_authorization =
        |idempotency_key: &str| crate::task_coordination::AuthorizationInput {
            expected_task_version: active_task.version,
            expected_coordination_version: active_coordination.authorization_version,
            mode: TaskCoordinationMode::PrepareAndExecute,
            coordinator_task_role_id: Some(active_role_id.clone()),
            coordinator_actor: Some(active_actor.clone()),
            envelope: None,
            idempotency_key: idempotency_key.to_owned(),
        };
    let running_reauthorization = coordination_service
        .authorize(
            &task_id,
            &fixture.owner_id,
            human_authorization("generic-authorize-running-batch"),
        )
        .await;
    assert!(matches!(
        running_reauthorization,
        Err(ServiceError::Db(db::DbError::ProjectPlanBatchInactive))
    ));
    let paused = fixture
        .service
        .update_batch(
            &fixture.project_id,
            &fixture.owner_id,
            &batch.id,
            batch.version,
            "pause",
            "reauthorization-active-batch-pause",
        )
        .await
        .expect("owner pauses the active Project batch");
    let paused_reauthorization = coordination_service
        .authorize(
            &task_id,
            &fixture.owner_id,
            human_authorization("generic-authorize-paused-batch"),
        )
        .await;
    assert!(matches!(
        paused_reauthorization,
        Err(ServiceError::Db(db::DbError::ProjectPlanBatchInactive))
    ));
    let coordination_after_rejections =
        TaskCoordinationRepo::get_task_coordination(&*fixture.db, &task_id)
            .await
            .expect("coordination remains inspectable after rejected changes")
            .expect("batch authorization remains durable");
    assert_eq!(
        coordination_after_rejections.authorization_version,
        active_coordination.authorization_version
    );
    let revoked = fixture
        .service
        .update_batch(
            &fixture.project_id,
            &fixture.owner_id,
            &batch.id,
            paused.version,
            "revoke",
            "reauthorization-after-revoke",
        )
        .await
        .expect("owner revokes the prior batch");
    assert_eq!(revoked.state, "revoked");

    let task = TaskRepo::get_by_id(&*fixture.db, &task_id, false)
        .await
        .expect("Task reloads after revoke")
        .expect("Task remains available");
    let coordination = TaskCoordinationRepo::get_task_coordination(&*fixture.db, &task_id)
        .await
        .expect("coordination reloads after revoke")
        .expect("Task has a durable coordination record");
    let task_role_id = coordination
        .coordinator_task_role_id
        .clone()
        .expect("exact coordinator role remains configured");
    let actor = coordination
        .coordinator_actor
        .clone()
        .expect("exact coordinator Actor remains configured");
    let previous_authorization_version = coordination.authorization_version;
    let service = crate::task_coordination::TaskCoordinationService::new(
        Arc::clone(&fixture.db),
        Arc::clone(&fixture.event_bus),
    );
    let old_batch_request = service
        .request_turn(
            &task_id,
            &fixture.owner_id,
            crate::task_coordination::TurnRequestInput {
                expected_task_version: task.version,
                expected_coordination_version: coordination.authorization_version,
                brief: crate::task_coordination::CoordinationBrief {
                    objective: task.title.clone(),
                    constraints: None,
                    acceptance_criteria: Vec::new(),
                    context: None,
                },
                idempotency_key: "batch-scoped-request-after-revoke".to_owned(),
            },
        )
        .await;
    assert!(
        matches!(
            old_batch_request,
            Err(ServiceError::Db(db::DbError::ProjectPlanBatchInactive))
        ),
        "a revoked batch's exact authorization cannot be reused as a generic Task turn"
    );
    let turns_after_rejection: i64 =
        sqlx::query_scalar("SELECT turn_count FROM task_coordination WHERE task_id=?")
            .bind(&task_id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("rejected batch-scoped request leaves its budget inspectable");
    assert_eq!(turns_after_rejection, coordination.turn_count);

    let reauthorized = service
        .authorize_with_expected_team_generation(
            &task_id,
            &fixture.owner_id,
            crate::task_coordination::AuthorizationInput {
                expected_task_version: task.version,
                expected_coordination_version: coordination.authorization_version,
                mode: TaskCoordinationMode::PrepareAndExecute,
                coordinator_task_role_id: Some(task_role_id.clone()),
                coordinator_actor: Some(actor.clone()),
                envelope: None,
                idempotency_key: "human-reauthorization-after-batch-revoke".to_owned(),
            },
            coordination.team_generation,
        )
        .await
        .expect("a fresh Human authorization is independent of the revoked batch history");
    assert_eq!(
        reauthorized.coordination.authorization_version,
        previous_authorization_version + 1,
    );
    assert_eq!(
        reauthorized.coordination.requested_by.as_deref(),
        Some(fixture.owner_id.as_str()),
    );
    assert_eq!(
        reauthorized.coordination.team_generation,
        coordination.team_generation
    );
    assert_eq!(
        reauthorized
            .coordination
            .coordinator_task_role_id
            .as_deref(),
        Some(task_role_id.as_str()),
        "the Human reauthorization keeps the exact configured TaskRole"
    );
    assert_eq!(
        reauthorized.coordination.coordinator_actor.as_ref(),
        Some(&actor)
    );
    assert!(reauthorized.coordination.revoked_at.is_none());
    let (request, created) = service
        .request_turn(
            &task_id,
            &fixture.owner_id,
            crate::task_coordination::TurnRequestInput {
                expected_task_version: reauthorized.task.version,
                expected_coordination_version: reauthorized.coordination.authorization_version,
                brief: crate::task_coordination::CoordinationBrief {
                    objective: task.title.clone(),
                    constraints: None,
                    acceptance_criteria: Vec::new(),
                    context: None,
                },
                idempotency_key: "human-request-after-revoked-batch".to_owned(),
            },
        )
        .await
        .expect("a new TaskCoordination turn uses the fresh Human authorization");
    assert!(created);
    assert_eq!(request.task_id, task_id);
    assert_eq!(
        request.authorization_version,
        reauthorized.coordination.authorization_version,
    );
    let historical_batch_task: (String, Option<String>) = sqlx::query_as(
        "SELECT state,blocker FROM project_plan_batch_task WHERE batch_id=? AND task_id=?",
    )
    .bind(&batch.id)
    .bind(&task_id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("the old batch's audit row remains inspectable");
    assert_eq!(historical_batch_task.0, "blocked");
    assert_eq!(historical_batch_task.1.as_deref(), Some("batch:revoked"));
    let request_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_coordination_request WHERE task_id=? AND authorization_version=?",
    )
    .bind(&task_id)
    .bind(reauthorized.coordination.authorization_version)
    .fetch_one(fixture.db.pool())
    .await
    .expect("new Human-authorized turn remains inspectable");
    assert_eq!(request_count, 1);
    let execution_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE task_id=?")
        .bind(&task_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("no provider Execution is created by the request");
    assert_eq!(execution_count, 0);
}

#[tokio::test]
async fn batch_coordination_grant_is_scoped_before_pin_and_survives_revoke() {
    let fixture = fixture().await;
    let task_id = seed_task(&fixture.db, &fixture.project_id, "Task scoped before pin").await;
    let agent_id = new_uuid_v4();
    seed_agent_coordinator(&fixture, &task_id, &agent_id).await;

    let graph = fixture
        .service
        .graph(&fixture.project_id)
        .await
        .expect("graph with a configured coordinator loads");
    let batch = fixture
        .service
        .create_batch_record(
            &fixture.project_id,
            &fixture.owner_id,
            graph.version,
            vec![task_id.clone()],
            1,
            "scope-before-pin-batch",
        )
        .await
        .expect("owner creates an authorized batch before coordination preparation");
    assert_eq!(batch.state, "paused");
    let preparation_complete: i64 =
        sqlx::query_scalar("SELECT preparation_complete FROM project_plan_batch WHERE id=?")
            .bind(&batch.id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("batch remains at the pre-pin boundary");
    assert_eq!(preparation_complete, 0);

    let task = TaskRepo::get_by_id(&*fixture.db, &task_id, false)
        .await
        .expect("Task reloads")
        .expect("Task remains available");
    let coordination = TaskCoordinationRepo::get_task_coordination(&*fixture.db, &task_id)
        .await
        .expect("coordination reloads")
        .expect("Task has an explicit coordinator");
    let task_role_id = coordination
        .coordinator_task_role_id
        .clone()
        .expect("exact coordinator TaskRole is configured");
    let actor = coordination
        .coordinator_actor
        .clone()
        .expect("exact coordinator Actor is configured");
    let service = crate::task_coordination::TaskCoordinationService::new(
        Arc::clone(&fixture.db),
        Arc::clone(&fixture.event_bus),
    );
    let authorized = service
        .authorize_for_project_plan_batch(
            &task_id,
            &fixture.owner_id,
            crate::task_coordination::AuthorizationInput {
                expected_task_version: task.version,
                expected_coordination_version: coordination.authorization_version,
                mode: TaskCoordinationMode::PrepareAndExecute,
                coordinator_task_role_id: Some(task_role_id),
                coordinator_actor: Some(actor),
                envelope: None,
                idempotency_key: "scope-before-pin-coordination".to_owned(),
            },
            db::ProjectPlanBatchAuthorizationBinding {
                project_id: fixture.project_id.clone(),
                batch_id: batch.id.clone(),
                authorized_by: fixture.owner_id.clone(),
                expected_batch_version: batch.version,
                expected_graph_version: graph.version,
                expected_team_generation: coordination.team_generation,
            },
        )
        .await
        .expect("coordination authorization and its batch origin commit together");
    assert_eq!(
        authorized.coordination.authorization_version,
        coordination.authorization_version + 1,
    );

    let exact_scope_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM project_plan_batch_coordination_authorization
         WHERE batch_id=? AND project_id=? AND task_id=? AND authorization_version=?
           AND authorized_by=? AND expected_batch_version=? AND expected_graph_version=?",
    )
    .bind(&batch.id)
    .bind(&fixture.project_id)
    .bind(&task_id)
    .bind(authorized.coordination.authorization_version)
    .bind(&fixture.owner_id)
    .bind(batch.version)
    .bind(graph.version)
    .fetch_one(fixture.db.pool())
    .await
    .expect("authorization source row remains inspectable before pin");
    assert_eq!(exact_scope_count, 1);
    let unpinned_snapshot: Option<String> = sqlx::query_scalar(
        "SELECT coordination_snapshot_digest FROM project_plan_batch_task
         WHERE batch_id=? AND task_id=?",
    )
    .bind(&batch.id)
    .bind(&task_id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("Task batch row remains inspectable before pin");
    assert!(unpinned_snapshot.is_none());

    let revoked = fixture
        .service
        .update_batch(
            &fixture.project_id,
            &fixture.owner_id,
            &batch.id,
            batch.version,
            "revoke",
            "scope-before-pin-revoke",
        )
        .await
        .expect("owner can revoke between authorization and coordinator pin");
    assert_eq!(revoked.state, "revoked");

    let request = service
        .request_turn(
            &task_id,
            &fixture.owner_id,
            crate::task_coordination::TurnRequestInput {
                expected_task_version: authorized.task.version,
                expected_coordination_version: authorized.coordination.authorization_version,
                brief: crate::task_coordination::CoordinationBrief {
                    objective: task.title,
                    constraints: None,
                    acceptance_criteria: Vec::new(),
                    context: None,
                },
                idempotency_key: "generic-turn-before-pin-revoke".to_owned(),
            },
        )
        .await;
    assert!(matches!(
        request,
        Err(ServiceError::Db(db::DbError::ProjectPlanBatchInactive))
    ));
    let turns_after_rejection: i64 =
        sqlx::query_scalar("SELECT turn_count FROM task_coordination WHERE task_id=?")
            .bind(&task_id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("rejected generic turn leaves its budget inspectable");
    assert_eq!(turns_after_rejection, coordination.turn_count);
    let request_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task_coordination_request WHERE task_id=?")
            .bind(&task_id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("no generic request escapes the revoked batch scope");
    assert_eq!(request_count, 0);
    let execution_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE task_id=?")
        .bind(&task_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("no Execution escapes the revoked batch scope");
    assert_eq!(execution_count, 0);
    let batch_task: (String, Option<String>) = sqlx::query_as(
        "SELECT state,blocker FROM project_plan_batch_task WHERE batch_id=? AND task_id=?",
    )
    .bind(&batch.id)
    .bind(&task_id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("revoked batch row remains inspectable");
    assert_eq!(batch_task.0, "blocked");
    assert_eq!(batch_task.1.as_deref(), Some("batch:revoked"));
}

#[tokio::test]
async fn scheduler_respects_batch_parallel_limit_without_starting_a_provider() {
    let fixture = fixture().await;
    let first = seed_task(&fixture.db, &fixture.project_id, "First independent packet").await;
    let second = seed_task(
        &fixture.db,
        &fixture.project_id,
        "Second independent packet",
    )
    .await;
    let agent_id = new_uuid_v4();
    seed_agent_coordinator(&fixture, &first, &agent_id).await;
    seed_agent_coordinator(&fixture, &second, &agent_id).await;

    let graph = fixture
        .service
        .graph(&fixture.project_id)
        .await
        .expect("graph with configured Tasks loads");
    let batch = fixture
        .service
        .authorize_batch(
            &fixture.project_id,
            &fixture.owner_id,
            graph.version,
            vec![first.clone(), second.clone()],
            1,
            "single-task-capacity-batch",
        )
        .await
        .expect("owner authorizes both Tasks with one concurrent Task slot");
    assert_eq!(batch.state, "running");
    assert_eq!(batch.max_parallel_tasks, 1);

    assert_eq!(
        fixture
            .service
            .check_once()
            .await
            .expect("scheduler admits only within the batch cap"),
        1,
    );
    let rows = sqlx::query(
        "SELECT task_id,state,blocker FROM project_plan_batch_task WHERE batch_id=? ORDER BY task_id",
    )
    .bind(&batch.id)
    .fetch_all(fixture.db.pool())
    .await
    .expect("batch admission states load");
    assert_eq!(rows.len(), 2);
    let mut started = 0;
    let mut capacity_waiting = 0;
    for row in rows {
        let state: String = sqlx::Row::try_get(&row, "state").expect("Task batch state");
        let blocker: Option<String> =
            sqlx::Row::try_get(&row, "blocker").expect("Task batch blocker");
        match state.as_str() {
            "started" => started += 1,
            "pending" if blocker.as_deref() == Some("capacity:project_parallel_limit") => {
                capacity_waiting += 1;
            }
            other => panic!("unexpected batch Task state {other:?} ({blocker:?})"),
        }
    }
    assert_eq!(
        started, 1,
        "one exact coordinator request occupies the slot"
    );
    assert_eq!(
        capacity_waiting, 1,
        "the other independent Task waits for capacity"
    );

    let requests: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task_coordination_request WHERE task_id IN (?,?)")
            .bind(&first)
            .bind(&second)
            .fetch_one(fixture.db.pool())
            .await
            .expect("coordination request count loads");
    assert_eq!(
        requests, 1,
        "the scheduler requested one Task turn, not two"
    );
    let executions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE task_id IN (?,?)")
            .bind(&first)
            .bind(&second)
            .fetch_one(fixture.db.pool())
            .await
            .expect("Execution count loads");
    assert_eq!(
        executions, 0,
        "a coordinator request is not a provider start"
    );

    assert_eq!(
        fixture
            .service
            .check_once()
            .await
            .expect("repeated scheduler tick preserves the cap"),
        0,
    );
    let requests_after_retry: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task_coordination_request WHERE task_id IN (?,?)")
            .bind(&first)
            .bind(&second)
            .fetch_one(fixture.db.pool())
            .await
            .expect("coordination request count reloads");
    assert_eq!(
        requests_after_retry, requests,
        "capacity waiting does not duplicate a turn"
    );
}

#[tokio::test]
async fn batch_request_replay_is_exact_and_does_not_charge_budget_or_start_runtime() {
    let fixture = fixture().await;
    let task_id = seed_task(&fixture.db, &fixture.project_id, "Batch request replay").await;
    let agent_id = new_uuid_v4();
    seed_agent_coordinator(&fixture, &task_id, &agent_id).await;
    let graph = fixture
        .service
        .graph(&fixture.project_id)
        .await
        .expect("graph with a configured coordinator loads");
    let batch = fixture
        .service
        .authorize_batch(
            &fixture.project_id,
            &fixture.owner_id,
            graph.version,
            vec![task_id.clone()],
            1,
            "batch-request-replay",
        )
        .await
        .expect("owner authorizes and prepares the batch");
    assert_eq!(
        fixture
            .service
            .check_once()
            .await
            .expect("scheduler claims one Task"),
        1
    );

    let request_key = format!("project-plan-batch:{}:turn:{}", batch.id, task_id);
    let request = TaskCoordinationRepo::get_task_coordination_request_by_key(
        &*fixture.db,
        &task_id,
        &request_key,
    )
    .await
    .expect("exact batch request lookup succeeds")
    .expect("scheduler creates one request");
    let scope = sqlx::query(
        "SELECT project_id,batch_id,authorized_by,expected_batch_version,
                expected_graph_version,expected_task_version,authorization_version,
                coordination_snapshot_digest,claim_token,origin_kind
         FROM project_plan_batch_coordination_request WHERE request_id=?",
    )
    .bind(&request.id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("request has an exact durable origin");
    let origin_kind: String = sqlx::Row::try_get(&scope, "origin_kind").expect("origin kind");
    assert_eq!(origin_kind, "scheduler");
    let binding = ProjectPlanBatchRequestBinding {
        project_id: sqlx::Row::try_get(&scope, "project_id").expect("Project ID"),
        batch_id: sqlx::Row::try_get(&scope, "batch_id").expect("batch ID"),
        authorized_by: sqlx::Row::try_get(&scope, "authorized_by").expect("authorizer"),
        expected_batch_version: sqlx::Row::try_get(&scope, "expected_batch_version")
            .expect("batch version"),
        expected_graph_version: sqlx::Row::try_get(&scope, "expected_graph_version")
            .expect("graph version"),
        expected_task_version: sqlx::Row::try_get(&scope, "expected_task_version")
            .expect("Task version"),
        expected_coordination_version: sqlx::Row::try_get(&scope, "authorization_version")
            .expect("authorization version"),
        coordination_snapshot_digest: sqlx::Row::try_get(&scope, "coordination_snapshot_digest")
            .expect("coordination snapshot digest"),
        origin: ProjectPlanBatchRequestOrigin::Scheduler {
            claim_token: sqlx::Row::try_get::<Option<String>, _>(&scope, "claim_token")
                .expect("claim token column")
                .expect("scheduler origin keeps the exact claim token"),
        },
    };
    let brief: crate::task_coordination::CoordinationBrief =
        serde_json::from_str(&request.brief_json).expect("persisted brief parses");
    let coordination_service = crate::task_coordination::TaskCoordinationService::new(
        Arc::clone(&fixture.db),
        Arc::clone(&fixture.event_bus),
    );
    let turns_before: i64 =
        sqlx::query_scalar("SELECT turn_count FROM task_coordination WHERE task_id=?")
            .bind(&task_id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("turn budget loads before replay");
    let (replayed, created) = coordination_service
        .request_project_batch_turn(
            &task_id,
            &fixture.owner_id,
            crate::task_coordination::TurnRequestInput {
                expected_task_version: binding.expected_task_version,
                expected_coordination_version: binding.expected_coordination_version,
                brief: brief.clone(),
                idempotency_key: request_key.clone(),
            },
            binding.clone(),
        )
        .await
        .expect("the exact scoped request replays");
    assert_eq!(replayed.id, request.id);
    assert!(!created, "an exact replay creates no second request");

    let generic_replay = coordination_service
        .request_turn(
            &task_id,
            &fixture.owner_id,
            crate::task_coordination::TurnRequestInput {
                expected_task_version: binding.expected_task_version,
                expected_coordination_version: binding.expected_coordination_version,
                brief,
                idempotency_key: request_key,
            },
        )
        .await;
    assert!(matches!(
        generic_replay,
        Err(ServiceError::Db(db::DbError::ProjectPlanBatchInactive))
    ));

    assert_eq!(
        fixture
            .service
            .check_once()
            .await
            .expect("repeated tick is idle"),
        0
    );
    let turns_after: i64 =
        sqlx::query_scalar("SELECT turn_count FROM task_coordination WHERE task_id=?")
            .bind(&task_id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("turn budget loads after replay");
    assert_eq!(
        turns_after, turns_before,
        "replay charges no additional turn"
    );
    let scoped_requests: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM project_plan_batch_coordination_request
         WHERE batch_id=? AND task_id=?",
    )
    .bind(&batch.id)
    .bind(&task_id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("batch request origins count");
    assert_eq!(scoped_requests, 1);
    let (wakes, executions, materializations): (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM orchestrator_wake WHERE task_id=?),
                (SELECT COUNT(*) FROM execution WHERE task_id=?),
                (SELECT COUNT(*) FROM task_coordination_materialization WHERE task_id=?)",
    )
    .bind(&task_id)
    .bind(&task_id)
    .bind(&task_id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("replay remains before wake, provider, and materialization");
    assert_eq!((wakes, executions, materializations), (0, 0, 0));
}

#[tokio::test]
async fn batch_budget_exhaustion_retries_only_after_an_owner_grants_more_budget() {
    let fixture = fixture().await;
    let task_id = seed_task(&fixture.db, &fixture.project_id, "Exhausted batch budget").await;
    let agent_id = new_uuid_v4();
    seed_agent_coordinator(&fixture, &task_id, &agent_id).await;
    let coordination = TaskCoordinationRepo::get_task_coordination(&*fixture.db, &task_id)
        .await
        .expect("coordination lookup succeeds")
        .expect("Task coordination is authorized");
    let task = TaskRepo::get_by_id(&*fixture.db, &task_id, false)
        .await
        .expect("Task lookup succeeds")
        .expect("Task exists");
    let coordination_service = crate::task_coordination::TaskCoordinationService::new(
        Arc::clone(&fixture.db),
        Arc::clone(&fixture.event_bus),
    );
    for index in 0..coordination.max_turns {
        coordination_service
            .request_turn(
                &task_id,
                &fixture.owner_id,
                crate::task_coordination::TurnRequestInput {
                    expected_task_version: task.version,
                    expected_coordination_version: coordination.authorization_version,
                    brief: crate::task_coordination::CoordinationBrief {
                        objective: format!("Consume pre-batch turn {index}"),
                        constraints: None,
                        acceptance_criteria: Vec::new(),
                        context: None,
                    },
                    idempotency_key: format!("pre-batch-budget-turn-{index}"),
                },
            )
            .await
            .expect("a turn within the configured budget commits");
    }
    let graph = fixture
        .service
        .graph(&fixture.project_id)
        .await
        .expect("graph loads after the budget is consumed");
    let batch = fixture
        .service
        .authorize_batch(
            &fixture.project_id,
            &fixture.owner_id,
            graph.version,
            vec![task_id.clone()],
            1,
            "exhausted-budget-batch",
        )
        .await
        .expect("batch authorization is distinct from its turn budget");
    assert_eq!(
        fixture
            .service
            .check_once()
            .await
            .expect("scheduler blocks at the exhausted budget"),
        0
    );

    let state: (String, Option<String>) = sqlx::query_as(
        "SELECT state,blocker FROM project_plan_batch_task WHERE batch_id=? AND task_id=?",
    )
    .bind(&batch.id)
    .bind(&task_id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("blocked batch row remains inspectable");
    assert_eq!(state.0, "blocked");
    assert_eq!(
        state.1.as_deref(),
        Some("coordination:coordination.budget_exhausted")
    );
    let (turns, batch_requests, executions, wakes, materializations): (i64, i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT (SELECT turn_count FROM task_coordination WHERE task_id=?),
                    (SELECT COUNT(*) FROM project_plan_batch_coordination_request
                     WHERE batch_id=? AND task_id=?),
                    (SELECT COUNT(*) FROM execution WHERE task_id=?),
                    (SELECT COUNT(*) FROM orchestrator_wake WHERE task_id=?),
                    (SELECT COUNT(*) FROM task_coordination_materialization WHERE task_id=?)",
        )
        .bind(&task_id)
        .bind(&batch.id)
        .bind(&task_id)
        .bind(&task_id)
        .bind(&task_id)
        .bind(&task_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("budget and downstream effects remain inspectable");
    assert_eq!(turns, coordination.max_turns);
    assert_eq!(
        (batch_requests, executions, wakes, materializations),
        (0, 0, 0, 0)
    );

    let autonomy = crate::task_autonomy::TaskAutonomyService::new(
        Arc::clone(&fixture.db),
        Arc::clone(&fixture.event_bus),
    );
    let current_limits = autonomy
        .get(&task_id, &fixture.owner_id)
        .await
        .expect("Task owner reads autonomy limits");
    let coordination_before_budget_grant =
        TaskCoordinationRepo::get_task_coordination(&*fixture.db, &task_id)
            .await
            .expect("batch coordination reloads before budget grant")
            .expect("batch authorization remains current before budget grant");
    assert_eq!(current_limits.max_turns, turns);
    let granted_limits = autonomy
        .update(
            &task_id,
            &fixture.owner_id,
            api_types::UpdateTaskAutonomyLimitsRequest {
                expected_version: current_limits.version,
                limits: api_types::TaskAutonomyLimits {
                    version: current_limits.version,
                    max_turns: current_limits.max_turns + 1,
                    ..current_limits.clone()
                },
                reason: "Resume the exact Project batch after explicit budget approval".to_owned(),
                idempotency_key: "project-batch-budget-retry-grant".to_owned(),
            },
        )
        .await
        .expect("Project owner explicitly raises the Task turn budget");
    assert_eq!(granted_limits.max_turns, turns + 1);
    let coordination_after_budget_grant =
        TaskCoordinationRepo::get_task_coordination(&*fixture.db, &task_id)
            .await
            .expect("batch coordination reloads after budget grant")
            .expect("batch authorization remains current after budget grant");
    assert_eq!(
        coordination_after_budget_grant.authorization_version,
        coordination_before_budget_grant.authorization_version
    );
    let budget_event: String = sqlx::query_scalar(
        "SELECT payload_json FROM domain_event
         WHERE event_type='task.autonomy_limits_changed' AND entity_id=?
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(&task_id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("budget grant event payload loads");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&budget_event)
            .expect("budget grant event payload is valid")
            .get("authorization_version")
            .and_then(serde_json::Value::as_i64),
        Some(coordination_before_budget_grant.authorization_version)
    );
    assert_eq!(
        fixture
            .service
            .check_once()
            .await
            .expect("scheduler retries the exact budget-blocked batch Task"),
        1
    );

    let (state, blocker, request_id): (String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT state,blocker,coordination_request_id FROM project_plan_batch_task
         WHERE batch_id=? AND task_id=?",
    )
    .bind(&batch.id)
    .bind(&task_id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("retried batch Task state loads");
    assert_eq!(state, "started");
    assert_eq!(blocker, None);
    let request_id = request_id.expect("the exact batch Task now has a coordination request");
    let (origin, claim_token, linked_task, authorization_version): (
        String,
        Option<String>,
        String,
        i64,
    ) = sqlx::query_as(
        "SELECT scope.origin_kind,scope.claim_token,scope.task_id,scope.authorization_version
         FROM project_plan_batch_coordination_request scope
         WHERE scope.request_id=? AND scope.batch_id=?",
    )
    .bind(&request_id)
    .bind(&batch.id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("requeued request remains bound to its exact batch scope");
    assert_eq!(origin, "scheduler");
    assert!(claim_token.is_some_and(|token| !token.is_empty()));
    assert_eq!(linked_task, task_id);
    assert_eq!(
        authorization_version,
        coordination.authorization_version + 1
    );
    let (turns, batch_requests, executions, wakes, materializations): (i64, i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT (SELECT turn_count FROM task_coordination WHERE task_id=?),
                (SELECT COUNT(*) FROM project_plan_batch_coordination_request
                 WHERE batch_id=? AND task_id=?),
                (SELECT COUNT(*) FROM execution WHERE task_id=?),
                (SELECT COUNT(*) FROM orchestrator_wake WHERE task_id=?),
                (SELECT COUNT(*) FROM task_coordination_materialization WHERE task_id=?)",
        )
        .bind(&task_id)
        .bind(&batch.id)
        .bind(&task_id)
        .bind(&task_id)
        .bind(&task_id)
        .bind(&task_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("only the newly authorized coordination turn is charged");
    assert_eq!(turns, granted_limits.max_turns);
    assert_eq!(
        (batch_requests, executions, wakes, materializations),
        (1, 0, 0, 0)
    );
}

#[tokio::test]
async fn autonomy_wake_admission_rechecks_authorization_and_batch_origin_atomically() {
    let fixture = fixture().await;
    let task_id = seed_task(&fixture.db, &fixture.project_id, "Autonomy wake fence").await;
    let agent_id = new_uuid_v4();
    seed_agent_coordinator(&fixture, &task_id, &agent_id).await;
    let coordination = TaskCoordinationRepo::get_task_coordination(&*fixture.db, &task_id)
        .await
        .expect("initial coordination lookup succeeds")
        .expect("TaskCoordination is authorized");
    let role_id = coordination
        .coordinator_task_role_id
        .clone()
        .expect("coordinator role is exact");
    let role = TaskRoleRepo::get_by_id(&*fixture.db, &role_id)
        .await
        .expect("coordinator TaskRole lookup succeeds")
        .expect("coordinator TaskRole exists");
    let autonomy = crate::task_autonomy::TaskAutonomyService::new(
        Arc::clone(&fixture.db),
        Arc::clone(&fixture.event_bus),
    );
    let limits = autonomy
        .get(&task_id, &fixture.owner_id)
        .await
        .expect("Project owner reads autonomy limits");
    autonomy
        .update(
            &task_id,
            &fixture.owner_id,
            api_types::UpdateTaskAutonomyLimitsRequest {
                expected_version: limits.version,
                limits: limits.clone(),
                reason: "Persist a pre-batch autonomy event for the wake fence".to_owned(),
                idempotency_key: "autonomy-wake-before-batch".to_owned(),
            },
        )
        .await
        .expect("Project owner persists an autonomy event before the batch grant");
    let stale_event = sqlx::query(
        "SELECT id,sequence,payload_json FROM domain_event
         WHERE event_type='task.autonomy_limits_changed' AND entity_id=?
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(&task_id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("pre-batch autonomy event loads");
    let stale_event_id: String = stale_event.try_get("id").expect("event ID reads");
    let stale_event_sequence: i64 = stale_event
        .try_get("sequence")
        .expect("event sequence reads");
    let stale_event_payload: String = stale_event
        .try_get("payload_json")
        .expect("event payload reads");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&stale_event_payload)
            .expect("autonomy event payload is valid")
            .get("authorization_version")
            .and_then(serde_json::Value::as_i64),
        Some(coordination.authorization_version)
    );

    let graph = fixture
        .service
        .graph(&fixture.project_id)
        .await
        .expect("Project graph loads before batch authorization");
    let batch = fixture
        .service
        .authorize_batch(
            &fixture.project_id,
            &fixture.owner_id,
            graph.version,
            vec![task_id.clone()],
            1,
            "autonomy-wake-batch-fence",
        )
        .await
        .expect("Project owner authorizes a batch for the exact Task");
    assert_eq!(batch.state, "running");
    let batch_coordination = TaskCoordinationRepo::get_task_coordination(&*fixture.db, &task_id)
        .await
        .expect("batch coordination lookup succeeds")
        .expect("batch TaskCoordination exists");
    assert_eq!(
        batch_coordination.authorization_version,
        coordination.authorization_version + 1
    );

    let make_wake = |event_id: String, event_sequence: i64| CreateOrchestratorWake {
        id: new_uuid_v4(),
        event_id: event_id.clone(),
        event_sequence,
        task_id: task_id.clone(),
        task_role_id: role.id.clone(),
        coordination_mode: role.coordination_mode,
        actor_kind: ActorKind::Agent,
        actor_id: agent_id.clone(),
        work_unit_id: None,
        correlation_id: event_id.clone(),
        causation_id: Some(event_id),
        causation_depth: 0,
        policy_ref: "test-policy".to_owned(),
        policy_version: 1,
        policy_digest: "test-digest".to_owned(),
        task_role_version: role.version,
        task_role_policy_json: role.policy_json.clone(),
        coordination_request_id: None,
        available_at: now_rfc3339(),
        created_at: now_rfc3339(),
        updated_at: now_rfc3339(),
    };
    assert!(
        !OrchestratorWakeRepo::admit_task_autonomy_orchestrator_wake(
            &*fixture.db,
            make_wake(stale_event_id.clone(), stale_event_sequence),
            coordination.authorization_version,
        )
        .await
        .expect("stale autonomy event is rejected under the DB writer lock")
    );

    let current_limits = autonomy
        .get(&task_id, &fixture.owner_id)
        .await
        .expect("Project owner reloads autonomy limits");
    autonomy
        .update(
            &task_id,
            &fixture.owner_id,
            api_types::UpdateTaskAutonomyLimitsRequest {
                expected_version: current_limits.version,
                limits: api_types::TaskAutonomyLimits {
                    version: current_limits.version,
                    max_turns: current_limits.max_turns + 1,
                    ..current_limits
                },
                reason: "Persist an autonomy event under the batch authorization".to_owned(),
                idempotency_key: "autonomy-wake-during-batch".to_owned(),
            },
        )
        .await
        .expect("Project owner persists a budget event under the current batch authorization");
    let batch_event = sqlx::query(
        "SELECT id,sequence,payload_json FROM domain_event
         WHERE event_type='task.autonomy_limits_changed' AND entity_id=?
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(&task_id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("batch-scoped autonomy event loads");
    let batch_event_id: String = batch_event.try_get("id").expect("event ID reads");
    let batch_event_sequence: i64 = batch_event
        .try_get("sequence")
        .expect("event sequence reads");
    let batch_event_payload: String = batch_event
        .try_get("payload_json")
        .expect("event payload reads");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&batch_event_payload)
            .expect("batch autonomy event payload is valid")
            .get("authorization_version")
            .and_then(serde_json::Value::as_i64),
        Some(batch_coordination.authorization_version)
    );
    assert!(
        !OrchestratorWakeRepo::admit_task_autonomy_orchestrator_wake(
            &*fixture.db,
            make_wake(batch_event_id.clone(), batch_event_sequence),
            batch_coordination.authorization_version,
        )
        .await
        .expect("batch-scoped autonomy event cannot create a generic wake")
    );

    let wake_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM orchestrator_wake WHERE event_id IN (?,?)")
            .bind(&stale_event_id)
            .bind(&batch_event_id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("both fenced event wake counts load");
    assert_eq!(wake_count, 0);
}

#[tokio::test]
async fn revocation_after_batch_claim_rejects_request_before_budget_or_wake() {
    let fixture = fixture().await;
    let task_id = seed_task(&fixture.db, &fixture.project_id, "Claim revocation barrier").await;
    let agent_id = new_uuid_v4();
    seed_agent_coordinator(&fixture, &task_id, &agent_id).await;
    let graph = fixture
        .service
        .graph(&fixture.project_id)
        .await
        .expect("graph loads with a configured coordinator");
    let batch = fixture
        .service
        .authorize_batch(
            &fixture.project_id,
            &fixture.owner_id,
            graph.version,
            vec![task_id.clone()],
            1,
            "claim-revocation-barrier",
        )
        .await
        .expect("owner authorizes and prepares the batch");
    let revoke_on_claim = format!(
        "CREATE TRIGGER test_revoke_project_batch_after_claim
         AFTER UPDATE OF state ON project_plan_batch_task
         WHEN NEW.batch_id='{}' AND NEW.state='claimed'
         BEGIN
           UPDATE project_plan_batch SET state='revoked',version=version+1,
             updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE id=NEW.batch_id AND state='running';
           UPDATE project_plan_batch_task SET state='blocked',claim_expires_at=NULL,
             claim_token=NULL,blocker='batch:revoked'
             WHERE batch_id=NEW.batch_id AND task_id=NEW.task_id
               AND state='claimed' AND claim_token=NEW.claim_token;
         END",
        batch.id
    );
    sqlx::query(&revoke_on_claim)
        .execute(fixture.db.pool())
        .await
        .expect("test interleaving trigger installs");

    assert_eq!(
        fixture
            .service
            .check_once()
            .await
            .expect("revoked claim cannot create a coordinator request"),
        0
    );
    let state: (String, String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT b.state,bt.state,bt.blocker,bt.claim_token
         FROM project_plan_batch b JOIN project_plan_batch_task bt ON bt.batch_id=b.id
         WHERE b.id=? AND bt.task_id=?",
    )
    .bind(&batch.id)
    .bind(&task_id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("revoked batch claim remains inspectable");
    assert_eq!(
        state,
        (
            "revoked".to_owned(),
            "blocked".to_owned(),
            Some("batch:revoked".to_owned()),
            None
        )
    );
    let (turns, requests, scope_rows, request_events, wakes, executions, materializations):
        (i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT turn_count FROM task_coordination WHERE task_id=?),
                    (SELECT COUNT(*) FROM task_coordination_request WHERE task_id=?),
                    (SELECT COUNT(*) FROM project_plan_batch_coordination_request
                     WHERE batch_id=? AND task_id=?),
                    (SELECT COUNT(*) FROM domain_event WHERE event_type='task.coordination_requested'
                     AND scope_type='task' AND scope_id=?),
                    (SELECT COUNT(*) FROM orchestrator_wake WHERE task_id=?),
                    (SELECT COUNT(*) FROM execution WHERE task_id=?),
                    (SELECT COUNT(*) FROM task_coordination_materialization WHERE task_id=?)",
        )
        .bind(&task_id)
        .bind(&task_id)
        .bind(&batch.id)
        .bind(&task_id)
        .bind(&task_id)
        .bind(&task_id)
        .bind(&task_id)
        .bind(&task_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("rejected claim leaves no budget, request, event, wake, provider, or materialization");
    assert_eq!(
        (
            turns,
            requests,
            scope_rows,
            request_events,
            wakes,
            executions,
            materializations
        ),
        (0, 0, 0, 0, 0, 0, 0)
    );
}

#[tokio::test]
async fn authorized_successor_waits_for_accepted_git_handoff_and_provider_start_fails_closed_on_a_diverged_base(
) {
    let fixture = fixture().await;
    let git_root = TempDir::new().expect("temporary Git repository root creates");
    let (repo_path, initial_sha, integrated_sha, successor_base_sha, stale_base_sha) =
        create_git_history(git_root.path());
    let repo_id = new_uuid_v4();
    let now = now_rfc3339();
    RepoRepo::create(
        &*fixture.db,
        CreateRepo {
            id: repo_id.clone(),
            project_id: fixture.project_id.clone(),
            name: "Handoff integration fixture".to_owned(),
            remote_url: repo_path.to_string_lossy().into_owned(),
            local_path: Some(repo_path.to_string_lossy().into_owned()),
            work_mode: WorkMode::DirectMerge,
            default_branch: "main".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("local Git repository binds to the Project");

    let predecessor = seed_repo_task(
        &fixture.db,
        &fixture.project_id,
        &repo_id,
        "Accepted predecessor",
        TaskLifecycleState::Done,
    )
    .await;
    let successor = seed_repo_task(
        &fixture.db,
        &fixture.project_id,
        &repo_id,
        "Successor waiting on accepted handoff",
        TaskLifecycleState::Backlog,
    )
    .await;
    TaskDependencyRepo::add_dependency(&*fixture.db, &successor, &predecessor, &now_rfc3339())
        .await
        .expect("successor has its canonical predecessor");

    let agent_id = new_uuid_v4();
    seed_agent(&fixture, &agent_id).await;
    let implementer_role_id = seed_implementer_role(&fixture.db, &successor).await;
    RoleMembershipRepo::add(
        &*fixture.db,
        CreateRoleMembership {
            id: new_uuid_v4(),
            task_role_id: implementer_role_id,
            actor_kind: ActorKind::Agent,
            actor_id: agent_id.clone(),
            status: RoleMembershipStatus::Active,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("successor implementer membership exists before batch authorization");
    seed_agent_coordinator(&fixture, &successor, &agent_id).await;

    let predecessor_role_id = seed_implementer_role(&fixture.db, &predecessor).await;
    RoleMembershipRepo::add(
        &*fixture.db,
        CreateRoleMembership {
            id: new_uuid_v4(),
            task_role_id: predecessor_role_id,
            actor_kind: ActorKind::Agent,
            actor_id: agent_id.clone(),
            status: RoleMembershipStatus::Active,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("predecessor result has an exact active implementer Actor");

    let predecessor_work_unit = seed_repository_work_unit(
        &fixture.db,
        &predecessor,
        &fixture.owner_id,
        "Accepted repository result",
        Some(ActorRef::Agent(agent_id.clone())),
    )
    .await;
    let source_branch = "project-plan-predecessor-result";
    let source_path = git_root.path().join("predecessor-worktree");
    add_git_worktree(&repo_path, &source_path, source_branch, &integrated_sha);
    let source_workspace = seed_work_unit_workspace(
        &fixture.db,
        &predecessor,
        &predecessor_work_unit,
        &repo_id,
        &source_path,
        source_branch,
        &initial_sha,
    )
    .await;
    let integration_workspace = WorkspaceRepo::create(
        &*fixture.db,
        CreateWorkspace {
            id: new_uuid_v4(),
            task_id: predecessor.clone(),
            repo_id: repo_id.clone(),
            worktree_path: repo_path.to_string_lossy().into_owned(),
            branch: "main".to_owned(),
            status: WorkspaceStatus::Ready,
            before_sha: Some(initial_sha.clone()),
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("Task integration Workspace is persisted");

    let execution_id = new_uuid_v4();
    ExecutionRepo::create(
        &*fixture.db,
        db::CreateExecution {
            id: execution_id.clone(),
            task_id: predecessor.clone(),
            agent_id: Some(agent_id.clone()),
            actor_ref: Some(ActorRef::Agent(agent_id.clone())),
            role: "implementer".to_owned(),
            status: ExecutionStatus::Completed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            harness_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("Persisted result for accepted integration fixture".to_owned()),
            logs_path: None,
            before_sha: Some(initial_sha.clone()),
            after_sha: Some(integrated_sha.clone()),
            error: None,
            executor_config_snapshot_json: None,
            workspace_id: Some(source_workspace.id.clone()),
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("completed predecessor result is persisted");
    sqlx::query("UPDATE execution SET work_unit_id=?,work_unit_version=1 WHERE id=?")
        .bind(&predecessor_work_unit)
        .bind(&execution_id)
        .execute(fixture.db.pool())
        .await
        .expect("fixture binds the completed result to its exact WorkUnit revision");
    WorkUnitRepo::transition(
        &*fixture.db,
        TransitionWorkUnit {
            id: predecessor_work_unit.clone(),
            expected_version: 1,
            status: WorkUnitStatus::Completed,
            updated_at: now_rfc3339(),
            reviewed_result: None,
        },
        domain_event(
            "work_unit.completed",
            "work_unit",
            &predecessor_work_unit,
            &predecessor,
            &fixture.owner_id,
        ),
    )
    .await
    .expect("exact completed WorkUnit result is accepted");
    let integration_id = new_uuid_v4();
    let integration = WorkUnitRepo::begin_integration(
        &*fixture.db,
        CreateWorkUnitIntegration {
            id: integration_id.clone(),
            task_id: predecessor.clone(),
            work_unit_id: predecessor_work_unit.clone(),
            execution_id: execution_id.clone(),
            source_workspace_id: source_workspace.id.clone(),
            source_branch: source_branch.to_owned(),
            source_sha: integrated_sha.clone(),
            target_workspace_id: integration_workspace.id,
            target_branch: "main".to_owned(),
            target_before_sha: initial_sha,
            operation_idempotency_key: "accepted-predecessor-integration".to_owned(),
            started_at: now_rfc3339(),
            created_at: now_rfc3339(),
        },
        domain_event(
            "work_unit.integration_started",
            "work_unit_integration",
            &integration_id,
            &predecessor,
            &fixture.owner_id,
        ),
    )
    .await
    .expect("successful integration is tied to the exact completed result");
    let prerequisite_satisfied: bool = sqlx::query_scalar(
        "SELECT satisfied FROM project_task_dependency_readiness WHERE task_id=? AND depends_on_id=?",
    )
    .bind(&successor)
    .bind(&predecessor)
    .fetch_one(fixture.db.pool())
    .await
    .expect("canonical delivery readiness is projected");
    assert!(
        !prerequisite_satisfied,
        "a completed predecessor without accepted integration does not satisfy the edge"
    );

    let graph = fixture
        .service
        .graph(&fixture.project_id)
        .await
        .expect("graph with accepted predecessor loads");
    let batch = fixture
        .service
        .authorize_batch(
            &fixture.project_id,
            &fixture.owner_id,
            graph.version,
            vec![successor.clone()],
            1,
            "accepted-handoff-batch",
        )
        .await
        .expect("owner authorizes the dependent Task");
    assert_eq!(batch.state, "running");
    assert_eq!(
        fixture
            .service
            .check_once()
            .await
            .expect("scheduler waits for accepted delivery"),
        0,
        "the already authorized successor waits while predecessor integration is pending"
    );
    let waiting_state: (String, Option<String>) = sqlx::query_as(
        "SELECT state,blocker FROM project_plan_batch_task WHERE batch_id=? AND task_id=?",
    )
    .bind(&batch.id)
    .bind(&successor)
    .fetch_one(fixture.db.pool())
    .await
    .expect("waiting successor state loads");
    assert_eq!(waiting_state.0, "blocked");
    assert_eq!(
        waiting_state.1.as_deref(),
        Some(format!("dependency:{predecessor}").as_str())
    );

    WorkUnitRepo::record_integration(
        &*fixture.db,
        RecordWorkUnitIntegration {
            id: integration_id,
            expected_version: integration.record.version,
            outcome: WorkUnitIntegrationOutcome::Success,
            target_after_sha: Some(integrated_sha.clone()),
            conflict_metadata_json: None,
            finished_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
        domain_event(
            "work_unit.integration_succeeded",
            "work_unit_integration",
            &integration.record.id,
            &predecessor,
            &fixture.owner_id,
        ),
    )
    .await
    .expect("the exact integration is durably successful");
    let prerequisite_satisfied: bool = sqlx::query_scalar(
        "SELECT satisfied FROM project_task_dependency_readiness WHERE task_id=? AND depends_on_id=?",
    )
    .bind(&successor)
    .bind(&predecessor)
    .fetch_one(fixture.db.pool())
    .await
    .expect("accepted delivery readiness reloads");
    assert!(
        prerequisite_satisfied,
        "completion plus exact successful integration satisfies the edge"
    );
    assert_eq!(
        fixture
            .service
            .check_once()
            .await
            .expect("scheduler requests the ready successor"),
        1
    );
    let batch_task_state: (String, Option<String>) = sqlx::query_as(
        "SELECT state,blocker FROM project_plan_batch_task WHERE batch_id=? AND task_id=?",
    )
    .bind(&batch.id)
    .bind(&successor)
    .fetch_one(fixture.db.pool())
    .await
    .expect("successor batch state loads");
    assert_eq!(batch_task_state.0, "started");
    assert_eq!(batch_task_state.1, None);

    let persisted_context: String = sqlx::query_scalar(
        "SELECT context_json FROM project_plan_handoff WHERE batch_id=? AND task_id=? AND predecessor_task_id=?",
    )
    .bind(&batch.id)
    .bind(&successor)
    .bind(&predecessor)
    .fetch_one(fixture.db.pool())
    .await
    .expect("scheduler pins the producer's persisted per-predecessor handoff");
    let persisted_context: serde_json::Value =
        serde_json::from_str(&persisted_context).expect("handoff context is valid JSON");
    assert_eq!(
        persisted_context["work_units"][0]["integration"]["commit_sha"], integrated_sha,
        "the row persists the exact accepted integration commit"
    );

    let positive_work_unit = seed_repository_work_unit(
        &fixture.db,
        &successor,
        &fixture.owner_id,
        "Successor with an up-to-date base",
        Some(ActorRef::Agent(agent_id.clone())),
    )
    .await;
    let positive_branch = "project-plan-successor-current";
    let positive_path = git_root.path().join("successor-current-worktree");
    add_git_worktree(
        &repo_path,
        &positive_path,
        positive_branch,
        &successor_base_sha,
    );
    let positive_workspace = seed_work_unit_workspace(
        &fixture.db,
        &successor,
        &positive_work_unit,
        &repo_id,
        &positive_path,
        positive_branch,
        &successor_base_sha,
    )
    .await;
    fixture
        .service
        .verify_handoff_workspace_base(&successor, &positive_work_unit, Some(&positive_workspace))
        .await
        .expect("a real Git worktree descending from the accepted commit passes the handoff guard");

    let negative_work_unit = seed_repository_work_unit(
        &fixture.db,
        &successor,
        &fixture.owner_id,
        "Successor with a diverged base",
        Some(ActorRef::Agent(agent_id.clone())),
    )
    .await;
    let negative_branch = "project-plan-successor-diverged";
    let negative_path = git_root.path().join("successor-diverged-worktree");
    add_git_worktree(&repo_path, &negative_path, negative_branch, &stale_base_sha);
    let negative_workspace = seed_work_unit_workspace(
        &fixture.db,
        &successor,
        &negative_work_unit,
        &repo_id,
        &negative_path,
        negative_branch,
        &stale_base_sha,
    )
    .await;

    let task = TaskRepo::get_by_id(&*fixture.db, &successor, false)
        .await
        .expect("successor Task loads")
        .expect("successor Task exists");
    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &successor)
        .await
        .expect("successor lifecycle loads")
        .expect("successor lifecycle exists");
    let lifecycle_service = crate::task_lifecycle::TaskLifecycleService::new(
        Arc::clone(&fixture.db),
        Arc::clone(&fixture.event_bus),
    );
    let lifecycle_transitions = lifecycle_service
        .plan_execution_activation(&task, &lifecycle)
        .await
        .expect("normal execution activation path is planned");
    let agent = AgentRepo::get_by_id(&*fixture.db, &agent_id)
        .await
        .expect("successor Agent loads")
        .expect("successor Agent exists");
    let executor_snapshot = crate::task_service::config::build_executor_config_snapshot(
        fixture.db.as_ref(),
        &task,
        &agent,
        None,
        None,
    )
    .await
    .expect("exact executor configuration snapshot builds")
    .expect("Agent has a frozen executor configuration");
    let task_service = crate::task_service::TaskService::new(
        Arc::clone(&fixture.db),
        Arc::clone(&fixture.event_bus),
    );
    let now = now_rfc3339();
    let (_, execution, _) = task_service
        .admit_running_work_unit_execution_with_artifact_inputs(
            &negative_work_unit,
            1,
            db::CreateExecution {
                id: new_uuid_v4(),
                task_id: successor.clone(),
                agent_id: Some(agent_id.clone()),
                actor_ref: Some(ActorRef::Agent(agent_id.clone())),
                role: "implementer".to_owned(),
                status: ExecutionStatus::Running,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                parent_execution_id: None,
                harness_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: Some("Exercise the durable ProviderStart ancestry guard".to_owned()),
                logs_path: None,
                before_sha: Some(stale_base_sha.clone()),
                after_sha: None,
                error: None,
                executor_config_snapshot_json: Some(executor_snapshot),
                workspace_id: Some(negative_workspace.id.clone()),
                created_at: now.clone(),
                updated_at: now,
            },
            Vec::new(),
            crate::task_service::WorkUnitAdmissionLifecyclePlan {
                expected_task_version: task.version,
                lifecycle,
                transitions: lifecycle_transitions,
                clear_blocking_metadata: None,
                follow_up_receipt: None,
                automatic_recovery_parent_id: None,
            },
        )
        .await
        .expect("exact WorkUnit admission records a Running Execution and lease without dispatch");

    let start = task_service.start_execution(&execution.id).await;
    assert!(matches!(
        start,
        Err(ServiceError::Db(db::DbError::ProjectDependencyUnsatisfied))
    ));
    let terminal_execution = ExecutionRepo::get_by_id(&*fixture.db, &execution.id)
        .await
        .expect("Execution status reloads")
        .expect("Execution remains auditable");
    assert_eq!(terminal_execution.status, ExecutionStatus::Failed);
    let provider_attempts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event WHERE entity_type='execution' AND entity_id=? AND event_type='execution.provider_start_attempted'",
    )
    .bind(&execution.id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("provider Start event count loads");
    assert_eq!(
        provider_attempts, 0,
        "the ancestry guard stops dispatch before ProviderStart"
    );

    let paused_batch: (String, String, Option<String>) = sqlx::query_as(
        "SELECT b.state,bt.state,bt.blocker FROM project_plan_batch b JOIN project_plan_batch_task bt ON bt.batch_id=b.id WHERE b.id=? AND bt.task_id=?",
    )
    .bind(&batch.id)
    .bind(&successor)
    .fetch_one(fixture.db.pool())
    .await
    .expect("handoff guard persists its batch blocker");
    assert_eq!(paused_batch.0, "paused");
    assert_eq!(paused_batch.1, "blocked");
    assert_eq!(
        paused_batch.2.as_deref(),
        Some(format!("handoff:workspace_base_missing_commit:{repo_id}:{integrated_sha}").as_str())
    );
}
