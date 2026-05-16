#![forbid(unsafe_code)]

pub mod agent;
pub mod client;
pub mod daemon;
pub mod daemon_link;
pub mod mcp;
pub mod output;
pub mod project;
pub mod repo;
pub mod run;
pub mod task;

#[derive(Clone, clap::ValueEnum)]
pub enum OutputFormat {
    Json,
    Table,
}
