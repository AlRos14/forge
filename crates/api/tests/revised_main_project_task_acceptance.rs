#![allow(dead_code)]

//! Acceptance coverage for the singular Main/Project Agent model.

mod common;

use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use db::AgentContextScopeRepo;
use serde_json::{json, Value};
use tower::ServiceExt;

const PROVIDER_SECRET: &str = "revised-acceptance-provider-secret";

#[tokio::test]
async fn main_project_handoff_project_task_worker_and_main_denial() {
    let workspace = common::TestDir::new("revised-main-project-task-acceptance");
    let harness = common::test_app(workspace.path(), "revised-main-project-task").await;
    harness
        .state
        .workflow_template_service
        .initialize()
        .await
        .expect("builtin workflow templates initialize");
    let app = &harness.app;
    let token = common::test_jwt();

    // Main, Project, and Worker are separate account-owned identities.  The
    // Worker is deliberately left unbound; Task assignment is its only
    // route into a repository Workspace.
    let main = connect_embedded(
        app,
        &token,
        "acceptance-main",
        &["read_account", "read_project", "handoff"],
    )
    .await;
    let project_agent = connect_embedded(
        app,
        &token,
        "acceptance-project",
        &["read_project", "propose_task", "read_task"],
    )
    .await;
    let worker = connect_embedded(
        app,
        &token,
        "acceptance-worker",
        &[
            "read_project",
            "read_task",
            "task_read",
            "task_write",
            "approve_actions",
        ],
    )
    .await;

    let main_identity = required_string(&main, &["agent", "id"]);
    let main_profile = required_string(&main, &["profile", "id"]);
    let project_identity = required_string(&project_agent, &["agent", "id"]);
    let project_profile = required_string(&project_agent, &["profile", "id"]);
    let worker_identity = required_string(&worker, &["agent", "id"]);
    let worker_profile = required_string(&worker, &["profile", "id"]);

    let main_binding = request_json(
        app,
        Method::PUT,
        "/api/v1/account/main-agent",
        &token,
        json!({
            "identity_id": main_identity,
            "profile_id": main_profile,
            "expected_version": 0,
            "autonomy_policy": {}
        }),
        &[StatusCode::OK, StatusCode::CREATED],
    )
    .await;
    let main_chat_id = required_string(&main_binding, &["chat_id"]);

    // Project creation carries the selected identity/profile so the Project,
    // binding, and Project Chat are one transaction in the replacement API.
    let project = request_json(
        app,
        Method::POST,
        "/api/v1/projects",
        &token,
        json!({
            "name": "Todo acceptance project",
            "project_agent_identity_id": project_identity,
            "project_agent_profile_id": project_profile
        }),
        &[StatusCode::OK, StatusCode::CREATED],
    )
    .await;
    let project_id = required_string(&project, &["id"]);

    let project_binding = request_json(
        app,
        Method::GET,
        &format!("/api/v1/projects/{project_id}/project-agent"),
        &token,
        Value::Null,
        &[StatusCode::OK],
    )
    .await;
    let project_chat_id = required_string(&project_binding, &["chat_id"]);
    assert_eq!(
        required_string(&project_binding, &["identity_id"]),
        required_string(&project_agent, &["agent", "id"])
    );

    // Core Main/Project chats are continuity scopes, not repository sessions.
    for (identity_id, profile_id, chat_id) in [
        (
            main_identity.as_str(),
            main_profile.as_str(),
            main_chat_id.as_str(),
        ),
        (
            project_identity.as_str(),
            project_profile.as_str(),
            project_chat_id.as_str(),
        ),
    ] {
        let session = request_json(
            app,
            Method::POST,
            &format!("/api/v1/agents/{identity_id}/sessions"),
            &token,
            json!({
                "profile_id": profile_id,
                "scope": { "type": "agent_chat", "chat_id": chat_id }
            }),
            &[StatusCode::OK, StatusCode::CREATED],
        )
        .await;
        let scope_id = required_string(&session, &["context_scope_id"]);
        let scope_row = AgentContextScopeRepo::get_context_scope(&*harness.state.db, &scope_id)
            .await
            .expect("core chat scope reads")
            .expect("core chat scope exists");
        assert_eq!(scope_row.workspace_access, "deny");
    }

    // A real repository keeps the subsequent TaskService claim on the normal
    // Workspace path instead of reducing the worker assertion to metadata.
    let repo_path = common::setup_git_repo(workspace.path());
    let default_branch = git_default_branch(&repo_path);
    let _repo = request_json(
        app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/repos"),
        &token,
        json!({
            "name": "todo-repo",
            "local_path": repo_path,
            "remote_url": repo_path,
            "default_branch": default_branch
        }),
        &[StatusCode::OK, StatusCode::CREATED],
    )
    .await;

    // The autonomous_v1 template has a canonical Worker role, which lets the
    // native Task session prove task_write without pretending that Project
    // Chat itself has repository authority.
    let _workflow = request_json(
        app,
        Method::PUT,
        &format!("/api/v1/projects/{project_id}/workflow"),
        &token,
        json!({ "template_name": "autonomous_v1" }),
        &[StatusCode::OK],
    )
    .await;

    let chats = request_json(
        app,
        Method::GET,
        "/api/v1/agent-chats",
        &token,
        Value::Null,
        &[StatusCode::OK],
    )
    .await;
    let chat_items = chats
        .get("items")
        .and_then(Value::as_array)
        .expect("chat switcher items");
    assert_eq!(
        chat_items
            .iter()
            .filter(|item| item.get("kind").and_then(Value::as_str) == Some("main"))
            .count(),
        1
    );
    assert_eq!(
        chat_items
            .iter()
            .filter(|item| item.get("project_id").and_then(Value::as_str) == Some(&project_id))
            .count(),
        1
    );
    assert!(chat_items.iter().all(|item| {
        let identity = item.get("identity_id").and_then(Value::as_str);
        identity != Some(&worker_identity)
    }));

    let handoff = request_json(
        app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/agent-handoffs"),
        &token,
        json!({
            "content": "Approved Todo brief: implement the smallest useful slice.",
            "dedupe_key": "revised-acceptance-handoff-1"
        }),
        &[StatusCode::OK, StatusCode::CREATED],
    )
    .await;
    assert_eq!(required_string(&handoff, &["source_chat_id"]), main_chat_id);
    assert_eq!(
        required_string(&handoff, &["target_chat_id"]),
        project_chat_id
    );
    assert!(matches!(
        required_string(&handoff, &["status"]).as_str(),
        "pending" | "delivered"
    ));
    assert!(handoff
        .get("target_message_id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty()));
    assert!(handoff
        .get("target_turn_job_id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty()));

    // A Main binding may discover and hand off, but it cannot submit the same
    // Task proposal operation even when it names a real Project.
    let denied = request_json(
        app,
        Method::POST,
        &format!("/api/v1/agents/{main_identity}/task-proposals"),
        &token,
        task_proposal_body(
            &project_id,
            "Main must not create this Task",
            "revised-acceptance-main-task-denial",
            &worker_identity,
        ),
        &[StatusCode::FORBIDDEN, StatusCode::NOT_FOUND],
    )
    .await;
    assert!(
        denied.get("id").is_none(),
        "denial must not create an action"
    );

    // Project Agent Task proposal remains an action envelope until the
    // existing approval/TaskService path commits the authoritative Task.
    let proposal = request_json(
        app,
        Method::POST,
        &format!("/api/v1/agents/{project_identity}/task-proposals"),
        &token,
        task_proposal_body(
            &project_id,
            "Implement Todo item",
            "revised-acceptance-project-task",
            &worker_identity,
        ),
        &[StatusCode::OK, StatusCode::CREATED],
    )
    .await;
    assert_eq!(required_string(&proposal, &["operation"]), "task.propose");
    let proposal_id = required_string(&proposal, &["id"]);
    let mut proposal_version = proposal
        .get("version")
        .and_then(Value::as_i64)
        .expect("proposal version");

    if proposal.get("status").and_then(Value::as_str) == Some("pending_approval") {
        let approved = request_json(
            app,
            Method::POST,
            &format!("/api/v1/actions/{proposal_id}/approve"),
            &token,
            json!({
                "expected_version": proposal_version,
                "approver_identity_id": worker_identity,
                "decision": "approved",
                "reason": "acceptance worker approval"
            }),
            &[StatusCode::OK],
        )
        .await;
        proposal_version = approved
            .get("version")
            .and_then(Value::as_i64)
            .expect("approved proposal version");
    }

    let executed = request_json(
        app,
        Method::POST,
        &format!("/api/v1/actions/{proposal_id}/execute-task"),
        &token,
        json!({
            "expected_version": proposal_version,
            "idempotency_key": "revised-acceptance-task-execution"
        }),
        &[StatusCode::OK],
    )
    .await;
    let task_id = required_string(&executed, &["task", "id"]);
    assert_eq!(
        required_string(&executed, &["task", "project_id"]),
        project_id
    );

    // Enter through the existing TaskService claim/workspace path.  Calling
    // the service directly avoids the HTTP claim handler's provider start,
    // which would make a clean-data test depend on an external model.
    let claimed = harness
        .state
        .task_service
        .claim_task(
            task_id.clone(),
            services::Assignee::Agent(worker_identity.clone()),
            None,
        )
        .await
        .expect("TaskService claim creates the running Worker execution");
    assert_eq!(claimed.execution.status.to_string(), "running");

    let session = request_json(
        app,
        Method::POST,
        &format!("/api/v1/agents/{worker_identity}/sessions"),
        &token,
        json!({
            "profile_id": worker_profile,
            "scope": { "type": "task", "task_id": task_id, "role": "worker" }
        }),
        &[StatusCode::OK, StatusCode::CREATED],
    )
    .await;
    let scope_id = required_string(&session, &["context_scope_id"]);
    let scope = AgentContextScopeRepo::get_context_scope(&*harness.state.db, &scope_id)
        .await
        .expect("Task scope reads")
        .expect("Task scope exists");
    assert_eq!(scope.identity_id, worker_identity);
    assert_eq!(scope.scope_type, "task");
    assert_eq!(scope.scope_id, task_id);
    assert_eq!(scope.task_role.as_deref(), Some("worker"));
    assert_eq!(scope.workspace_access, "task_write");

    // The same Task scope is not transferable to the Main identity, even
    // though that identity owns the account and can see the Project chat.
    let main_task_session = request_json(
        app,
        Method::POST,
        &format!("/api/v1/agents/{main_identity}/sessions"),
        &token,
        json!({
            "profile_id": required_string(&main, &["profile", "id"]),
            "scope": { "type": "task", "task_id": task_id, "role": "worker" }
        }),
        &[StatusCode::FORBIDDEN, StatusCode::NOT_FOUND],
    )
    .await;
    assert!(
        main_task_session.get("context_scope_id").is_none(),
        "Main denial must not create a Task session"
    );
}

async fn connect_embedded(app: &Router, token: &str, name: &str, permissions: &[&str]) -> Value {
    request_json(
        app,
        Method::POST,
        "/api/v1/embedded-agents/connect",
        token,
        json!({
            "name": name,
            "description": "V071 acceptance identity",
            "provider": "openai_compatible",
            "base_url": "https://8.8.8.8",
            "model": "acceptance-model",
            "credential_label": name,
            "credential": PROVIDER_SECRET,
            "account_permission_ceiling": { "permissions": permissions },
            "tool_policy": { "allowed": permissions }
        }),
        &[StatusCode::OK],
    )
    .await
}

fn task_proposal_body(project_id: &str, title: &str, dedupe_key: &str, worker_id: &str) -> Value {
    json!({
        "project_id": project_id,
        "title": title,
        "description": "Acceptance task",
        "role_assignments": [{
            "role_name": "worker",
            "assignee_type": "agent",
            "assignee_id": worker_id
        }],
        "dedupe_key": dedupe_key,
        "correlation_id": format!("{dedupe_key}-correlation")
    })
}

async fn request_json(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    body: Value,
    expected_statuses: &[StatusCode],
) -> Value {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(
            serde_json::to_vec(&body).expect("request JSON serializes"),
        ))
        .expect("request builds");
    let response = app.clone().oneshot(request).await.expect("router responds");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body reads");
    assert!(
        expected_statuses.contains(&status),
        "unexpected {status} from {uri}: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).expect("response JSON parses")
}

fn required_string(value: &Value, path: &[&str]) -> String {
    let mut current = value;
    for segment in path {
        current = current
            .get(*segment)
            .unwrap_or_else(|| panic!("missing JSON field {}", path.join(".")));
    }
    current
        .as_str()
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| panic!("JSON field {} is not a non-empty string", path.join(".")))
        .to_owned()
}

fn git_default_branch(path: &std::path::Path) -> String {
    let output = std::process::Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(path)
        .output()
        .expect("git default branch reads");
    assert!(output.status.success(), "git branch command succeeds");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}
