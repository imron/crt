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

/// A server-to-client notification.
pub type Notification = crate::protocol::Notification;

/// Async client connected to a crt server.
pub struct Client {
    reader: Mutex<BufReader<tokio::net::unix::OwnedReadHalf>>,
    writer: Mutex<tokio::net::unix::OwnedWriteHalf>,
    next_id: AtomicU64,
    /// Channel for server-pushed notifications received during `call()`.
    notify_tx: tokio::sync::mpsc::UnboundedSender<Notification>,
    notify_rx: Mutex<tokio::sync::mpsc::UnboundedReceiver<Notification>>,
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
        let (notify_tx, notify_rx) = tokio::sync::mpsc::unbounded_channel();

        Ok(Self {
            reader: Mutex::new(BufReader::new(read_half)),
            writer: Mutex::new(write_half),
            next_id: AtomicU64::new(1),
            notify_tx,
            notify_rx: Mutex::new(notify_rx),
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

    pub async fn mark_reviewed(&self, file_path: &str) -> Result<model::ReviewActionResult> {
        self.call(
            "mark_reviewed",
            serde_json::json!({ "file_path": file_path }),
        )
        .await
    }

    pub async fn unmark_reviewed(&self, file_path: &str) -> Result<model::ReviewActionResult> {
        self.call(
            "unmark_reviewed",
            serde_json::json!({ "file_path": file_path }),
        )
        .await
    }

    pub async fn reset_reviews(&self) -> Result<model::ResetReviewsResult> {
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
    // Search
    // -----------------------------------------------------------------------

    pub async fn search_codebase(
        &self,
        pattern: &str,
        scope: &str,
    ) -> Result<crate::model::SearchCodebaseResult> {
        let value = self
            .call(
                "search_codebase",
                serde_json::json!({ "pattern": pattern, "scope": scope }),
            )
            .await?;
        serde_json::from_value(value)
            .map_err(|e| anyhow::anyhow!("Failed to parse search result: {e}"))
    }

    pub async fn find_definition(
        &self,
        symbol: &str,
        context_file: Option<&str>,
    ) -> Result<crate::model::FindDefinitionResult> {
        let value = self
            .call(
                "find_definition",
                serde_json::json!({ "symbol": symbol, "context_file": context_file }),
            )
            .await?;
        serde_json::from_value(value)
            .map_err(|e| anyhow::anyhow!("Failed to parse definition result: {e}"))
    }

    // -----------------------------------------------------------------------
    // Notifications
    // -----------------------------------------------------------------------

    /// Drain all pending server-pushed notifications.
    pub async fn drain_notifications(&self) -> Vec<Notification> {
        let mut rx = self.notify_rx.lock().await;
        let mut out = Vec::new();
        while let Ok(n) = rx.try_recv() {
            out.push(n);
        }
        out
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

        // Read response, routing any interleaved notifications to the
        // notification channel.
        let response: serde_json::Value;
        {
            let mut reader = self.reader.lock().await;
            loop {
                let mut response_line = String::new();
                let n = reader
                    .read_line(&mut response_line)
                    .await
                    .context("Failed to read from server")?;
                if n == 0 {
                    bail!("Server closed the connection");
                }

                let msg: serde_json::Value = serde_json::from_str(response_line.trim())
                    .context("Failed to parse server message")?;

                // Server-pushed notification: has "method" but no "id".
                if msg.get("method").is_some() && msg.get("id").is_none() {
                    if let Some(params) = msg.get("params") {
                        if let Ok(notif) = serde_json::from_value::<Notification>(params.clone()) {
                            let _ = self.notify_tx.send(notif);
                        }
                    }
                    continue; // keep reading for the actual response
                }

                response = msg;
                break;
            }
        }

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
