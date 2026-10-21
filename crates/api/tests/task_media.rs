mod common;

use axum::http::{Method, StatusCode};
use serde_json::json;

#[tokio::test]
async fn retired_task_media_collection_routes_return_not_found() {
    let workspace = common::TestDir::new("retired-task-media-collection");
    let harness = common::test_app(workspace.path(), "retired-task-media-collection").await;
    let task_id = "00000000-0000-4000-8000-000000000000";
    let path = format!("/api/v1/tasks/{task_id}/media");

    for (method, body) in [
        (Method::GET, json!(null)),
        (
            Method::POST,
            json!({"file_name": "legacy.png", "content_type": "image/png"}),
        ),
    ] {
        let response = common::raw_json_request(&harness.app, method, &path, body).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn retired_task_media_item_routes_return_not_found() {
    let workspace = common::TestDir::new("retired-task-media-item");
    let harness = common::test_app(workspace.path(), "retired-task-media-item").await;
    let media_id = "00000000-0000-4000-8000-000000000000";
    let path = format!("/api/v1/media/{media_id}");

    for method in [Method::GET, Method::DELETE] {
        let response = common::raw_empty_request(&harness.app, method, &path).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
