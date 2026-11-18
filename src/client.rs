//! Async client for connecting to the crt server.
//!
//! Provides typed methods for all server API calls, hiding JSON-RPC details.

use std::io;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::core::ConnectionState;
use crate::review_types;

/// Result of the `init` call. Alias for [`review_types::ConnectionContext`].
pub type InitResult = review_types::ConnectionContext;

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// A server-to-client notification.
pub type Notification = crate::protocol::Notification;

const EMBEDDED_SERVER_STARTUP_DELAY: Duration = Duration::from_millis(50);
const DEFAULT_RECONNECT_JITTER: RangeInclusive<Duration> =
    Duration::from_millis(50)..=Duration::from_millis(200);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientEvent {
    ConnectionState(ConnectionState),
}

#[derive(Debug, Clone)]
pub struct ReconnectOptions {
    pub jitter: RangeInclusive<Duration>,
    pub embedded_startup_delay: Duration,
}

impl Default for ReconnectOptions {
    fn default() -> Self {
        Self {
            jitter: DEFAULT_RECONNECT_JITTER,
            embedded_startup_delay: EMBEDDED_SERVER_STARTUP_DELAY,
        }
    }
}

#[derive(Debug, Clone)]
struct ReconnectPolicy {
    socket_path: PathBuf,
    allow_embedded_start: bool,
    options: ReconnectOptions,
}

struct ClientConnection {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

impl ClientConnection {
    async fn connect(socket_path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(socket_path).await.with_context(|| {
            format!(
                "Could not connect to server at {}. Is `crt server` running?",
                socket_path.display()
            )
        })?;

        let (read_half, write_half) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(read_half),
            writer: write_half,
        })
    }
}

#[derive(Debug, Clone)]
struct InitArgs {
    worktree: String,
    base_ref: String,
}

/// Async client connected to a crt server.
pub struct Client {
    connection: Mutex<Option<ClientConnection>>,
    reconnect_policy: Option<ReconnectPolicy>,
    init_args: Mutex<Option<InitArgs>>,
    embedded_server: Mutex<Option<CancellationToken>>,
    connection_state: Mutex<ConnectionState>,
    event_tx: tokio::sync::mpsc::UnboundedSender<ClientEvent>,
    event_rx: Mutex<tokio::sync::mpsc::UnboundedReceiver<ClientEvent>>,
    next_id: AtomicU64,
    /// Channel for server-pushed notifications received during `call()`.
    notify_tx: tokio::sync::mpsc::UnboundedSender<Notification>,
    notify_rx: Mutex<tokio::sync::mpsc::UnboundedReceiver<Notification>>,
}

impl Client {
    /// Connect to a running server at the given socket path.
    pub async fn connect(socket_path: &Path) -> Result<Self> {
        let connection = ClientConnection::connect(socket_path).await?;
        Ok(Self::new(
            Some(connection),
            None,
            None,
            ConnectionState::Connected,
        ))
    }

    /// Connect to a running server or start an embedded server if needed.
    pub async fn connect_or_start(socket_path: &Path, standalone: bool) -> Result<Self> {
        Self::connect_or_start_with_options(socket_path, standalone, ReconnectOptions::default())
            .await
    }

    pub async fn connect_or_start_with_options(
        socket_path: &Path,
        standalone: bool,
        options: ReconnectOptions,
    ) -> Result<Self> {
        let socket_path = if standalone {
            let dir = std::env::temp_dir().join(format!("crt-{}", std::process::id()));
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("Failed to create temp dir {}", dir.display()))?;
            dir.join("server.sock")
        } else {
            socket_path.to_path_buf()
        };

        let policy = ReconnectPolicy {
            socket_path: socket_path.clone(),
            allow_embedded_start: true,
            options,
        };

