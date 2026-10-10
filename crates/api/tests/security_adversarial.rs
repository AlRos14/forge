#![allow(dead_code)]

mod common;

use api_types::{AgentProfileResponse, AgentResponse, ErrorResponse};
use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

const PROVIDER_SECRET: &str = "provider-secret-never-return-this";

#[tokio::test]
async fn provider_credentials_are_redacted_and_unsupported_embedded_harnesses_fail_closed() {
    let workspace = common::TestDir::new("security-provider-ws");
    let harness = common::test_app(workspace.path(), "security-provider").await;
    let app = &harness.app;
    let token = common::test_jwt();

    for (index, base_url) in [
        "http://127.0.0.1:9",
        "https://127.0.0.1:9",
        "https://[::1]",
        "https://169.254.169.254",
        "https://10.0.0.1",
        "https://user:pass@example.com",
        "https://example.com/#fragment",
        "https://localhost",
    ]
    .into_iter()
    .enumerate()
    {
        let error: ErrorResponse = common::json_request_with_bearer(
            app,
            Method::POST,
            "/api/v1/providers",
            &token,
            json!({
                "provider": "openai_compatible",
                "label": format!("rejected-provider-url-{index}"),
                "credential": PROVIDER_SECRET,
                "base_url": base_url,
            }),
            StatusCode::BAD_REQUEST,
        )
        .await;
        assert_eq!(error.code, "invalid_operation");
        assert_json_does_not_contain_secret(
            &serde_json::to_value(&error).expect("URL error serializes"),
            PROVIDER_SECRET,
        );
    }

    for (index, executor_type) in ["embedded", "Embedded", " embedded "]
        .into_iter()
        .enumerate()
    {
        let unsupported: ErrorResponse = common::json_request_with_bearer(
            app,
            Method::POST,
            "/api/v1/agents",
            &token,
            json!({
                "name": format!("retired-embedded-harness-{index}"),
                "executor_type": executor_type
            }),
            StatusCode::BAD_REQUEST,
        )
        .await;
        assert_eq!(unsupported.code, "invalid_operation");
    }

    let _other_provider = common::create_provider_entry(
        app,
        &token,
        "openai_compatible",
        "adversarial",
        PROVIDER_SECRET,
        "https://8.8.8.8",
    )
    .await;
    let codex_entry = common::create_provider_entry(
        app,
        &token,
        "openai",
        "codex-adversarial",
        PROVIDER_SECRET,
        "https://8.8.8.8",
    )
    .await;
    let connected: AgentResponse = common::json_request_with_bearer(
        app,
        Method::POST,
        "/api/v1/agents",
        &token,
        json!({
            "name": "external-harness-agent",
            "executor_type": "codex",
            "credential_id": codex_entry.id
        }),
        StatusCode::OK,
    )
    .await;

    let profile_list: Vec<AgentProfileResponse> = common::empty_request_with_bearer(
        app,
        Method::GET,
        &format!("/api/v1/agents/{}/profiles", connected.id),
        &token,
        StatusCode::OK,
    )
    .await;
    assert!(!profile_list.is_empty());
    assert_json_does_not_contain_secret(
        &serde_json::to_value(&profile_list).expect("profile list serializes"),
        PROVIDER_SECRET,
    );

    let providers: api_types::ProviderEntriesResponse = common::empty_request_with_bearer(
        app,
        Method::GET,
        "/api/v1/providers",
        &token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(providers.items.len(), 2);
    assert!(providers
        .items
        .iter()
        .flat_map(|item| &item.used_by)
        .any(|usage| usage.agent_id == connected.id));
    assert_json_does_not_contain_secret(
        &serde_json::to_value(&providers).expect("provider list serializes"),
        PROVIDER_SECRET,
    );

    let fetched: AgentResponse = common::empty_request_with_bearer(
        app,
        Method::GET,
        &format!("/api/v1/agents/{}", connected.id),
        &token,
        StatusCode::OK,
    )
    .await;
    assert_json_does_not_contain_secret(
        &serde_json::to_value(&fetched).expect("agent response serializes"),
        PROVIDER_SECRET,
    );
}

#[tokio::test]
async fn recursive_config_redaction_covers_nested_objects_and_arrays() {
    let workspace = common::TestDir::new("security-redaction-ws");
    let harness = common::test_app(workspace.path(), "security-redaction").await;
    let app = &harness.app;
    let token = common::test_jwt();
    let secret_values = [
        "nested-api-key-value",
        "nested-bearer-value",
        "nested-private-key-value",
        "nested-password-value",
    ];

    let created: AgentResponse = common::json_request_with_bearer(
        app,
        Method::POST,
        "/api/v1/agents",
        &token,
        json!({
            "name": "nested-redaction",
            "executor_type": "shell",
            "config_json": {
                "safe": "visible",
                "api_key": secret_values[0],
                "nested": {
                    "authorization": secret_values[1],
                    "inner": { "private_key": secret_values[2] }
                },
                "array": [
                    { "password": secret_values[3] },
                    { "safe_nested": "still-visible" }
                ]
            }
        }),
        StatusCode::OK,
    )
    .await;
    let config = &created.config_json;
    assert_eq!(config["safe"], "visible");
    assert_eq!(config["nested"]["inner"]["private_key"], "[redacted]");
    assert_eq!(config["array"][1]["safe_nested"], "still-visible");
    for secret in secret_values {
        assert_json_does_not_contain_secret(&serde_json::to_value(&created).unwrap(), secret);
    }

    let fetched: AgentResponse = common::empty_request_with_bearer(
        app,
        Method::GET,
        &format!("/api/v1/agents/{}", created.id),
        &token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(fetched.config_json["api_key"], "[redacted]");
    for secret in secret_values {
        assert_json_does_not_contain_secret(&serde_json::to_value(&fetched).unwrap(), secret);
    }
}

fn assert_json_does_not_contain_secret(value: &Value, secret: &str) {
    assert!(
        !value.to_string().contains(secret),
        "secret leaked in JSON response: {secret}"
    );
}
