use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use events::EventBus;
use serde::Deserialize;
use serde_json::json;

use super::{DaemonConnection, DaemonConnectionRegistry, DaemonExecutionEventHandler};
use crate::ServiceError;

struct NoopHandler;

#[async_trait]
impl DaemonExecutionEventHandler for NoopHandler {
    async fn handle_log(
        &self,
        _daemon_id: &str,
        _notification: api_types::ExecutionLogNotification,
    ) -> Result<(), ServiceError> {
        Ok(())
    }

    async fn handle_terminal(
        &self,
        _daemon_id: &str,
        _notification: api_types::ExecutionTerminalNotification,
    ) -> Result<(), ServiceError> {
        Ok(())
    }
}

fn make_registry() -> Arc<DaemonConnectionRegistry> {
    let event_bus = Arc::new(EventBus::new(16));
    let handler = Arc::new(NoopHandler) as Arc<dyn DaemonExecutionEventHandler>;
    Arc::new(DaemonConnectionRegistry::new(event_bus, handler))
}

#[derive(Debug, Deserialize)]
struct TestResponse {
    message: String,
}

#[tokio::test]
async fn daemon_transport_registry_happy_path_completes_typed_response() {
    let registry = make_registry();
    let (connection, mut outbound) = DaemonConnection::new("daemon-1".to_owned());
    registry.register("daemon-1".to_owned(), connection);

    let dispatcher = registry.clone();
    let handle = tokio::spawn(async move {
        let frame = outbound.recv().await.expect("request frame sent");
        let api_types::DaemonFrame::Request { id, method, params } = frame else {
            panic!("expected request frame");
        };
        assert_eq!(method, "test.echo");
        assert_eq!(params["name"], "forge");
        dispatcher.dispatch_incoming(
            "daemon-1",
            api_types::DaemonFrame::Response {
                id,
                result: json!({ "message": "ok" }),
            },
        );
    });

    let result: TestResponse = registry
        .send_request("daemon-1", "test.echo", json!({ "name": "forge" }), 1)
        .await
        .expect("daemon request succeeds");

    assert_eq!(result.message, "ok");
    handle.await.expect("dispatcher task joins");
}

#[tokio::test]
async fn daemon_transport_registry_timeout_returns_daemon_timeout() {
    let registry = make_registry();
    let (connection, _outbound) = DaemonConnection::new("daemon-1".to_owned());
    registry.register("daemon-1".to_owned(), connection);

    let result: Result<TestResponse, ServiceError> = registry
        .send_request_with_timeout(
            "daemon-1",
            "test.timeout",
            json!({}),
            Duration::from_millis(50),
        )
        .await;

    assert!(matches!(
        result,
        Err(ServiceError::DaemonTimeout { daemon_id, method })
            if daemon_id == "daemon-1" && method == "test.timeout"
    ));
}

#[tokio::test]
async fn daemon_transport_registry_unknown_daemon_returns_unavailable() {
    let registry = make_registry();

    let result: Result<TestResponse, ServiceError> = registry
        .send_request("missing-daemon", "test.echo", json!({}), 1)
        .await;

    assert!(matches!(
        result,
        Err(ServiceError::DaemonUnavailable { daemon_id })
            if daemon_id == "missing-daemon"
    ));
}

#[test]
fn daemon_transport_register_returns_prior_connection_on_second_call() {
    let registry = make_registry();
    let (first, _first_outbound) = DaemonConnection::new("daemon-1".to_owned());
    let (second, _second_outbound) = DaemonConnection::new("daemon-1".to_owned());

    let first_prior = registry.register("daemon-1".to_owned(), first);
    assert!(first_prior.is_none());

    let second_prior = registry.register("daemon-1".to_owned(), second);
    let prior = second_prior.expect("second register returns prior connection");
    assert_eq!(prior.daemon_id, "daemon-1");
    assert!(registry.is_connected("daemon-1"));
}
