use anyhow::Result;
use api_types::MemoryBackfillResponse;
use clap::Subcommand;
use serde_json::json;

use crate::{client::ForgeClient, output::print_json, OutputFormat};

#[derive(clap::Args)]
pub struct MemoryArgs {
    #[command(subcommand)]
    cmd: MemoryCmd,
}

#[derive(Subcommand)]
enum MemoryCmd {
    Backfill,
}

impl MemoryArgs {
    pub async fn run(&self, client: &ForgeClient, output: &OutputFormat) -> Result<()> {
        match self.cmd {
            MemoryCmd::Backfill => {
                let response: MemoryBackfillResponse =
                    client.post("/api/v1/memory/backfill", &json!({})).await?;
                print_backfill(output, &response)
            }
        }
    }
}

fn print_backfill(output: &OutputFormat, response: &MemoryBackfillResponse) -> Result<()> {
    match output {
        OutputFormat::Json => print_json(response),
        OutputFormat::Table => {
            println!(
                "Indexed {} memory items, skipped {} already indexed",
                response.indexed, response.skipped
            );
            for item in &response.items {
                println!(
                    "{}: indexed {}, skipped {}",
                    item.source_type, item.indexed, item.skipped
                );
            }
            Ok(())
        }
    }
}
