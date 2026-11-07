use anyhow::Result;
use api_types::{
    ActorRef, AddRoleMembershipRequest, ApproveTaskCoordinationPlanRequest,
    AuthorizeTaskCoordinationRequest, CoordinationMode, CreateTaskCoordinationTurnRequest,
    CreateTaskCoordinationTurnResponse, CreateTaskRequest, CreateTaskRoleRequest,
    MaterializeTaskCoordinationPlanRequest, MaterializeTaskCoordinationPlanResponse,
    PaginatedResponse, RevokeTaskCoordinationRequest, RoleMembershipResponse, TaskCoordinationMode,
    TaskCoordinationOverviewResponse, TaskCoordinationPlanDecision, TaskCoordinationPlanResponse,
    TaskDispatchActionRequest, TaskDispatchActionResponse, TaskLaunchResponse, TaskLifecycleState,
    TaskLifecycleTransitionResponse, TaskResponse, TaskRoleResponse,
    TransitionTaskLifecycleRequest,
};
use clap::{Subcommand, ValueEnum};

use crate::{
    client::ForgeClient,
    output::{print_json, print_table_tasks},
    OutputFormat,
};

#[derive(clap::Args)]
pub struct TaskArgs {
    #[command(subcommand)]
    pub cmd: TaskCmd,
}

