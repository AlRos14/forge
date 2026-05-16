use anyhow::Result;
use api_types::{
    ClaimTaskRequest, CreateTaskRequest, PaginatedResponse, TaskResponse, TransitionTaskRequest,
    TransitionTaskResponse,
};
use clap::Subcommand;
use serde_json::json;

use crate::{
    client::ForgeClient,
    output::{print_json, print_table_tasks},
    OutputFormat,
};

#[derive(clap::Args)]
pub struct TaskArgs {
    #[command(subcommand)]
    cmd: TaskCmd,
}

#[derive(Subcommand)]
enum TaskCmd {
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
        status: Option<String>,
        #[arg(long)]
        limit: Option<i64>,
    },
    Get {
        id: String,
    },
    Claim {
        id: String,
        #[arg(long)]
        agent_id: String,
    },
    Transition {
        id: String,
        status: String,
        version: i64,
    },
    Cancel {
        id: String,
    },
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
                    review_config: None,
                    merge_config: None,
                    role_assignments: None,
                };
                let task: TaskResponse = client
                    .post(&format!("/api/v1/projects/{project_id}/tasks"), &request)
                    .await?;
                print_task(output, &task)
            }
            TaskCmd::List {
                project_id,
                status,
                limit,
            } => {
                let response: PaginatedResponse<TaskResponse> = client
                    .get(&task_list_path(project_id, status.as_deref(), *limit))
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
            TaskCmd::Claim { id, agent_id } => {
                let request = ClaimTaskRequest {
                    agent_id: agent_id.clone(),
                    overrides: None,
                };
                let task: TaskResponse = client
                    .post(&format!("/api/v1/tasks/{id}/claim"), &request)
                    .await?;
                print_task(output, &task)
            }
            TaskCmd::Transition {
                id,
                status,
                version,
            } => {
                let request = TransitionTaskRequest {
                    status: status.to_string(),
                    version: *version,
                    reason: None,
                    source: None,
                };
                let response: TransitionTaskResponse = client
                    .post(&format!("/api/v1/tasks/{id}/transition"), &request)
                    .await?;
                print_task(output, &response.task)
            }
            TaskCmd::Cancel { id } => {
                let task: TaskResponse = client
                    .post(&format!("/api/v1/tasks/{id}/cancel"), &json!({}))
                    .await?;
                print_task(output, &task)
            }
        }
    }
}

fn task_list_path(project_id: &str, status: Option<&str>, limit: Option<i64>) -> String {
    let mut params = Vec::new();
    if let Some(status) = status {
        params.push(format!("status={status}"));
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
