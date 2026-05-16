use clap::{Parser, Subcommand};
use forge_client::{
    agent, client::ForgeClient, daemon, mcp, project, repo, run, task, OutputFormat,
};

#[derive(Parser)]
#[command(name = "forge-ctl", about = "Forge CLI client")]
struct Cli {
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    server: String,
    #[arg(long, default_value = "table", value_enum)]
    output: OutputFormat,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Task(task::TaskArgs),
    Agent(agent::AgentArgs),
    Daemon(daemon::DaemonArgs),
    Project(project::ProjectArgs),
    Repo(repo::RepoArgs),
    Run(run::RunArgs),
    Mcp(mcp::McpArgs),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let client = ForgeClient::new(&cli.server);

    match cli.command {
        Commands::Task(args) => args.run(&client, &cli.output).await,
        Commands::Agent(args) => args.run(&client, &cli.output).await,
        Commands::Daemon(args) => args.run(&client, &cli.output).await,
        Commands::Project(args) => args.run(&client, &cli.output).await,
        Commands::Repo(args) => args.run(&client, &cli.output).await,
        Commands::Run(args) => {
            let exit_code = args.run(&client).await?;
            std::process::exit(exit_code);
        }
        Commands::Mcp(args) => args.run(&cli.server).await,
    }
}
