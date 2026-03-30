//! Async client for connecting to the crt server.
//!
//! Provides typed methods for all server API calls, hiding JSON-RPC details.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::Mutex;

use crate::model;

/// Result of the `init` call. Alias for [`model::ConnectionContext`].
pub type InitResult = model::ConnectionContext;

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// Async client connected to a crt server.
pub struct Client {
    reader: Mutex<BufReader<tokio::net::unix::OwnedReadHalf>>,
    writer: Mutex<tokio::net::unix::OwnedWriteHalf>,
    next_id: AtomicU64,
}

impl Client {
    /// Connect to a running server at the given socket path.
    pub async fn connect(socket_path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(socket_path).await.with_context(|| {
            format!(
                "Could not connect to server at {}. Is `crt server` running?",
                socket_path.display()
            )
        })?;

        let (read_half, write_half) = stream.into_split();

        Ok(Self {
            reader: Mutex::new(BufReader::new(read_half)),
            writer: Mutex::new(write_half),
            next_id: AtomicU64::new(1),
        })
    }

    /// Initialize the connection. Must be called before any other method.
    pub async fn init(&self, worktree: &str, base_ref: &str) -> Result<InitResult> {
        self.call(
            "init",
            serde_json::json!({
                "worktree": worktree,
                "base_ref": base_ref,
            }),
        )
        .await
    }

    // -----------------------------------------------------------------------
    // Review state (stubs — will return "not implemented" from server)
    // -----------------------------------------------------------------------

    pub async fn list_changed_files(&self) -> Result<model::ListChangedFilesResult> {
        self.call("list_changed_files", serde_json::json!({})).await
    }

    pub async fn get_file_diff(&self, file_path: &str) -> Result<model::GetFileDiffResult> {
        self.call(
            "get_file_diff",
            serde_json::json!({ "file_path": file_path }),
        )
        .await
    }

    pub async fn get_file_content(
        &self,
        file_path: &str,
        version: &str,
    ) -> Result<serde_json::Value> {
        self.call(
            "get_file_content",
            serde_json::json!({ "file_path": file_path, "version": version }),
        )
        .await
    }

    pub async fn mark_reviewed(&self, file_path: &str) -> Result<serde_json::Value> {
        self.call(
            "mark_reviewed",
            serde_json::json!({ "file_path": file_path }),
        )
        .await
    }

    pub async fn unmark_reviewed(&self, file_path: &str) -> Result<serde_json::Value> {
        self.call(
            "unmark_reviewed",
            serde_json::json!({ "file_path": file_path }),
        )
        .await
    }

    pub async fn reset_reviews(&self) -> Result<serde_json::Value> {
        self.call("reset_reviews", serde_json::json!({})).await
    }

    // -----------------------------------------------------------------------
    // Comments (stubs)
    // -----------------------------------------------------------------------

    pub async fn list_comments(
        &self,
        file_path: Option<&str>,
        include_resolved: bool,
    ) -> Result<serde_json::Value> {
        self.call(
            "list_comments",
            serde_json::json!({
                "file_path": file_path,
                "include_resolved": include_resolved,
            }),
        )
        .await
    }

    pub async fn get_comment(&self, id: i64) -> Result<serde_json::Value> {
        self.call("get_comment", serde_json::json!({ "id": id }))
            .await
    }

    pub async fn resolve_comment(&self, id: i64) -> Result<serde_json::Value> {
        self.call("resolve_comment", serde_json::json!({ "id": id }))
            .await
    }

    pub async fn unresolve_comment(&self, id: i64) -> Result<serde_json::Value> {
        self.call("unresolve_comment", serde_json::json!({ "id": id }))
            .await
    }

    // -----------------------------------------------------------------------
    // Search (stubs)
    // -----------------------------------------------------------------------

    pub async fn search_codebase(&self, pattern: &str, scope: &str) -> Result<serde_json::Value> {
        self.call(
            "search_codebase",
            serde_json::json!({ "pattern": pattern, "scope": scope }),
        )
        .await
    }

    pub async fn find_definition(
        &self,
        symbol: &str,
        context_file: Option<&str>,
    ) -> Result<serde_json::Value> {
        self.call(
            "find_definition",
            serde_json::json!({ "symbol": symbol, "context_file": context_file }),
        )
        .await
    }

    // -----------------------------------------------------------------------
    // Generic JSON-RPC call
    // -----------------------------------------------------------------------

    async fn call<R: DeserializeOwned>(&self, method: &str, params: impl Serialize) -> Result<R> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": id,
        });

        let mut line = serde_json::to_string(&request).context("Failed to serialize request")?;
        line.push('\n');

        // Send
        {
            let mut writer = self.writer.lock().await;
            writer
                .write_all(line.as_bytes())
                .await
                .context("Failed to write to server")?;
            writer.flush().await.context("Failed to flush to server")?;
        }

        // Read response
        let mut response_line = String::new();
        {
            let mut reader = self.reader.lock().await;
            let n = reader
                .read_line(&mut response_line)
                .await
                .context("Failed to read from server")?;
            if n == 0 {
                bail!("Server closed the connection");
            }
        }

        let response: serde_json::Value =
            serde_json::from_str(&response_line).context("Failed to parse server response")?;

        // Check for error
        if let Some(error) = response.get("error") {
            let code = error.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
            let message = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");
            bail!("Server error ({}): {} [method: {}]", code, message, method);
        }

        // Extract result
        let result = response
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        serde_json::from_value(result)
            .with_context(|| format!("Failed to deserialize response for '{method}'"))
    }
}
