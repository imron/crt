use std::path::{Path, PathBuf};
use std::process::Command;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::task::JoinHandle;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

struct TestServer {
    _dir: tempfile::TempDir,
    socket_path: PathBuf,
    repo_dir: PathBuf,
    handle: JoinHandle<()>,
}

impl TestServer {
    async fn start() -> Self {
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

        let sp = socket_path.clone();
        let handle = tokio::spawn(async move {
            let _ = crt::server::run_persistent(&sp).await;
        });

        // Wait for socket
        for _ in 0..50 {
            if socket_path.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(socket_path.exists(), "Server socket was not created");

        Self {
            _dir: dir,
            socket_path,
            repo_dir,
            handle,
        }
    }

    async fn connect(&self) -> ClientConn {
        let stream = UnixStream::connect(&self.socket_path).await.unwrap();
        ClientConn {
            reader: BufReader::new(stream),
            next_id: 1,
        }
    }

    /// Connect and send init for the test repo.
    async fn connect_and_init(&self) -> ClientConn {
        let mut conn = self.connect().await;
        let resp = conn
            .request(
                "init",
                serde_json::json!({
                    "worktree": self.repo_dir.to_string_lossy(),
                    "base_ref": "HEAD",
                }),
            )
            .await;
        assert!(resp["error"].is_null(), "init failed: {resp}");
        conn
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

struct ClientConn {
    reader: BufReader<UnixStream>,
    next_id: u64,
}

impl ClientConn {
    async fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": id,
        });
        let mut line = serde_json::to_string(&request).unwrap();
        line.push('\n');
        self.reader
            .get_mut()
            .write_all(line.as_bytes())
            .await
            .unwrap();

        let mut response_line = String::new();
        self.reader.read_line(&mut response_line).await.unwrap();
        serde_json::from_str(&response_line).unwrap()
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_init_resolves_context() {
    let server = TestServer::start().await;
    let mut conn = server.connect().await;

    let resp = conn
        .request(
            "init",
            serde_json::json!({
                "worktree": server.repo_dir.to_string_lossy(),
                "base_ref": "HEAD",
            }),
        )
        .await;

    assert!(resp["error"].is_null(), "init should succeed: {resp}");
    let result = &resp["result"];
    assert!(result["head_ref"].as_str().is_some());
    assert_eq!(result["base_ref"], "HEAD");
    assert!(result["repo_root"].as_str().is_some_and(|s| !s.is_empty()));
    // merge_base should be a 40-char hex hash
    let mb = result["merge_base"].as_str().unwrap_or("");
    assert_eq!(
        mb.len(),
        40,
        "merge_base should be a full commit hash: {mb}"
    );
}

#[tokio::test]
async fn test_init_bad_base_ref() {
    let server = TestServer::start().await;
    let mut conn = server.connect().await;

    let resp = conn
        .request(
            "init",
            serde_json::json!({
                "worktree": server.repo_dir.to_string_lossy(),
                "base_ref": "nonexistent-xyz",
            }),
        )
        .await;

    assert!(resp["error"].is_object(), "should return error");
    let msg = resp["error"]["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("nonexistent-xyz"),
        "error should mention the bad ref: {msg}"
    );
}

#[tokio::test]
async fn test_request_before_init() {
    let server = TestServer::start().await;
    let mut conn = server.connect().await;

    let resp = conn
        .request("list_changed_files", serde_json::json!({}))
        .await;

    assert_eq!(resp["error"]["code"], -32000);
    assert!(
        resp["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("init")
    );
}

#[tokio::test]
async fn test_unknown_method() {
    let server = TestServer::start().await;
    let mut conn = server.connect_and_init().await;

    let resp = conn.request("bogus_method", serde_json::json!({})).await;

    assert_eq!(resp["error"]["code"], -32601);
}

#[tokio::test]
async fn test_stub_method() {
    let server = TestServer::start().await;
    let mut conn = server.connect_and_init().await;

    // Use a method that is still a stub (not yet implemented).
    let resp = conn
        .request("mark_reviewed", serde_json::json!({"file_path": "test.rs"}))
        .await;

    assert_eq!(resp["error"]["code"], -32001);
    assert!(
        resp["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("not yet implemented")
    );
}

#[tokio::test]
async fn test_multiple_clients() {
    let server = TestServer::start().await;

    let mut conn1 = server.connect_and_init().await;
    let mut conn2 = server.connect_and_init().await;

    // Both should be able to send requests independently.
    // list_changed_files is now implemented and returns a result.
    let resp1 = conn1
        .request("list_changed_files", serde_json::json!({}))
        .await;
    let resp2 = conn2
        .request("list_changed_files", serde_json::json!({}))
        .await;

    // Both should succeed (result is present, no error).
    assert!(resp1.get("result").is_some(), "client 1 should get result");
    assert!(resp2.get("result").is_some(), "client 2 should get result");
    assert!(resp1.get("error").is_none(), "client 1 should have no error");
    assert!(resp2.get("error").is_none(), "client 2 should have no error");
}

#[tokio::test]
async fn test_stale_socket_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("test.sock");

    // Create a stale socket file (regular file, not a real socket)
    std::fs::write(&socket_path, "stale").unwrap();
    assert!(socket_path.exists());

    let repo_dir = dir.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    run_git(&repo_dir, &["init"]);
    run_git(&repo_dir, &["config", "user.email", "test@test.com"]);
    run_git(&repo_dir, &["config", "user.name", "Test"]);
    std::fs::write(repo_dir.join("file.txt"), "hello\n").unwrap();
    run_git(&repo_dir, &["add", "-A"]);
    run_git(&repo_dir, &["commit", "-m", "initial"]);

    let sp = socket_path.clone();
    let handle = tokio::spawn(async move {
        let _ = crt::server::run_persistent(&sp).await;
    });

    // Wait for server to start and replace the stale file
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if UnixStream::connect(&socket_path).await.is_ok() {
            break;
        }
    }

    let result = UnixStream::connect(&socket_path).await;
    assert!(result.is_ok(), "should connect after stale socket cleanup");

    handle.abort();
}
