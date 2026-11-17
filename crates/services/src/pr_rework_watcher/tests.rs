use super::*;
use crate::pr_github_client::{GitHubClient, GitHubPrProvider};
use crate::pr_rework::tests::{superseded, world, World};
use crate::Secret;
use db::{HumanDecisionRepo, PrReworkRepo, PrReworkSupersession};
use serde_json::json;
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const TOKEN: &str = "ghp_pr_rework_watcher_test_token";

#[derive(Debug, Clone)]
struct CapturedRequest {
    line: String,
    headers: String,
}

struct MockGitHub {
    base_url: String,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
}

async fn mock_github(state: &str, merged: bool, head_sha: &str, branch: &str) -> MockGitHub {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base_url = format!("http://{}", listener.local_addr().expect("address"));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);
    let state = state.to_owned();
    let head_sha = head_sha.to_owned();
    let branch = branch.to_owned();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 2048];
            loop {
                let read = socket.read(&mut buffer).await.unwrap_or(0);
                if read == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..read]);
                if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let headers = String::from_utf8_lossy(&bytes).to_string();
            let line = headers.lines().next().unwrap_or_default().to_owned();
            captured
                .lock()
                .expect("requests")
                .push(CapturedRequest { line, headers });
            let payload = json!({
                "number": 77,
                "html_url": "https://github.com/acme/widgets/pull/77",
                "state": state.as_str(),
                "merged": merged,
                "draft": false,
                "head": {"ref": branch.as_str(), "sha": head_sha.as_str()},
                "base": {"ref": "main", "sha": "base-sha"},
                "merge_commit_sha": if merged { Some("merge-sha") } else { None::<&str> },
                "mergeable": true,
                "mergeable_state": "clean",
                "updated_at": "2026-11-16T00:00:00Z"
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    MockGitHub { base_url, requests }
}

async fn watcher_rig(
    state: &str,
    merged: bool,
) -> (World, PrReworkSupersession, MockGitHub, PrReworkWatcher) {
    let world = world(true).await;
    // The shared rework fixture intentionally uses a placeholder provider id
    // and remote. Give this HTTP-level test a realistic frozen GitHub identity.
    for table in ["remote_pr_admission", "pr_metadata"] {
        let names: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'trigger' AND tbl_name = ?",
        )
        .bind(table)
        .fetch_all(world.db.pool())
        .await
        .expect("trigger names");
        for name in names {
            sqlx::query(&format!("DROP TRIGGER IF EXISTS \"{name}\""))
                .execute(world.db.pool())
                .await
                .expect("drop fixture trigger");
        }
    }
    sqlx::query(
        "UPDATE remote_pr_admission SET remote_repo_identity =
             'https://github.com/acme/widgets.git'
         WHERE task_merge_operation_id = ?",
    )
    .bind(&world.merge_id)
    .execute(world.db.pool())
    .await
    .expect("GitHub repository identity");
    sqlx::query("UPDATE pr_metadata SET provider_pr_id = '77' WHERE task_id = ?")
        .bind(&world.task_id)
        .execute(world.db.pool())
        .await
        .expect("numeric GitHub PR id");
    let supersession = superseded(&world).await;
    let mock = mock_github(
        state,
        merged,
        &supersession.superseded_head_sha,
        &format!("task/{}", &world.task_id[..8]),
    )
    .await;
    let client = GitHubClient::new_unvalidated(&mock.base_url, Secret::new(TOKEN.to_owned()))
        .expect("GitHub client");
    let provider = Arc::new(GitHubPrProvider::new(client));
    let watcher = PrReworkWatcher::new(Arc::clone(&world.db), Arc::clone(&world.bus))
        .with_provider_for_test(provider);
    (world, supersession, mock, watcher)
}

#[tokio::test]
async fn pr_rework_watcher_observes_open_pr_read_only() {
    let (world, supersession, mock, watcher) = watcher_rig("open", false).await;
    let stats = watcher.watch_once().await.expect("watch");
    assert_eq!(stats.observed, 1);
    assert_eq!(stats.remote_merged, 0);
    assert_eq!(stats.remote_closed, 0);
    assert_eq!(
        PrReworkRepo::get_supersession(&*world.db, &supersession.id)
            .await
            .expect("supersession lookup")
            .expect("supersession")
            .state,
        PrReworkState::Open
    );
    let requests = mock.requests.lock().expect("requests");
    assert_eq!(requests.len(), 1);
    assert!(requests[0].line.starts_with("GET "));
    assert!(requests[0].line.contains("/repos/acme/widgets/pulls/77"));
    assert!(requests[0]
        .headers
        .to_ascii_lowercase()
        .contains("authorization: bearer"));
    assert!(requests[0].headers.contains(TOKEN));
    assert!(!requests[0].line.contains(TOKEN));
}

#[tokio::test]
async fn pr_rework_watcher_merged_pr_waits_for_rt_decision_scanner() {
    let (world, supersession, mock, watcher) = watcher_rig("closed", true).await;
    let stats = watcher.watch_once().await.expect("watch");
    assert_eq!(stats.remote_merged, 1);
    let merged = PrReworkRepo::get_supersession(&*world.db, &supersession.id)
        .await
        .expect("supersession lookup")
        .expect("supersession");
    assert_eq!(merged.state, PrReworkState::RemoteMerged);
    let decision_id = merged
        .convergence_decision_id
        .as_deref()
        .expect("convergence decision");
    let decision = HumanDecisionRepo::get_human_decision_request(&*world.db, decision_id)
        .await
        .expect("decision lookup")
        .expect("decision");
    assert_eq!(decision.state, db::HumanDecisionState::Open);
    // RT's answered-decision scanner is the sole owner of convergence. The
    // watcher records status and raises the exact Decision, then stops.
    assert_eq!(
        world
            .service
            .converge_remote_merged(&supersession.id)
            .await
            .expect("unanswered decision remains open"),
        crate::pr_rework::ConvergeOutcome::AwaitingDecision
    );
    assert!(mock
        .requests
        .lock()
        .expect("requests")
        .iter()
        .all(|request| request.line.starts_with("GET ")));
}

#[tokio::test]
async fn pr_rework_watcher_closed_pr_marks_remote_closed_without_writing_provider() {
    let (world, supersession, mock, watcher) = watcher_rig("closed", false).await;
    let stats = watcher.watch_once().await.expect("watch");
    assert_eq!(stats.remote_closed, 1);
    assert_eq!(
        PrReworkRepo::get_supersession(&*world.db, &supersession.id)
            .await
            .expect("supersession lookup")
            .expect("supersession")
            .state,
        PrReworkState::RemoteClosed
    );
    assert!(mock
        .requests
        .lock()
        .expect("requests")
        .iter()
        .all(|request| request.line.starts_with("GET ")));
}