#[derive(Subcommand)]
pub enum TaskCmd {
    Create {
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        priority: Option<i64>,
    },
    List {
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        lifecycle_state: Option<LifecycleStateArg>,
        #[arg(long)]
        limit: Option<i64>,
    },
    Get {
        id: String,
    },
    Launch {
        id: String,
        #[arg(long)]
        expected_task_version: i64,
        #[arg(long)]
        expected_lifecycle_version: i64,
        #[arg(long)]
        idempotency_key: String,
    },
    Pause {
        id: String,
        #[arg(long)]
        expected_task_version: i64,
        #[arg(long)]
        expected_lifecycle_version: i64,
        #[arg(long)]
        idempotency_key: String,
        #[arg(long)]
        reason: Option<String>,
    },
    Resume {
        id: String,
        #[arg(long)]
        expected_task_version: i64,
        #[arg(long)]
        expected_lifecycle_version: i64,
        #[arg(long)]
        idempotency_key: String,
    },
    Cancel {
        id: String,
        #[arg(long)]
        expected_task_version: i64,
        #[arg(long)]
        expected_lifecycle_version: i64,
        #[arg(long)]
        idempotency_key: String,
        #[arg(long)]
        reason: Option<String>,
    },
    Role {
        #[command(subcommand)]
        command: TaskRoleCmd,
    },
    Coordination {
        #[command(subcommand)]
        command: CoordinationCmd,
    },
    Transition {
        id: String,
        #[arg(value_enum)]
        to_state: LifecycleStateArg,
        #[arg(long)]
        expected_lifecycle_version: i64,
        #[arg(long)]
        idempotency_key: String,
        #[arg(long)]
        gate_evaluation_id: Option<String>,
        #[arg(long)]
        reason_kind: Option<String>,
        #[arg(long)]
        reason_ref: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum TaskRoleCmd {
    List {
        id: String,
    },
    Create {
        id: String,
        #[arg(long)]
        role: String,
        #[arg(long, value_enum)]
        coordination_mode: CoordinationModeArg,
    },
    AddMember {
        id: String,
        role: String,
        #[arg(long, value_enum)]
        actor_kind: ActorKindArg,
        #[arg(long)]
        actor_id: String,
    },
}

#[derive(Subcommand)]
pub enum CoordinationCmd {
    Show {
        id: String,
    },
    Authorize {
        id: String,
        #[arg(long)]
        expected_task_version: i64,
        #[arg(long)]
        expected_coordination_version: i64,
        #[arg(long, value_enum)]
        mode: TaskCoordinationModeArg,
        #[arg(long)]
        role_id: Option<String>,
        #[arg(long, value_enum)]
        actor_kind: Option<ActorKindArg>,
        #[arg(long)]
        actor_id: Option<String>,
        #[arg(long)]
        envelope_json: Option<String>,
        #[arg(long)]
        idempotency_key: String,
    },
    Revoke {
        id: String,
        #[arg(long)]
        expected_task_version: i64,
        #[arg(long)]
        expected_coordination_version: i64,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        idempotency_key: String,
    },
    Request {
        id: String,
        #[arg(long)]
        expected_task_version: i64,
        #[arg(long)]
        expected_coordination_version: i64,
        #[arg(long)]
        objective: String,
        #[arg(long)]
        constraints: Option<String>,
        #[arg(long = "acceptance")]
        acceptance_criteria: Vec<String>,
        #[arg(long)]
        context: Option<String>,
        #[arg(long)]
        idempotency_key: String,
    },
    Approve {
        id: String,
        #[arg(long)]
        plan: String,
        #[arg(long)]
        expected_coordination_version: i64,
        #[arg(long)]
        expected_plan_digest: String,
        #[arg(long, value_enum, default_value_t = PlanDecisionArg::Approve)]
        decision: PlanDecisionArg,
        #[arg(long)]
        rationale: Option<String>,
        #[arg(long)]
        idempotency_key: String,
    },
    Materialize {
        id: String,
        #[arg(long)]
        plan: String,
        #[arg(long)]
        expected_task_version: i64,
        #[arg(long)]
        expected_coordination_version: i64,
        #[arg(long)]
        expected_plan_digest: String,
        #[arg(long)]
        expected_plan_version: i64,
        #[arg(long)]
        expected_request_version: i64,
        #[arg(long)]
        idempotency_key: String,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum TaskCoordinationModeArg {
    Manual,
    Proposal,
    PrepareAndExecute,
}

impl From<TaskCoordinationModeArg> for TaskCoordinationMode {
    fn from(value: TaskCoordinationModeArg) -> Self {
        match value {
            TaskCoordinationModeArg::Manual => Self::Manual,
            TaskCoordinationModeArg::Proposal => Self::Proposal,
            TaskCoordinationModeArg::PrepareAndExecute => Self::PrepareAndExecute,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum PlanDecisionArg {
    Approve,
    Reject,
}

impl From<PlanDecisionArg> for TaskCoordinationPlanDecision {
    fn from(value: PlanDecisionArg) -> Self {
        match value {
            PlanDecisionArg::Approve => Self::Approve,
            PlanDecisionArg::Reject => Self::Reject,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum CoordinationModeArg {
    Partitioned,
    Collaborative,
    Independent,
}

impl From<CoordinationModeArg> for CoordinationMode {
    fn from(value: CoordinationModeArg) -> Self {
        match value {
            CoordinationModeArg::Partitioned => Self::Partitioned,
            CoordinationModeArg::Collaborative => Self::Collaborative,
            CoordinationModeArg::Independent => Self::Independent,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum ActorKindArg {
    Human,
    Agent,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum LifecycleStateArg {
    Backlog,
    Ready,
    Active,
    Blocked,
    ReadyToMerge,
    Merging,
    Done,
    Cancelled,
}

impl From<LifecycleStateArg> for TaskLifecycleState {
    fn from(value: LifecycleStateArg) -> Self {
        match value {
            LifecycleStateArg::Backlog => Self::Backlog,
            LifecycleStateArg::Ready => Self::Ready,
            LifecycleStateArg::Active => Self::Active,
            LifecycleStateArg::Blocked => Self::Blocked,
            LifecycleStateArg::ReadyToMerge => Self::ReadyToMerge,
            LifecycleStateArg::Merging => Self::Merging,
            LifecycleStateArg::Done => Self::Done,
            LifecycleStateArg::Cancelled => Self::Cancelled,
        }
    }
}

impl LifecycleStateArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::Backlog => "backlog",
            Self::Ready => "ready",
            Self::Active => "active",
            Self::Blocked => "blocked",
            Self::ReadyToMerge => "ready_to_merge",
            Self::Merging => "merging",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }
}

impl TaskArgs {
    pub async fn run(&self, client: &ForgeClient, output: &OutputFormat) -> Result<()> {
        match &self.cmd {
            TaskCmd::Create {
                project_id,
                title,
                description,
                priority,
            } => {
                let request = CreateTaskRequest {
                    title: title.clone(),
                    description: description.clone(),
                    parent_task_id: None,
                    priority: *priority,
                };
                let task: TaskResponse = client
                    .post(&format!("/api/v1/projects/{project_id}/tasks"), &request)
                    .await?;
                print_task(output, &task)
            }
            TaskCmd::List {
                project_id,
                lifecycle_state,
                limit,
            } => {
                let response: PaginatedResponse<TaskResponse> = client
                    .get(&task_list_path(
                        project_id,
                        lifecycle_state.map(LifecycleStateArg::as_str),
                        *limit,
                    ))
                    .await?;
                match output {
                    OutputFormat::Json => print_json(&response),
                    OutputFormat::Table => {
                        print_table_tasks(&response.items);
                        Ok(())
                    }
                }
            }
            TaskCmd::Get { id } => {
                let task: TaskResponse = client.get(&format!("/api/v1/tasks/{id}")).await?;
                print_task(output, &task)
            }
            TaskCmd::Launch {
                id,
                expected_task_version,
                expected_lifecycle_version,
                idempotency_key,
            } => {
                let request = TaskDispatchActionRequest {
                    expected_task_version: *expected_task_version,
                    expected_lifecycle_version: *expected_lifecycle_version,
                    idempotency_key: idempotency_key.clone(),
                    reason: None,
                };
                let response: TaskLaunchResponse = client
                    .post(&format!("/api/v1/tasks/{id}/dispatch/launch"), &request)
                    .await?;
                match output {
                    OutputFormat::Json => print_json(&response),
                    OutputFormat::Table => {
                        println!(
                            "Task {} dispatch={:?} v{} admitted={} admitted_executions={} started_executions={}",
                            id,
                            response.dispatch.state,
                            response.dispatch.version,
                            response.admitted,
                            response.admitted_execution_ids.len(),
                            response.started_execution_ids.len()
                        );
                        Ok(())
                    }
                }
            }
            TaskCmd::Pause {
                id,
                expected_task_version,
                expected_lifecycle_version,
                idempotency_key,
                reason,
            } => {
                let request = TaskDispatchActionRequest {
                    expected_task_version: *expected_task_version,
                    expected_lifecycle_version: *expected_lifecycle_version,
                    idempotency_key: idempotency_key.clone(),
                    reason: reason.clone(),
                };
                let response: TaskDispatchActionResponse = client
                    .post(&format!("/api/v1/tasks/{id}/dispatch/pause"), &request)
                    .await?;
                print_dispatch_action(output, id, &response)
            }
            TaskCmd::Resume {
                id,
                expected_task_version,
                expected_lifecycle_version,
                idempotency_key,
            } => {
                let request = TaskDispatchActionRequest {
                    expected_task_version: *expected_task_version,
                    expected_lifecycle_version: *expected_lifecycle_version,
                    idempotency_key: idempotency_key.clone(),
                    reason: None,
                };
                let response: TaskDispatchActionResponse = client
                    .post(&format!("/api/v1/tasks/{id}/dispatch/resume"), &request)
                    .await?;
                print_dispatch_action(output, id, &response)
            }
            TaskCmd::Cancel {
                id,
                expected_task_version,
                expected_lifecycle_version,
                idempotency_key,
                reason,
            } => {
                let request = TaskDispatchActionRequest {
                    expected_task_version: *expected_task_version,
                    expected_lifecycle_version: *expected_lifecycle_version,
                    idempotency_key: idempotency_key.clone(),
                    reason: reason.clone(),
                };
                let response: TaskDispatchActionResponse = client
                    .post(&format!("/api/v1/tasks/{id}/dispatch/cancel"), &request)
                    .await?;
                print_dispatch_action(output, id, &response)
            }
            TaskCmd::Role { command } => match command {
                TaskRoleCmd::List { id } => {
                    let roles: Vec<TaskRoleResponse> = client
                        .get(&format!("/api/v1/tasks/{id}/task-roles"))
                        .await?;
                    match output {
                        OutputFormat::Json => print_json(&roles),
                        OutputFormat::Table => {
                            for role in &roles {
                                println!(
                                    "{} coordination={:?} members={}",
                                    role.role,
                                    role.coordination_mode,
                                    role.members.len()
                                );
                            }
                            Ok(())
                        }
                    }
                }
                TaskRoleCmd::Create {
                    id,
                    role,
                    coordination_mode,
                } => {
                    let request = CreateTaskRoleRequest {
                        role: role.clone(),
                        coordination_mode: (*coordination_mode).into(),
                        configuration: api_types::TaskRoleConfiguration::default(),
                        policy: None,
                    };
                    let response: TaskRoleResponse = client
                        .post(&format!("/api/v1/tasks/{id}/task-roles"), &request)
                        .await?;
                    match output {
                        OutputFormat::Json => print_json(&response),
                        OutputFormat::Table => {
                            println!(
                                "task role {} coordination={:?}",
                                response.role, response.coordination_mode
                            );
                            Ok(())
                        }
                    }
                }
                TaskRoleCmd::AddMember {
                    id,
                    role,
                    actor_kind,
                    actor_id,
                } => {
                    let actor_ref = match actor_kind {
                        ActorKindArg::Human => ActorRef::Human(actor_id.clone()),
                        ActorKindArg::Agent => ActorRef::Agent(actor_id.clone()),
                    };
                    let request = AddRoleMembershipRequest { actor_ref };
                    let response: RoleMembershipResponse = client
                        .post(
                            &format!("/api/v1/tasks/{id}/task-roles/{role}/members"),
                            &request,
                        )
                        .await?;
                    match output {
                        OutputFormat::Json => print_json(&response),
                        OutputFormat::Table => {
                            println!(
                                "membership {} actor={:?} status={:?}",
                                response.id, response.actor_ref, response.status
                            );
                            Ok(())
                        }
                    }
                }
            },
            TaskCmd::Coordination { command } => {
                coordination_command(client, output, command).await
            }
            TaskCmd::Transition {
                id,
                to_state,
                expected_lifecycle_version,
                idempotency_key,
                gate_evaluation_id,
                reason_kind,
                reason_ref,
            } => {
                let request = TransitionTaskLifecycleRequest {
                    to_state: (*to_state).into(),
                    expected_lifecycle_version: *expected_lifecycle_version,
                    idempotency_key: idempotency_key.clone(),
                    gate_evaluation_id: gate_evaluation_id.clone(),
                    reason_kind: reason_kind.clone(),
                    reason_ref: reason_ref.clone(),
                };
                let response: TaskLifecycleTransitionResponse = client
                    .post(&format!("/api/v1/tasks/{id}/lifecycle"), &request)
                    .await?;
                match output {
                    OutputFormat::Json => print_json(&response),
                    OutputFormat::Table => {
                        println!(
                            "task {} lifecycle={} version={} transition={}",
                            response.task_id,
                            serde_json::to_value(response.lifecycle.state)?
                                .as_str()
                                .unwrap_or("unknown"),
                            response.lifecycle.version,
                            response.transition_id.as_deref().unwrap_or("replayed")
                        );
                        Ok(())
                    }
                }
            }
        }
    }
}

async fn coordination_command(
    client: &ForgeClient,
    output: &OutputFormat,
    command: &CoordinationCmd,
) -> Result<()> {
    match command {
        CoordinationCmd::Show { id } => {
            let response: TaskCoordinationOverviewResponse = client
                .get(&format!("/api/v1/tasks/{id}/coordination"))
                .await?;
            match output {
                OutputFormat::Json => print_json(&response),
                OutputFormat::Table => {
                    println!(
                        "Task {} coordination={:?} v{} turns={}/{} quality={}/{} can_request={}",
                        id,
                        response.mode,
                        response.authorization_version,
                        response.turn_count,
                        response.max_turns,
                        response.quality_request_count,
                        response.max_quality_requests,
                        response.can_request
                    );
                    if let Some(coordinator) = response.coordinator.as_ref() {
                        println!(
                            "coordinator: {} ({}) harness={}",
                            coordinator.display_name,
                            coordinator.role,
                            coordinator.harness.as_deref().unwrap_or("none")
                        );
                    }
                    for blocker in &response.blockers {
                        println!("blocker {}: {}", blocker.code, blocker.message);
                    }
                    Ok(())
                }
            }
        }
        CoordinationCmd::Authorize {
            id,
            expected_task_version,
            expected_coordination_version,
            mode,
            role_id,
            actor_kind,
            actor_id,
            envelope_json,
            idempotency_key,
        } => {
            let coordinator_actor = match (actor_kind, actor_id) {
                (Some(ActorKindArg::Human), Some(actor_id)) => {
                    Some(ActorRef::Human(actor_id.clone()))
                }
                (Some(ActorKindArg::Agent), Some(actor_id)) => {
                    Some(ActorRef::Agent(actor_id.clone()))
                }
                (None, None) => None,
                _ => anyhow::bail!("--actor-kind and --actor-id must be provided together"),
            };
            let envelope = envelope_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?;
            let request = AuthorizeTaskCoordinationRequest {
                expected_task_version: *expected_task_version,
                expected_coordination_version: *expected_coordination_version,
                mode: (*mode).into(),
                coordinator_task_role_id: role_id.clone(),
                coordinator_actor,
                envelope,
                idempotency_key: idempotency_key.clone(),
            };
            let response: TaskCoordinationOverviewResponse = client
                .post(
                    &format!("/api/v1/tasks/{id}/coordination/authorize"),
                    &request,
                )
                .await?;
            print_json(&response)
        }
        CoordinationCmd::Revoke {
            id,
            expected_task_version,
            expected_coordination_version,
            reason,
            idempotency_key,
        } => {
            let request = RevokeTaskCoordinationRequest {
                expected_task_version: *expected_task_version,
                expected_coordination_version: *expected_coordination_version,
                idempotency_key: idempotency_key.clone(),
                reason: reason.clone(),
            };
            let response: TaskCoordinationOverviewResponse = client
                .post(&format!("/api/v1/tasks/{id}/coordination/revoke"), &request)
                .await?;
            print_json(&response)
        }
        CoordinationCmd::Request {
            id,
            expected_task_version,
            expected_coordination_version,
            objective,
            constraints,
            acceptance_criteria,
            context,
            idempotency_key,
        } => {
            let request = CreateTaskCoordinationTurnRequest {
                expected_task_version: *expected_task_version,
                expected_coordination_version: *expected_coordination_version,
                objective: objective.clone(),
                constraints: constraints.clone(),
                acceptance_criteria: acceptance_criteria.clone(),
                context: context.clone(),
                idempotency_key: idempotency_key.clone(),
            };
            let response: CreateTaskCoordinationTurnResponse = client
                .post(
                    &format!("/api/v1/tasks/{id}/coordination/requests"),
                    &request,
                )
                .await?;
            match output {
                OutputFormat::Json => print_json(&response),
                OutputFormat::Table => {
                    println!(
                        "request {} state={:?} accepted={}",
                        response.request.id, response.request.state, response.accepted
                    );
                    Ok(())
                }
            }
        }
        CoordinationCmd::Approve {
            id,
            plan,
            expected_coordination_version,
            expected_plan_digest,
            decision,
            rationale,
            idempotency_key,
        } => {
            let request = ApproveTaskCoordinationPlanRequest {
                expected_coordination_version: *expected_coordination_version,
                expected_plan_digest: expected_plan_digest.clone(),
                decision: (*decision).into(),
                rationale: rationale.clone(),
                idempotency_key: idempotency_key.clone(),
            };
            let response: TaskCoordinationPlanResponse = client
                .post(
                    &format!("/api/v1/tasks/{id}/coordination/plans/{plan}/approve"),
                    &request,
                )
                .await?;
            print_json(&response)
        }
        CoordinationCmd::Materialize {
            id,
            plan,
            expected_task_version,
            expected_coordination_version,
            expected_plan_digest,
            expected_plan_version,
            expected_request_version,
            idempotency_key,
        } => {
            let request = MaterializeTaskCoordinationPlanRequest {
                expected_task_version: *expected_task_version,
                expected_coordination_version: *expected_coordination_version,
                expected_plan_digest: expected_plan_digest.clone(),
                expected_plan_version: *expected_plan_version,
                expected_request_version: *expected_request_version,
                idempotency_key: idempotency_key.clone(),
            };
            let response: MaterializeTaskCoordinationPlanResponse = client
                .post(
                    &format!("/api/v1/tasks/{id}/coordination/plans/{plan}/materialize"),
                    &request,
                )
                .await?;
            match output {
                OutputFormat::Json => print_json(&response),
                OutputFormat::Table => {
                    println!(
                        "materialized plan {} receipt={} work_units={} replayed={} launch_accepted={}",
                        response.plan.artifact_id,
                        response.materialization.id,
                        response.materialization.work_unit_ids.len(),
                        response.replayed,
                        response.launch_intent_accepted
                    );
                    for blocker in &response.launch_blockers {
                        println!("launch blocker: {blocker}");
                    }
                    Ok(())
                }
            }
        }
    }
}

fn task_list_path(project_id: &str, lifecycle_state: Option<&str>, limit: Option<i64>) -> String {
    let mut params = Vec::new();
    if let Some(state) = lifecycle_state {
        params.push(format!("lifecycle_state={state}"));
    }
    if let Some(limit) = limit {
        params.push(format!("limit={limit}"));
    }
    if params.is_empty() {
        format!("/api/v1/projects/{project_id}/tasks")
    } else {
        format!("/api/v1/projects/{project_id}/tasks?{}", params.join("&"))
    }
}

fn print_task(output: &OutputFormat, task: &TaskResponse) -> Result<()> {
    match output {
        OutputFormat::Json => print_json(task),
        OutputFormat::Table => {
            print_table_tasks(std::slice::from_ref(task));
            Ok(())
        }
    }
}

fn print_dispatch_action(
    output: &OutputFormat,
    task_id: &str,
    response: &TaskDispatchActionResponse,
) -> Result<()> {
    match output {
        OutputFormat::Json => print_json(response),
        OutputFormat::Table => {
            println!(
                "Task {} lifecycle={} dispatch={:?} v{}",
                task_id,
                serde_json::to_value(response.task.lifecycle.state)?
                    .as_str()
                    .unwrap_or("unknown"),
                response.dispatch.state,
                response.dispatch.version
            );
            Ok(())
        }
    }
}
