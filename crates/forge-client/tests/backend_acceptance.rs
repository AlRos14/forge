use std::{fs, process::Stdio, sync::Arc, time::Duration};

use api::AppState;
use api_types::RegisterRequest;
use events::EventBus;
use serde_json::Value;
use tokio::{io::AsyncWriteExt, process::Command, sync::oneshot, time::timeout};

const TEST_EMAIL: &str = "pr15-cli@example.test";
const TEST_PASSWORD: &str = "temporary-pr15-password";

#[tokio::test]
async fn forge_ctl_creates_project_and_task_against_real_backend() {
    let data_dir = tempfile::tempdir().expect("temporary CLI data directory");
    let web_dir = tempfile::tempdir().expect("temporary web distribution");
    fs::write(web_dir.path().join("index.html"), "<html></html>")
        .expect("create minimal web entry point");

    let pool = db::create_sqlite_pool("sqlite::memory:")
        .await
        .expect("create isolated SQLite database");
    db::run_migrations(&pool)
        .await
        .expect("migrate isolated SQLite database");
    let database = Arc::new(db::SqliteDb::new(pool));
    let state = AppState::new(database, Arc::new(EventBus::new(128)), true);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind temporary loopback listener");
    let address = listener.local_addr().expect("read listener address");
    let server_url = format!("http://{address}");
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(api::serve_with_listener(
        listener,
        state,
        web_dir.path().to_path_buf(),
        async move {
            let _ = shutdown_rx.await;
        },
    ));

    let http = reqwest::Client::new();
    let registration = http
        .post(format!("{server_url}/api/v1/auth/register"))
        .json(&RegisterRequest {
            email: TEST_EMAIL.to_owned(),
            password: TEST_PASSWORD.to_owned(),
            display_name: Some("PR15 CLI acceptance".to_owned()),
        })
        .send()
        .await
        .expect("register isolated user");
    assert_eq!(registration.status(), reqwest::StatusCode::CREATED);
    let auth: Value = registration
        .json()
        .await
        .expect("parse registration response");
    let access_token = auth["access_token"]
        .as_str()
        .expect("registration returns an access token");

    let login = run_cli(
        &server_url,
        data_dir.path(),
        &["login", "--email", TEST_EMAIL, "--password-stdin"],
        Some(TEST_PASSWORD),
    )
    .await;
    assert_success(&login, "CLI login");

    let project_output = run_cli(
        &server_url,
        data_dir.path(),
        &["project", "create", "--name", "PR15 CLI backend project"],
        None,
    )
    .await;
    assert_success(&project_output, "CLI Project creation");
    let project: Value = serde_json::from_slice(&project_output.stdout)
        .expect("CLI prints the created Project as JSON");
    let project_id = project["id"].as_str().expect("Project response has an ID");

    let task_output = run_cli(
        &server_url,
        data_dir.path(),
        &[
            "task",
            "create",
            "--project-id",
            project_id,
            "--title",
            "Created through forge-ctl",
        ],
        None,
    )
    .await;
    assert_success(&task_output, "CLI Task creation");
    let task: Value =
        serde_json::from_slice(&task_output.stdout).expect("CLI prints the created Task as JSON");
    let task_id = task["id"].as_str().expect("Task response has an ID");

    let fetched = http
        .get(format!("{server_url}/api/v1/tasks/{task_id}"))
        .bearer_auth(access_token)
        .send()
        .await
        .expect("read Task from the real backend");
    assert_eq!(fetched.status(), reqwest::StatusCode::OK);
    let fetched: Value = fetched.json().await.expect("parse Task response");
    assert_eq!(fetched["id"], task_id);
    assert_eq!(fetched["title"], "Created through forge-ctl");

    let _ = shutdown_tx.send(());
    timeout(Duration::from_secs(5), server)
        .await
        .expect("Forge server shuts down promptly")
        .expect("Forge server task completes")
        .expect("Forge server exits successfully");
}

async fn run_cli(
    server_url: &str,
    data_dir: &std::path::Path,
    args: &[&str],
    stdin: Option<&str>,
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_forge-ctl"));
    command
        .args(["--server", server_url, "--output", "json"])
        .args(args)
        .env("FORGE_DATA_DIR", data_dir)
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if stdin.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }

    let mut child = command.spawn().expect("start forge-ctl");
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .expect("CLI stdin is piped")
            .write_all(input.as_bytes())
            .await
            .expect("send isolated test password to stdin");
    }
    timeout(Duration::from_secs(20), child.wait_with_output())
        .await
        .expect("forge-ctl finishes within the bounded timeout")
        .expect("read forge-ctl process output")
}

fn assert_success(output: &std::process::Output, operation: &str) {
    assert!(
        output.status.success(),
        "{operation} failed with status {}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
