use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crt::client::{Client, ClientEvent, ReconnectOptions};
use crt::core::ConnectionState;
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
        let socket_path = self.socket_path.clone();
        let repo_dir = self.repo_dir.clone();
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
                let result = match request["method"].as_str().unwrap() {
                    "init" => serde_json::json!({
                        "repo_root": repo_dir.clone(),
                        "worktree": repo_dir.clone(),
                        "base_ref": "HEAD",
                        "head_ref": "HEAD",
                        "merge_base": "0000000000000000000000000000000000000000",
                    }),
                    "list_changed_files" => serde_json::json!({ "files": [] }),
                    method => panic!("unexpected fake server method: {method}"),
                };
                let response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "result": result,
                    "id": id,
                });
                let mut response_line = serde_json::to_string(&response).unwrap();
                response_line.push('\n');
                stream
                    .get_mut()
                    .write_all(response_line.as_bytes())
                    .await
                    .unwrap();
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
