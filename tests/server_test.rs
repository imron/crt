use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU16, Ordering};

use crt::client::Client;
use crt::protocol::NotificationKind;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpStream, UnixStream};
use tokio::task::JoinHandle;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

static TEST_SERVER_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static NEXT_HTTP_TEST_PORT: AtomicU16 = AtomicU16::new(0);

struct TestServer {
    _server_guard: tokio::sync::MutexGuard<'static, ()>,
    _dir: tempfile::TempDir,
    socket_path: PathBuf,
    repo_dir: PathBuf,
    handle: JoinHandle<()>,
}

impl TestServer {
    async fn start() -> Self {
        Self::start_with_http_port(None).await
    }

    async fn start_with_http_port(http_port: Option<u16>) -> Self {
        let server_guard = TEST_SERVER_LOCK.lock().await;
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
        let previous_http_port = std::env::var_os(crt::server::ENV_HTTP_PORT);
        if let Some(port) = http_port {
            unsafe {
                std::env::set_var(crt::server::ENV_HTTP_PORT, port.to_string());
            }
        }
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
        if let Some(port) = http_port {
            let mut http_ready = false;
            for _ in 0..50 {
                if TcpStream::connect((crt::server::DEFAULT_HTTP_HOST, port))
                    .await
                    .is_ok()
                {
                    http_ready = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            assert!(http_ready, "Server HTTP listener was not created");
        }
        match previous_http_port {
            Some(value) => unsafe {
                std::env::set_var(crt::server::ENV_HTTP_PORT, value);
            },
            None => unsafe {
                std::env::remove_var(crt::server::ENV_HTTP_PORT);
            },
        }

        Self {
            _server_guard: server_guard,
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

        loop {
            let mut response_line = String::new();
            self.reader.read_line(&mut response_line).await.unwrap();
            let msg: serde_json::Value = serde_json::from_str(&response_line).unwrap();
            if msg.get("method").is_some() && msg.get("id").is_none() {
                continue;
            }
            return msg;
        }
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

fn git_output(path: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn unused_local_port() -> u16 {
    let seed = NEXT_HTTP_TEST_PORT.fetch_add(1, Ordering::Relaxed);
    let start = 20_000 + ((std::process::id() as u16).wrapping_add(seed) % 20_000);
    for offset in 0..1000 {
        let port = start.saturating_add(offset);
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    panic!("could not find an unused localhost port for HTTP test");
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
        .request(
            "get_file_content",
            serde_json::json!({"file_path": "test.rs", "version": "HEAD"}),
        )
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
    assert!(
        resp1.get("error").is_none(),
        "client 1 should have no error"
    );
    assert!(
        resp2.get("error").is_none(),
        "client 2 should have no error"
    );
}

#[tokio::test]
async fn test_list_file_statuses_returns_compact_review_state() {
    let server = TestServer::start().await;
    std::fs::write(server.repo_dir.join("file.txt"), "hello\nworld\n").unwrap();
    let mut conn = server.connect_and_init().await;

    let resp = conn
        .request("list_file_statuses", serde_json::json!({}))
        .await;

    assert!(resp["error"].is_null(), "list failed: {resp}");
    assert_eq!(resp["result"]["total_files"], 1);
    assert_eq!(resp["result"]["reviewed_files"], 0);
    assert_eq!(resp["result"]["unreviewed_files"], 1);
    assert_eq!(resp["result"]["changed_files"], 0);
    let files = resp["result"]["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    let file = &files[0];
    assert_eq!(file["change"]["path"], "file.txt");
    assert_eq!(file["status"]["status"], "unreviewed");
    assert_eq!(file["diff"]["hunks"], 1);
    assert_eq!(file["diff"]["additions"], 1);
    assert_eq!(file["diff"]["deletions"], 0);
    assert_eq!(file["diff"]["is_binary"], false);
    assert!(
        file["diff"]["diff_hash"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    assert!(
        file["diff"].get("lines").is_none(),
        "compact status response should not include hunk lines: {file}"
    );
}

#[tokio::test]
async fn test_client_receives_review_notification_while_idle() {
    let server = TestServer::start().await;
    std::fs::write(server.repo_dir.join("file.txt"), "changed\n").unwrap();

    let watcher = Client::connect(&server.socket_path).await.unwrap();
    watcher
        .init(&server.repo_dir.to_string_lossy(), "HEAD")
        .await
        .unwrap();

    let reviewer = Client::connect(&server.socket_path).await.unwrap();
    reviewer
        .init(&server.repo_dir.to_string_lossy(), "HEAD")
        .await
        .unwrap();
    reviewer.mark_reviewed("file.txt").await.unwrap();

    let mut received = Vec::new();
    for _ in 0..50 {
        received.extend(watcher.drain_notifications().await);
        if !received.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    assert!(
        received.iter().any(|notification| matches!(
            &notification.kind,
            NotificationKind::ReviewChanged { file_path } if file_path == "file.txt"
        )),
        "watcher did not receive review notification: {received:?}"
    );
}

#[tokio::test]
async fn test_http_client_lists_repos() {
    let port = unused_local_port();
    let server = TestServer::start_with_http_port(Some(port)).await;

    let client = {
        let mut client = None;
        for _ in 0..50 {
            match Client::connect_http(crt::server::DEFAULT_HTTP_HOST, port).await {
                Ok(connected) => {
                    client = Some(connected);
                    break;
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
            }
        }
        client.expect("HTTP client should connect")
    };

    client
        .init(&server.repo_dir.to_string_lossy(), "HEAD")
        .await
        .unwrap();

    let repos = client.list_repos().await.unwrap();
    assert_eq!(repos.sessions.len(), 1);
    assert_eq!(
        repos.sessions[0].worktree,
        server.repo_dir.to_string_lossy()
    );
}

#[tokio::test]
async fn test_comment_lifecycle() {
    let server = TestServer::start().await;
    let mut conn = server.connect_and_init().await;

    let create = conn
        .request(
            "create_comment",
            serde_json::json!({
                "file_path": "file.txt",
                "line_start": 1,
                "line_end": 1,
                "char_start": null,
                "char_end": null,
                "anchor_text": "hello",
                "context_before": "",
                "context_after": "",
                "body": "check this",
            }),
        )
        .await;
    assert!(create["error"].is_null(), "create failed: {create}");
    let comment = &create["result"]["comment"];
    let id = comment["id"].as_i64().unwrap();
    assert_eq!(comment["file_path"], "file.txt");
    assert_eq!(comment["body"], "check this");
    assert_eq!(comment["resolved"], false);
    assert_eq!(comment["anchor_status"], "anchored");

    let list = conn
        .request(
            "list_comments",
            serde_json::json!({
                "file_path": "file.txt",
                "include_resolved": false,
            }),
        )
        .await;
    assert!(list["error"].is_null(), "list failed: {list}");
    let comments = list["result"]["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0]["id"], id);

    let update = conn
        .request(
            "update_comment",
            serde_json::json!({
                "id": id,
                "body": "updated body",
            }),
        )
        .await;
    assert!(update["error"].is_null(), "update failed: {update}");
    assert_eq!(update["result"]["comment"]["body"], "updated body");

    let detail = conn
        .request("get_comment", serde_json::json!({ "id": id }))
        .await;
    assert!(detail["error"].is_null(), "get failed: {detail}");
    assert_eq!(detail["result"]["comment"]["context_before"], "");
    assert_eq!(detail["result"]["comment"]["context_after"], "");

    let resolved = conn
        .request("resolve_comment", serde_json::json!({ "id": id }))
        .await;
    assert!(resolved["error"].is_null(), "resolve failed: {resolved}");
    assert_eq!(resolved["result"]["comment"]["resolved"], true);

    let unresolved_only = conn
        .request(
            "list_comments",
            serde_json::json!({
                "file_path": "file.txt",
                "include_resolved": false,
            }),
        )
        .await;
    assert!(
        unresolved_only["error"].is_null(),
        "list unresolved failed: {unresolved_only}"
    );
    assert_eq!(
        unresolved_only["result"]["comments"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    let all = conn
        .request(
            "list_comments",
            serde_json::json!({
                "file_path": "file.txt",
                "include_resolved": true,
            }),
        )
        .await;
    assert!(all["error"].is_null(), "list all failed: {all}");
    assert_eq!(all["result"]["comments"].as_array().unwrap().len(), 1);

    let unresolved = conn
        .request("unresolve_comment", serde_json::json!({ "id": id }))
        .await;
    assert!(
        unresolved["error"].is_null(),
        "unresolve failed: {unresolved}"
    );
    assert_eq!(unresolved["result"]["comment"]["resolved"], false);

    let deleted = conn
        .request("delete_comment", serde_json::json!({ "id": id }))
        .await;
    assert!(deleted["error"].is_null(), "delete failed: {deleted}");
    assert_eq!(deleted["result"]["deleted"], true);

    let after_delete = conn
        .request(
            "list_comments",
            serde_json::json!({
                "file_path": "file.txt",
                "include_resolved": true,
            }),
        )
        .await;
    assert!(
        after_delete["error"].is_null(),
        "list after delete failed: {after_delete}"
    );
    assert_eq!(
        after_delete["result"]["comments"].as_array().unwrap().len(),
        0
    );
}

#[tokio::test]
async fn test_explicit_commit_base_does_not_migrate_reviews() {
    let server = TestServer::start().await;
    let head_ref = git_output(&server.repo_dir, &["branch", "--show-current"]);

    let old_base = git_output(&server.repo_dir, &["rev-parse", "HEAD"]);

    std::fs::write(server.repo_dir.join("a.txt"), "a\n").unwrap();
    run_git(&server.repo_dir, &["add", "-A"]);
    run_git(&server.repo_dir, &["commit", "-m", "A"]);
    let explicit_base = git_output(&server.repo_dir, &["rev-parse", "HEAD"]);

    std::fs::write(server.repo_dir.join("b.txt"), "b\n").unwrap();
    run_git(&server.repo_dir, &["add", "-A"]);
    run_git(&server.repo_dir, &["commit", "-m", "B"]);
    let head = git_output(&server.repo_dir, &["rev-parse", "HEAD"]);

    let crt_dir = server.repo_dir.join(".crt");
    std::fs::create_dir_all(&crt_dir).unwrap();
    let db = crt::db::Database::open(&crt_dir.join("reviews.db")).unwrap();
    db.store_review(&old_base, &head_ref, "b.txt", "old-diff-hash", &head)
        .unwrap();

    let mut conn = server.connect().await;
    let resp = conn
        .request(
            "init",
            serde_json::json!({
                "worktree": server.repo_dir.to_string_lossy(),
                "base_ref": explicit_base,
            }),
        )
        .await;
    assert!(resp["error"].is_null(), "init failed: {resp}");

    let resp = conn
        .request("list_changed_files", serde_json::json!({}))
        .await;
    assert!(resp["error"].is_null(), "list failed: {resp}");

    let files = resp["result"]["files"].as_array().unwrap();
    let b = files
        .iter()
        .find(|file| file["change"]["path"] == "b.txt")
        .expect("b.txt should be in explicit commit review range");
    assert_eq!(b["status"]["status"], "unreviewed");

    let current_scope_reviews = db.load_reviews(&explicit_base, &head_ref).unwrap();
    assert!(
        current_scope_reviews.is_empty(),
        "explicit commit base should not receive migrated review rows"
    );
    let old_scope_reviews = db.load_reviews(&old_base, &head_ref).unwrap();
    assert_eq!(
        old_scope_reviews.len(),
        1,
        "old review scope should be preserved when migration is skipped"
    );
}

#[tokio::test]
async fn test_migrated_changed_review_stays_changed() {
    let server = TestServer::start().await;
    let head_ref = git_output(&server.repo_dir, &["branch", "--show-current"]);

    let old_base = git_output(&server.repo_dir, &["rev-parse", "HEAD"]);

    std::fs::write(server.repo_dir.join("b.txt"), "reviewed version\n").unwrap();
    run_git(&server.repo_dir, &["add", "-A"]);
    run_git(&server.repo_dir, &["commit", "-m", "reviewed"]);
    run_git(&server.repo_dir, &["tag", "newbase"]);
    let reviewed_commit = git_output(&server.repo_dir, &["rev-parse", "HEAD"]);

    let repo = crt::git::Repo::open(&server.repo_dir).unwrap();
    let reviewed_diff = repo
        .diff_file(&old_base, &reviewed_commit, "b.txt")
        .unwrap();

    std::fs::write(server.repo_dir.join("b.txt"), "changed after review\n").unwrap();
    run_git(&server.repo_dir, &["add", "-A"]);
    run_git(&server.repo_dir, &["commit", "-m", "changed"]);

    let crt_dir = server.repo_dir.join(".crt");
    std::fs::create_dir_all(&crt_dir).unwrap();
    let db = crt::db::Database::open(&crt_dir.join("reviews.db")).unwrap();
    db.store_review(
        &old_base,
        &head_ref,
        "b.txt",
        &reviewed_diff.diff_hash,
        &reviewed_commit,
    )
    .unwrap();

    let mut conn = server.connect().await;
    let resp = conn
        .request(
            "init",
            serde_json::json!({
                "worktree": server.repo_dir.to_string_lossy(),
                "base_ref": "newbase",
            }),
        )
        .await;
    assert!(resp["error"].is_null(), "init failed: {resp}");

    let resp = conn
        .request("list_changed_files", serde_json::json!({}))
        .await;
    assert!(resp["error"].is_null(), "list failed: {resp}");

    let files = resp["result"]["files"].as_array().unwrap();
    let b = files
        .iter()
        .find(|file| file["change"]["path"] == "b.txt")
        .expect("b.txt should still be in review range");
    assert_eq!(b["status"]["status"], "changed");
}

#[tokio::test]
async fn test_list_repos_reports_active_initialized_sessions() {
    let server = TestServer::start().await;
    let mut discovery = server.connect().await;

    let resp = discovery.request("list_repos", serde_json::json!({})).await;
    assert!(
        resp["error"].is_null(),
        "list_repos before init failed: {resp}"
    );
    assert_eq!(resp["result"]["sessions"].as_array().unwrap().len(), 0);

    let initialized = server.connect_and_init().await;

    let resp = discovery.request("list_repos", serde_json::json!({})).await;
    assert!(
        resp["error"].is_null(),
        "list_repos after init failed: {resp}"
    );
    let sessions = resp["result"]["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0]["worktree"].as_str(),
        Some(server.repo_dir.to_string_lossy().as_ref())
    );
    assert_eq!(sessions[0]["base_ref"], "HEAD");
    assert_eq!(sessions[0]["client_count"], 1);

    drop(initialized);
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let resp = discovery.request("list_repos", serde_json::json!({})).await;
        let sessions = resp["result"]["sessions"].as_array().unwrap();
        if sessions.is_empty() {
            return;
        }
    }

    panic!("initialized session was not unregistered after disconnect");
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
