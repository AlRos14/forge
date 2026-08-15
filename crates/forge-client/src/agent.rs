use anyhow::Result;
use api_types::{AgentResponse, CreateAgentRequest, PaginatedResponse};
use clap::Subcommand;

use crate::{
    client::ForgeClient,
    output::{print_json, print_table_agents},
    OutputFormat,
};

#[derive(clap::Args)]
pub struct AgentArgs {
    #[command(subcommand)]
    cmd: AgentCmd,
}

#[derive(Subcommand)]
enum AgentCmd {
    Register {
        #[arg(long)]
        name: String,
        #[arg(long)]
        executor_type: String,
        #[arg(long)]
        daemon_id: Option<String>,
        /// Provider entry that powers this harness agent (dispatch-time key injection).
        #[arg(long)]
        credential_id: Option<String>,
    },
    List {
        #[arg(long)]
        status: Option<String>,
    },
    Get {
        id: String,
    },
}

impl AgentArgs {
    pub async fn run(&self, client: &ForgeClient, output: &OutputFormat) -> Result<()> {
        match &self.cmd {
            AgentCmd::Register {
                name,
                executor_type,
                daemon_id,
                credential_id,
            } => {
                let request = CreateAgentRequest {
                    name: name.clone(),
                    description: None,
                    executor_type: executor_type.clone(),
                    model: None,
                    reasoning_effort: None,
                    permission_policy: None,
                    prompt_template: None,
                    capabilities: None,
                    config_json: None,
                    daemon_id: daemon_id.clone(),
                    max_concurrent_tasks: None,
                    heartbeat_interval_seconds: None,
                    max_missed_heartbeats: None,
                    is_default: None,
                    credential_id: credential_id.clone(),
                };
                let agent: AgentResponse = client.post("/api/v1/agents", &request).await?;
                print_agent(output, &agent)
            }
            AgentCmd::List { status } => {
                let response: PaginatedResponse<AgentResponse> =
                    client.get(&agent_list_path(status.as_deref())).await?;
                match output {
                    OutputFormat::Json => print_json(&response),
                    OutputFormat::Table => {
                        print_table_agents(&response.items);
                        Ok(())
                    }
                }
            }
            AgentCmd::Get { id } => {
                let agent: AgentResponse = client.get(&format!("/api/v1/agents/{id}")).await?;
                print_agent(output, &agent)
            }
        }
    }
}

fn agent_list_path(status: Option<&str>) -> String {
    match status {
        Some(status) => format!("/api/v1/agents?status={status}"),
        None => "/api/v1/agents".to_owned(),
    }
}

fn print_agent(output: &OutputFormat, agent: &AgentResponse) -> Result<()> {
    match output {
        OutputFormat::Json => print_json(agent),
        OutputFormat::Table => {
            print_table_agents(std::slice::from_ref(agent));
            Ok(())
        }
    }
}
