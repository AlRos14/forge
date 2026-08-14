use anyhow::Result;
use api_types::{CreateProjectRequest, PaginatedResponse, ProjectResponse};
use clap::Subcommand;

use crate::{
    client::ForgeClient,
    output::{print_json, print_table_projects},
    OutputFormat,
};

#[derive(clap::Args)]
pub struct ProjectArgs {
    #[command(subcommand)]
    cmd: ProjectCmd,
}

#[derive(Subcommand)]
enum ProjectCmd {
    Create {
        #[arg(long)]
        name: String,
    },
    List,
}

impl ProjectArgs {
    pub async fn run(&self, client: &ForgeClient, output: &OutputFormat) -> Result<()> {
        match &self.cmd {
            ProjectCmd::Create { name } => {
                let request = CreateProjectRequest {
                    name: name.clone(),
                    settings: None,
                    default_review_config: None,
                    paused: None,
                    project_agent_identity_id: None,
                    project_agent_profile_id: None,
                };
                let project: ProjectResponse = client.post("/api/v1/projects", &request).await?;
                print_project(output, &project)
            }
            ProjectCmd::List => {
                let response: PaginatedResponse<ProjectResponse> =
                    client.get("/api/v1/projects").await?;
                match output {
                    OutputFormat::Json => print_json(&response),
                    OutputFormat::Table => {
                        print_table_projects(&response.items);
                        Ok(())
                    }
                }
            }
        }
    }
}

fn print_project(output: &OutputFormat, project: &ProjectResponse) -> Result<()> {
    match output {
        OutputFormat::Json => print_json(project),
        OutputFormat::Table => {
            print_table_projects(std::slice::from_ref(project));
            Ok(())
        }
    }
}
