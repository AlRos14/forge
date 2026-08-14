use anyhow::{bail, Result};
use api_types::{ClaimTaskRequest, CreateTaskRequest, TaskResponse, TaskStatus};
use futures_util::StreamExt;
use serde_json::Value;

use crate::client::ForgeClient;

#[derive(clap::Args)]
pub struct RunArgs {
    #[arg(long)]
    project: String,
    #[arg(long)]
    agent: String,
    #[arg(long)]
    title: String,
    #[arg(long)]
    description: Option<String>,
}

impl RunArgs {
    pub async fn run(&self, client: &ForgeClient) -> Result<i32> {
        let created: TaskResponse = client
            .post(
                &format!("/api/v1/projects/{}/tasks", self.project),
                &CreateTaskRequest {
                    title: self.title.clone(),
                    description: self.description.clone(),
                    parent_task_id: None,
                    task_type: None,
                    priority: None,
                    review_config: None,
                    merge_config: None,
                    role_assignments: None,
                    governance: None,
                },
            )
            .await?;

        let claimed: TaskResponse = client
            .post(
                &format!("/api/v1/tasks/{}/claim", created.id),
                &ClaimTaskRequest {
                    agent_id: self.agent.clone(),
                    overrides: None,
                },
            )
            .await?;
        print_state_change(&claimed.id, &claimed.status);

        wait_for_terminal_state(client, &claimed.id).await
    }
}

async fn wait_for_terminal_state(client: &ForgeClient, task_id: &str) -> Result<i32> {
    let mut request = reqwest::Client::new().get(client.url("/api/v1/events"));
    if let Some(token) = client.bearer_token() {
        request = request.bearer_auth(token);
    }
    let response = request.send().await?;
    if !response.status().is_success() {
        bail!("event stream failed with status {}", response.status());
    }

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(index) = find_event_boundary(&buffer) {
            let raw_event = buffer[..index].to_owned();
            buffer.drain(..index + boundary_len(&buffer[index..]));
            if let Some(exit_code) = handle_sse_event(&raw_event, task_id)? {
                return Ok(exit_code);
            }
        }
    }

    bail!("event stream ended before task reached a terminal state")
}

fn handle_sse_event(raw_event: &str, task_id: &str) -> Result<Option<i32>> {
    let data = raw_event
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n");
    if data.trim().is_empty() {
        return Ok(None);
    }

    let event: Value = serde_json::from_str(&data)?;
    if event.get("entity_id").and_then(Value::as_str) != Some(task_id) {
        return Ok(None);
    }
    if event.get("event_type").and_then(Value::as_str) != Some("task.status_changed") {
        return Ok(None);
    }

    let Some(new_status) = event.get("new_status").and_then(Value::as_str) else {
        return Ok(None);
    };
    println!("task {task_id} status={new_status}");
    Ok(terminal_exit_code(new_status))
}

fn terminal_exit_code(status: &str) -> Option<i32> {
    match status {
        "done" => Some(0),
        "cancelled" | "blocked" | "merge_failed" => Some(1),
        _ => None,
    }
}

fn print_state_change(task_id: &str, status: &TaskStatus) {
    let status = serde_json::to_value(status)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned());
    println!("task {task_id} status={status}");
}

fn find_event_boundary(buffer: &str) -> Option<usize> {
    buffer.find("\n\n").or_else(|| buffer.find("\r\n\r\n"))
}

fn boundary_len(boundary: &str) -> usize {
    if boundary.starts_with("\r\n\r\n") {
        4
    } else {
        2
    }
}
