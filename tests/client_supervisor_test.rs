use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use crt::client::{Client, ClientEvent, ReconnectOptions};
use crt::core::ConnectionState;
use crt::protocol::{
    JsonRpcNotification, JsonRpcResponse, Notification, NotificationKind, RpcMethod,
};
use crt::review_types;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::task::JoinHandle;

struct TestRepo {
    _dir: tempfile::TempDir,
    socket_path: PathBuf,
    repo_dir: PathBuf,
}

impl TestRepo {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("test.sock");
        let repo_dir = dir.path().join("repo");
        std::fs::create_dir_all(&repo_dir).unwrap();
        run_git(&repo_dir, &["init"]);
        run_git(&repo_dir, &["config", "user.email", "test@test.com"]);
        run_git(&repo_dir, &["config", "user.name", "Test"]);
        std::fs::write(repo_dir.join("file.txt"), "hello\n").unwrap();
        run_git(&repo_dir, &["add", "-A"]);
        run_git(&repo_dir, &["commit", "-m", "initial"]);

        Self {
            _dir: dir,
            socket_path,
            repo_dir,
        }
    }

    async fn start_fake_server(&self) -> JoinHandle<()> {
        self.start_fake_server_with_init_notification(None).await
    }

    async fn start_fake_server_with_init_notification(
        &self,
        notification_path: Option<&str>,
    ) -> JoinHandle<()> {
        let socket_path = self.socket_path.clone();
        let repo_dir = self.repo_dir.clone();
        let notification_path = notification_path.map(str::to_string);
        let handle = tokio::spawn(async move {
            let _ = std::fs::remove_file(&socket_path);
            let listener = UnixListener::bind(&socket_path).unwrap();
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let repo_dir = repo_dir.to_string_lossy().to_string();

            loop {
                let mut line = String::new();
                if stream.read_line(&mut line).await.unwrap() == 0 {
                    break;
                }
                let request: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
                let id = request["id"].clone();
                let method = request["method"].as_str().unwrap();
                let result = match RpcMethod::from_str(method) {
                    Some(RpcMethod::Init) => {
                        serde_json::to_value(review_types::ConnectionContext {
                            repo_root: repo_dir.clone(),
                            worktree: repo_dir.clone(),
                            base_ref: "HEAD".to_string(),
                            head_ref: "HEAD".to_string(),
                            merge_base: "0000000000000000000000000000000000000000".to_string(),
                        })
                        .unwrap()
                    }
                    Some(RpcMethod::ListChangedFiles) => {
                        serde_json::to_value(review_types::ListChangedFilesResult {
                            files: Vec::new(),
                        })
                        .unwrap()
                    }
                    other => panic!("unexpected fake server method: {method} ({other:?})"),
                };
                let should_notify_after_init = method == RpcMethod::Init.as_str();
                let response = JsonRpcResponse::success(id, result);
                let mut response_line = serde_json::to_string(&response).unwrap();
                response_line.push('\n');
                stream
                    .get_mut()
                    .write_all(response_line.as_bytes())
                    .await
                    .unwrap();
                if should_notify_after_init && let Some(file_path) = notification_path.as_deref() {
                    let notification = JsonRpcNotification {
                        jsonrpc: "2.0",
                        method: RpcMethod::Notification,
                        params: Notification {
                            base_ref: "HEAD".to_string(),
                            head_ref: "HEAD".to_string(),
                            kind: NotificationKind::ReviewChanged {
                                file_path: file_path.to_string(),
                                status: Some(review_types::ReviewStatus::Reviewed {
                                    at: "2026-01-01T00:00:00Z".to_string(),
                                    reviewed_commit: Some("deadbeef".to_string()),
                                }),
                            },
                        },
                    };
                    let mut notification_line = serde_json::to_string(&notification).unwrap();
                    notification_line.push('\n');
                    stream
                        .get_mut()
                        .write_all(notification_line.as_bytes())
                        .await
                        .unwrap();
                }
                stream.get_mut().flush().await.unwrap();
            }
        });

        for _ in 0..50 {
            if self.socket_path.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(self.socket_path.exists(), "server socket was not created");
        handle
    }
}

