use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use forge_client::daemon_link::{run_dispatch_loop, run_with_reconnect, DaemonClient};
use forge_client::daemon_runtime::ActiveExecutionTracker;
use tokio::sync::{mpsc, watch};

use crate::commands;

pub async fn run(
    client: Arc<DaemonClient>,
    workspace_root: PathBuf,
    active_executions: ActiveExecutionTracker,
    shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let workspace_root = Arc::new(workspace_root);
    run_with_reconnect(client, move |stream| {
        let workspace_root = Arc::clone(&workspace_root);
        let shutdown = shutdown.clone();
        let active_executions = active_executions.clone();
        async move {
            let (responses_tx, responses_rx) = mpsc::unbounded_channel();
            let terminal = crate::terminal::TerminalRuntime::new(
                responses_tx.clone(),
                workspace_root.as_ref().clone(),
            );
            let daemon_runtime = forge_client::daemon_runtime::DaemonRuntime::new_with_tracker(
                responses_tx.clone(),
                workspace_root.as_ref().clone(),
                active_executions,
            );
            let handler = {
                let workspace_root = Arc::clone(&workspace_root);
                let terminal = Arc::clone(&terminal);
                let daemon_runtime = Arc::clone(&daemon_runtime);
                move |frame| {
                    let workspace_root = Arc::clone(&workspace_root);
                    let terminal = Arc::clone(&terminal);
                    let daemon_runtime = Arc::clone(&daemon_runtime);
                    async move {
                        commands::handle_request_with_terminal(
                            frame,
                            workspace_root.as_ref(),
                            Some(&terminal),
                            Some(&daemon_runtime),
                        )
                        .await
                    }
                }
            };
            run_dispatch_loop(stream, handler, shutdown, responses_tx, responses_rx).await
        }
    })
    .await
}
