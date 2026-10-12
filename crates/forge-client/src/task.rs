use anyhow::Result;
use api_types::{
    ActorRef, AddRoleMembershipRequest, CoordinationMode, CreateTaskRequest, CreateTaskRoleRequest,
    ExecutionPurpose, ExecutionResponse, PaginatedResponse, RoleMembershipResponse,
    StartExecutionRequest, TaskLifecycleState, TaskLifecycleTransitionResponse, TaskResponse,
    TaskRoleResponse, TransitionTaskLifecycleRequest,
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
    Execute {
        id: String,
        #[arg(long)]
        agent_id: String,
        #[arg(long)]
        role: String,
        #[arg(long, value_enum)]
        purpose: PurposeArg,
        #[arg(long)]
        prompt: String,
        #[arg(long = "input-artifact-id")]
        input_artifact_ids: Vec<String>,
    },
    Role {
        #[command(subcommand)]
        command: TaskRoleCmd,
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

#[derive(Clone, Copy, ValueEnum)]
pub enum PurposeArg {
    Plan,
    Implement,
    Review,
    Validate,
    Investigate,
    Orchestrate,
    General,
}

impl From<PurposeArg> for ExecutionPurpose {
    fn from(value: PurposeArg) -> Self {
        match value {
            PurposeArg::Plan => Self::Plan,
            PurposeArg::Implement => Self::Implement,
            PurposeArg::Review => Self::Review,
            PurposeArg::Validate => Self::Validate,
            PurposeArg::Investigate => Self::Investigate,
            PurposeArg::Orchestrate => Self::Orchestrate,
            PurposeArg::General => Self::General,
        }
    }
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
                    task_type: None,
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
            TaskCmd::Execute {
                id,
                agent_id,
                role,
                purpose,
                prompt,
                input_artifact_ids,
            } => {
                let request = StartExecutionRequest {
                    agent_id: agent_id.clone(),
                    role: role.clone(),
                    purpose: (*purpose).into(),
                    prompt: prompt.clone(),
                    input_artifact_ids: input_artifact_ids.clone(),
                };
                let execution: ExecutionResponse = client
                    .post(&format!("/api/v1/tasks/{id}/executions"), &request)
                    .await?;
                match output {
                    OutputFormat::Json => print_json(&execution),
                    OutputFormat::Table => {
                        let actor = serde_json::to_value(&execution.actor_ref)?.to_string();
                        let purpose = serde_json::to_value(&execution.purpose)?;
                        println!(
                            "Execution {} status={} actor={} role={} purpose={}",
                            execution.id,
                            serde_json::to_value(execution.status)?
                                .as_str()
                                .unwrap_or("unknown"),
                            actor,
                            execution.role,
                            purpose.as_str().unwrap_or("unknown")
                        );
                        Ok(())
                    }
                }
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