        match ClientConnection::connect(&socket_path).await {
            Ok(connection) if !standalone => Ok(Self::new(
                Some(connection),
                Some(policy),
                None,
                ConnectionState::Connected,
            )),
            _ => {
                let token = crate::server::start_embedded(&socket_path).await?;
                tokio::time::sleep(policy.options.embedded_startup_delay).await;
                let connection = ClientConnection::connect(&socket_path)
                    .await
                    .context("Failed to connect to embedded server")?;
                Ok(Self::new(
                    Some(connection),
                    Some(policy),
                    Some(token),
                    ConnectionState::Connected,
                ))
            }
        }
    }

    fn new(
        connection: Option<ClientConnection>,
        reconnect_policy: Option<ReconnectPolicy>,
        embedded_server: Option<CancellationToken>,
        connection_state: ConnectionState,
    ) -> Self {
        let (notify_tx, notify_rx) = tokio::sync::mpsc::unbounded_channel();
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();

        Self {
            connection: Mutex::new(connection),
            reconnect_policy,
            init_args: Mutex::new(None),
            embedded_server: Mutex::new(embedded_server),
            connection_state: Mutex::new(connection_state),
            event_tx,
            event_rx: Mutex::new(event_rx),
            next_id: AtomicU64::new(1),
            notify_tx,
            notify_rx: Mutex::new(notify_rx),
        }
    }

    /// Initialize the connection. Must be called before any other method.
    pub async fn init(&self, worktree: &str, base_ref: &str) -> Result<InitResult> {
        let result = self
            .call(
                "init",
                serde_json::json!({
                    "worktree": worktree,
                    "base_ref": base_ref,
                }),
            )
            .await?;
        *self.init_args.lock().await = Some(InitArgs {
            worktree: worktree.to_string(),
            base_ref: base_ref.to_string(),
        });
        Ok(result)
    }

    async fn reinit(&self, init_args: &InitArgs) -> Result<InitResult> {
        self.call_once(
            "init",
            serde_json::json!({
                "worktree": init_args.worktree,
                "base_ref": init_args.base_ref,
            }),
        )
        .await
    }

    // -----------------------------------------------------------------------
    // Review state (stubs — will return "not implemented" from server)
    // -----------------------------------------------------------------------

    pub async fn list_changed_files(&self) -> Result<review_types::ListChangedFilesResult> {
        self.call("list_changed_files", serde_json::json!({})).await
    }

    pub async fn get_file_diff(&self, file_path: &str) -> Result<review_types::GetFileDiffResult> {
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

    pub async fn mark_reviewed(&self, file_path: &str) -> Result<review_types::ReviewActionResult> {
        self.call(
            "mark_reviewed",
            serde_json::json!({ "file_path": file_path }),
        )
        .await
    }

    pub async fn unmark_reviewed(
        &self,
        file_path: &str,
    ) -> Result<review_types::ReviewActionResult> {
        self.call(
            "unmark_reviewed",
            serde_json::json!({ "file_path": file_path }),
        )
        .await
    }

    pub async fn reset_reviews(&self) -> Result<review_types::ResetReviewsResult> {
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
    ) -> Result<crate::review_types::SearchCodebaseResult> {
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
    ) -> Result<crate::review_types::FindDefinitionResult> {
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

    pub async fn drain_events(&self) -> Vec<ClientEvent> {
        let mut rx = self.event_rx.lock().await;
        let mut out = Vec::new();
        while let Ok(event) = rx.try_recv() {
            out.push(event);
        }
        out
    }

    /// Make one reconnect attempt when the client is disconnected.
    ///
    /// This is intentionally bounded so UI loops can call it from a tick
    /// without blocking indefinitely. Retry remains unbounded because each
    /// later tick makes another attempt while the process is alive.
    pub async fn recover_if_disconnected(&self) -> Result<Option<ConnectionState>> {
        if self.is_connected_like().await {
            return Ok(None);
        }

        let Some(policy) = self.reconnect_policy.clone() else {
            return Ok(None);
        };
        let Some(init_args) = self.init_args.lock().await.clone() else {
            return Ok(None);
        };

        tokio::time::sleep(jitter_delay(&policy.options.jitter)).await;

        if self
            .replace_connection_from_path(&policy.socket_path, &init_args)
            .await
            .is_ok()
        {
            self.emit_connection_state(ConnectionState::Reconnected)
                .await;
            self.set_connection_state(ConnectionState::Connected).await;
            return Ok(Some(ConnectionState::Reconnected));
        }

        if policy.allow_embedded_start {
            {
                let mut embedded = self.embedded_server.lock().await;
                if let Some(token) = embedded.take() {
                    token.cancel();
                }
                match crate::server::start_embedded(&policy.socket_path).await {
                    Ok(token) => {
                        *embedded = Some(token);
                    }
                    Err(_) => {
                        return Ok(None);
                    }
                }
            }

            tokio::time::sleep(policy.options.embedded_startup_delay).await;
            if self
                .replace_connection_from_path(&policy.socket_path, &init_args)
                .await
                .is_ok()
            {
                self.emit_connection_state(ConnectionState::Reconnected)
                    .await;
                self.set_connection_state(ConnectionState::Connected).await;
                return Ok(Some(ConnectionState::Reconnected));
            }
        }

        Ok(None)
    }

    // -----------------------------------------------------------------------
    // Generic JSON-RPC call
    // -----------------------------------------------------------------------

    async fn call<R: DeserializeOwned>(&self, method: &str, params: impl Serialize) -> Result<R> {
        if !self.is_connected_like().await {
            bail!("Transient transport error: reconnecting");
        }

        match self.call_once(method, params).await {
            Ok(result) => Ok(result),
            Err(error) if is_transport_loss(&error) => {
                self.mark_reconnecting().await;
                bail!("Transient transport error: reconnecting")
            }
            Err(error) => Err(error),
        }
    }

    async fn call_once<R: DeserializeOwned>(
        &self,
        method: &str,
        params: impl Serialize,
    ) -> Result<R> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": id,
        });

        let mut line = serde_json::to_string(&request).context("Failed to serialize request")?;
        line.push('\n');

        let mut connection_guard = self.connection.lock().await;
        let connection = connection_guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Transient transport error: reconnecting"))?;

        connection
            .writer
            .write_all(line.as_bytes())
            .await
            .context("Failed to write to server")?;
        connection
            .writer
            .flush()
            .await
            .context("Failed to flush to server")?;

        // Read response, routing any interleaved notifications to the
        // notification channel.
        let response: serde_json::Value;
        loop {
            let mut response_line = String::new();
            let n = connection
                .reader
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

    async fn replace_connection_from_path(
        &self,
        socket_path: &Path,
        init_args: &InitArgs,
    ) -> Result<()> {
        let connection = ClientConnection::connect(socket_path).await?;
        *self.connection.lock().await = Some(connection);
        self.reinit(init_args).await?;
        Ok(())
    }

    async fn mark_reconnecting(&self) {
        *self.connection.lock().await = None;
        self.emit_connection_state(ConnectionState::Reconnecting)
            .await;
        self.set_connection_state(ConnectionState::Reconnecting)
            .await;
    }

    async fn set_connection_state(&self, state: ConnectionState) {
        *self.connection_state.lock().await = state;
    }

    async fn emit_connection_state(&self, state: ConnectionState) {
        let _ = self.event_tx.send(ClientEvent::ConnectionState(state));
    }

    async fn is_connected_like(&self) -> bool {
        matches!(
            *self.connection_state.lock().await,
            ConnectionState::Connected | ConnectionState::Reconnected
        )
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Ok(mut embedded) = self.embedded_server.try_lock()
            && let Some(token) = embedded.take()
        {
            token.cancel();
        }
    }
}

fn is_transport_loss(error: &anyhow::Error) -> bool {
    if error.to_string().contains("Server closed the connection") {
        return true;
    }

    error.chain().any(|cause| {
        cause.downcast_ref::<io::Error>().is_some_and(|io_error| {
            matches!(
                io_error.kind(),
                io::ErrorKind::BrokenPipe
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::NotConnected
                    | io::ErrorKind::UnexpectedEof
            )
        })
    })
}

fn jitter_delay(range: &RangeInclusive<Duration>) -> Duration {
    let start = range.start().as_millis() as u64;
    let end = range.end().as_millis() as u64;
    if end <= start {
        return Duration::from_millis(start);
    }

    let span = end - start + 1;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos() as u64)
        .unwrap_or(0);
    Duration::from_millis(start + (nanos % span))
}
