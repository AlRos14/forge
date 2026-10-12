use std::io::{self, Read};

use anyhow::{bail, Context, Result};
use api_types::{
    AgentProviderId, CreateProviderEntryRequest, DisconnectCredentialResponse,
    ProviderEntriesResponse, ProviderEntryResponse, RenameProviderEntryRequest,
};
use clap::{Args, Subcommand};
use serde::Serialize;

use crate::{
    client::ForgeClient,
    output::print_json,
    password_prompt::{prompt_password, stdin_is_terminal},
    provider_login, OutputFormat,
};

#[derive(Args)]
pub struct ProviderArgs {
    #[command(subcommand)]
    command: ProviderCommand,
}

#[derive(Subcommand)]
enum ProviderCommand {
    /// List configured provider entries and discovered CLI runtimes.
    List,
    /// Add an API key provider entry. OAuth entries come from `login`.
    Add(ProviderAddArgs),
    /// Sign in to a provider with OAuth from this machine.
    Login(ProviderLoginArgs),
    /// Rename a provider entry.
    Rename {
        id: String,
        #[arg(long)]
        label: String,
        #[arg(long)]
        version: i64,
    },
    /// Disconnect a provider entry.
    Remove {
        id: String,
        #[arg(long)]
        version: i64,
    },
}

#[derive(Args)]
struct ProviderAddArgs {
    #[arg(long, value_enum)]
    provider: ProviderKind,
    #[arg(long, default_value = "default")]
    label: String,
    /// Read the API key from stdin. Without this flag a terminal prompt is used.
    #[arg(long)]
    credential_stdin: bool,
    #[arg(long)]
    base_url: Option<String>,
}

#[derive(Args)]
struct ProviderLoginArgs {
    #[arg(long, value_enum)]
    provider: ProviderKind,
    #[arg(long, default_value = "default")]
    label: String,
    #[arg(long, value_enum, default_value = "browser")]
    method: ProviderLoginMethod,
    #[arg(long)]
    no_open: bool,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum ProviderLoginMethod {
    Browser,
    Device,
}

#[derive(Clone, clap::ValueEnum)]
enum ProviderKind {
    Openai,
    Xai,
    Gemini,
    Openrouter,
    OpenaiCompatible,
}

impl From<ProviderKind> for AgentProviderId {
    fn from(kind: ProviderKind) -> Self {
        match kind {
            ProviderKind::Openai => AgentProviderId::OpenAi,
            ProviderKind::Xai => AgentProviderId::XAi,
            ProviderKind::Gemini => AgentProviderId::Gemini,
            ProviderKind::Openrouter => AgentProviderId::OpenRouter,
            ProviderKind::OpenaiCompatible => AgentProviderId::OpenAiCompatible,
        }
    }
}

impl ProviderArgs {
    pub async fn run(&self, client: &ForgeClient, output: &OutputFormat) -> Result<()> {
        match &self.command {
            ProviderCommand::List => {
                let response: ProviderEntriesResponse = client.get("/api/v1/providers").await?;
                print_result(output, &response)
            }
            ProviderCommand::Add(args) => {
                let request = CreateProviderEntryRequest {
                    provider: args.provider.clone().into(),
                    label: args.label.clone(),
                    credential: read_credential(args.credential_stdin)?,
                    base_url: args.base_url.clone(),
                };
                let response: ProviderEntryResponse =
                    client.post("/api/v1/providers", &request).await?;
                print_result(output, &response)
            }
            ProviderCommand::Login(args) => {
                let provider = args.provider.clone().into();
                let operation = match args.method {
                    ProviderLoginMethod::Browser => {
                        provider_login::browser_login(client, provider, &args.label, args.no_open)
                            .await?
                    }
                    ProviderLoginMethod::Device => {
                        provider_login::device_login(client, provider, &args.label).await?
                    }
                };
                print_result(output, &operation)?;
                match provider_login::failed_reason(&operation) {
                    Some(reason) => Err(anyhow::anyhow!(reason)),
                    None => Ok(()),
                }
            }
            ProviderCommand::Rename { id, label, version } => {
                let request = RenameProviderEntryRequest {
                    label: label.clone(),
                    version: *version,
                };
                let response: ProviderEntryResponse = client
                    .patch(&format!("/api/v1/providers/{id}"), &request)
                    .await?;
                print_result(output, &response)
            }
            ProviderCommand::Remove { id, version } => {
                let response: DisconnectCredentialResponse = client
                    .delete_json(&format!("/api/v1/providers/{id}?version={version}"))
                    .await?;
                print_result(output, &response)
            }
        }
    }
}

fn read_credential(from_stdin: bool) -> Result<String> {
    if from_stdin {
        let mut credential = String::new();
        io::stdin()
            .read_to_string(&mut credential)
            .context("read credential from stdin")?;
        let credential = credential.trim_end_matches(&['\r', '\n'][..]).to_owned();
        if credential.is_empty() {
            bail!("credential stdin was empty");
        }
        return Ok(credential);
    }
    if stdin_is_terminal() {
        return prompt_password();
    }
    bail!("credential input is not a terminal; pass --credential-stdin")
}

fn print_result<T: Serialize>(output: &OutputFormat, value: &T) -> Result<()> {
    match output {
        OutputFormat::Json | OutputFormat::Table => print_json(value),
    }
}