fn run_git(path: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

#[tokio::test]
async fn reconnects_after_server_death_and_reinitializes() {
    let repo = TestRepo::new();
    let server = repo.start_fake_server().await;
    let client = Client::connect_or_start_with_options(
        &repo.socket_path,
        false,
        ReconnectOptions {
            jitter: Duration::from_millis(0)..=Duration::from_millis(0),
            embedded_startup_delay: Duration::from_millis(10),
        },
    )
    .await
    .unwrap();

    client
        .init(&repo.repo_dir.to_string_lossy(), "HEAD")
        .await
        .unwrap();
    assert!(client.list_changed_files().await.unwrap().files.is_empty());

    server.abort();
    let _ = server.await;
    let failed = client.list_changed_files().await.unwrap_err();
    assert!(failed.to_string().contains("Transient transport error"));
    assert_eq!(
        client.drain_events().await,
        vec![ClientEvent::ConnectionState(ConnectionState::Reconnecting)]
    );

    let mut reconnected = false;
    for _ in 0..20 {
        if client.recover_if_disconnected().await.unwrap() == Some(ConnectionState::Reconnected) {
            reconnected = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert!(reconnected, "client did not reconnect");
    assert_eq!(
        client.drain_events().await,
        vec![ClientEvent::ConnectionState(ConnectionState::Reconnected)]
    );
    assert!(client.list_changed_files().await.unwrap().files.is_empty());
}

#[tokio::test]
async fn reconnect_preserves_notification_delivery() {
    let repo = TestRepo::new();
    let server = repo
        .start_fake_server_with_init_notification(Some("before.txt"))
        .await;
    let client = Client::connect_or_start_with_options(
        &repo.socket_path,
        false,
        ReconnectOptions {
            jitter: Duration::from_millis(0)..=Duration::from_millis(0),
            embedded_startup_delay: Duration::from_millis(10),
        },
    )
    .await
    .unwrap();

    client
        .init(&repo.repo_dir.to_string_lossy(), "HEAD")
        .await
        .unwrap();
    assert_notification_for_path(&client, "before.txt").await;

    server.abort();
    let _ = server.await;
    let failed = client.list_changed_files().await.unwrap_err();
    assert!(failed.to_string().contains("Transient transport error"));

    let server = repo
        .start_fake_server_with_init_notification(Some("after.txt"))
        .await;

    let mut reconnected = false;
    for _ in 0..20 {
        if client.recover_if_disconnected().await.unwrap() == Some(ConnectionState::Reconnected) {
            reconnected = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert!(reconnected, "client did not reconnect");
    assert_notification_for_path(&client, "after.txt").await;
    server.abort();
}

async fn assert_notification_for_path(client: &Client, expected_path: &str) {
    let mut received = Vec::new();
    for _ in 0..20 {
        received.extend(client.drain_notifications().await);
        if received.iter().any(|notification| {
            matches!(
                &notification.kind,
                NotificationKind::ReviewChanged { file_path, .. } if file_path == expected_path
            )
        }) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("did not receive notification for {expected_path}: {received:?}");
}

#[tokio::test]
async fn peer_recovers_when_embedded_owner_shuts_down() {
    let repo = TestRepo::new();
    let options = ReconnectOptions {
        jitter: Duration::from_millis(0)..=Duration::from_millis(0),
        embedded_startup_delay: Duration::from_millis(10),
    };
    let owner = Client::connect_or_start_with_options(&repo.socket_path, false, options.clone())
        .await
        .unwrap();
    owner
        .init(&repo.repo_dir.to_string_lossy(), "HEAD")
        .await
        .unwrap();
    let peer = Client::connect_or_start_with_options(&repo.socket_path, false, options)
        .await
        .unwrap();
    peer.init(&repo.repo_dir.to_string_lossy(), "HEAD")
        .await
        .unwrap();

    assert!(peer.list_changed_files().await.unwrap().files.is_empty());

    owner.shutdown().await;
    let failed = peer.list_changed_files().await.unwrap_err();
    assert!(failed.to_string().contains("Transient transport error"));
    assert_eq!(
        peer.drain_events().await,
        vec![ClientEvent::ConnectionState(ConnectionState::Reconnecting)]
    );

    let mut reconnected = false;
    for _ in 0..20 {
        if peer.recover_if_disconnected().await.unwrap() == Some(ConnectionState::Reconnected) {
            reconnected = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert!(reconnected, "peer did not take over embedded server");
    assert_eq!(
        peer.drain_events().await,
        vec![ClientEvent::ConnectionState(ConnectionState::Reconnected)]
    );
    assert!(peer.list_changed_files().await.unwrap().files.is_empty());
    peer.shutdown().await;
}

#[tokio::test]
async fn simultaneous_peer_recovery_has_one_bind_winner() {
    let repo = TestRepo::new();
    let options = ReconnectOptions {
        jitter: Duration::from_millis(0)..=Duration::from_millis(0),
        embedded_startup_delay: Duration::from_millis(10),
    };
    let owner = Client::connect_or_start_with_options(&repo.socket_path, false, options.clone())
        .await
        .unwrap();
    owner
        .init(&repo.repo_dir.to_string_lossy(), "HEAD")
        .await
        .unwrap();
    let peer_a = Arc::new(
        Client::connect_or_start_with_options(&repo.socket_path, false, options.clone())
            .await
            .unwrap(),
    );
    peer_a
        .init(&repo.repo_dir.to_string_lossy(), "HEAD")
        .await
        .unwrap();
    let peer_b = Arc::new(
        Client::connect_or_start_with_options(&repo.socket_path, false, options)
            .await
            .unwrap(),
    );
    peer_b
        .init(&repo.repo_dir.to_string_lossy(), "HEAD")
        .await
        .unwrap();

    owner.shutdown().await;
    assert!(peer_a.list_changed_files().await.is_err());
    assert!(peer_b.list_changed_files().await.is_err());
    peer_a.drain_events().await;
    peer_b.drain_events().await;

    let recover_a = {
        let peer = Arc::clone(&peer_a);
        tokio::spawn(async move { peer.recover_if_disconnected().await })
    };
    let recover_b = {
        let peer = Arc::clone(&peer_b);
        tokio::spawn(async move { peer.recover_if_disconnected().await })
    };
    let first = recover_a.await.unwrap().unwrap();
    let second = recover_b.await.unwrap().unwrap();

    assert!(
        first == Some(ConnectionState::Reconnected) || second == Some(ConnectionState::Reconnected),
        "expected at least one simultaneous recovery attempt to win the bind, got {first:?} and {second:?}"
    );

    assert!(
        recover_until_reconnected(&peer_a).await,
        "peer A did not reconnect after race"
    );
    assert!(
        recover_until_reconnected(&peer_b).await,
        "peer B did not reconnect after race"
    );
    assert!(peer_a.list_changed_files().await.unwrap().files.is_empty());
    assert!(peer_b.list_changed_files().await.unwrap().files.is_empty());
    peer_a.shutdown().await;
    peer_b.shutdown().await;
}

async fn recover_until_reconnected(client: &Client) -> bool {
    if client.list_changed_files().await.is_ok() {
        return true;
    }

    for _ in 0..20 {
        if client.recover_if_disconnected().await.unwrap() == Some(ConnectionState::Reconnected) {
            return true;
        }
        if client.list_changed_files().await.is_ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}
