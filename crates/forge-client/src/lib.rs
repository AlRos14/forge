#![forbid(unsafe_code)]

pub mod agent;
pub mod auth;
pub mod client;
pub mod daemon;
#[doc(hidden)]
pub mod daemon_fs;
pub mod daemon_link;
pub mod daemon_runtime;
pub mod mcp;
pub mod memory;
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
