use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::Result;
use api_types::DaemonFrame;
use forge_client::daemon_link::{
    run_with_reconnect, DaemonClient, DaemonCommandStream, DAEMON_HEARTBEAT_INTERVAL_SECS,
};
use tokio::{
    sync::{mpsc, watch},
    time,
};

use crate::commands;

pub async fn run(
    client: Arc<DaemonClient>,
    workspace_root: PathBuf,
    shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let workspace_root = Arc::new(workspace_root);
    run_with_reconnect(client, move |stream| {
        let workspace_root = Arc::clone(&workspace_root);
        let shutdown = shutdown.clone();
        async move { dispatch_loop(stream, workspace_root, shutdown).await }
    })
    .await
}

async fn dispatch_loop(
    mut stream: DaemonCommandStream,
    workspace_root: Arc<PathBuf>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let (responses_tx, mut responses_rx) = mpsc::unbounded_channel::<DaemonFrame>();
    let terminal = crate::terminal::TerminalRuntime::new(
        responses_tx.clone(),
        workspace_root.as_ref().clone(),
    );
    let mut heartbeat = time::interval(Duration::from_secs(DAEMON_HEARTBEAT_INTERVAL_SECS));
    heartbeat.tick().await;
    let mut heartbeat_seq = 0_u64;

    loop {
        tokio::select! {
            result = shutdown.changed() => {
                if result.is_err() || *shutdown.borrow() {
                    tracing::info!("daemon command stream shutdown requested");
                    stream.close().await?;
                    return Ok(());
                }
            }
            frame = stream.recv() => {
                match frame {
                    Ok(frame @ DaemonFrame::Request { .. }) => {
                        let responses_tx = responses_tx.clone();
                        let workspace_root = Arc::clone(&workspace_root);
                        let terminal = Arc::clone(&terminal);
                        tokio::spawn(async move {
                            let response = commands::handle_request_with_terminal(
                                frame,
                                workspace_root.as_ref(),
                                Some(&terminal),
                            )
                            .await;
                            if responses_tx.send(response).is_err() {
                                tracing::warn!("daemon command response dropped because stream loop ended");
                            }
                        });
                    }
                    Ok(DaemonFrame::Heartbeat { seq }) => {
                        tracing::trace!(seq, "daemon command heartbeat received");
                    }
                    Ok(DaemonFrame::Notification { method, .. }) => {
                        tracing::warn!(%method, "unexpected daemon command notification received");
                    }
                    Ok(DaemonFrame::Response { id, .. }) => {
                        tracing::warn!(%id, "unexpected daemon command response received");
                    }
                    Ok(DaemonFrame::Error { id, error }) => {
                        tracing::warn!(
                            id = ?id,
                            code = %error.code,
                            message = %error.message,
                            "unexpected daemon command error received"
                        );
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "daemon command stream receive failed");
                        return Err(error);
                    }
                }
            }
            Some(response) = responses_rx.recv() => {
                stream.send(&response).await?;
            }
            _ = heartbeat.tick() => {
                heartbeat_seq = heartbeat_seq.saturating_add(1);
                stream.send_heartbeat(heartbeat_seq).await?;
            }
        }
    }
}
