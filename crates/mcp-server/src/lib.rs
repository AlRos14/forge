#![forbid(unsafe_code)]

mod error;
mod params;
mod protocol;
mod rpc;
mod state;
mod tools;
mod values;

#[cfg(test)]
mod tests;

pub use protocol::{mcp_handler, mcp_router, McpError, McpRequest, McpResponse, McpUser};
pub use state::AppState;
