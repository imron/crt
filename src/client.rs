//! Async client for connecting to the crt server.
//!
//! Provides typed methods for all server API calls, hiding JSON-RPC details.

use std::collections::{BTreeSet, HashMap};
use std::io;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::header::CONTENT_TYPE;
use hyper::{Method, Request};
use hyper_util::rt::TokioIo;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpStream, UnixStream};
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::core::ConnectionState;
use crate::protocol::{JsonRpcCall, RpcMethod};
use crate::review_types;

/// Result of the `init` call. Alias for [`review_types::ConnectionContext`].
pub type InitResult = review_types::ConnectionContext;

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// A server-to-client notification.
pub type Notification = crate::protocol::Notification;
type PendingResponses =
    Arc<Mutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, String>>>>>;

#[derive(Debug, Clone, Copy, Serialize)]
struct EmptyParams {}

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
    cleanup_dir: Option<PathBuf>,
    allow_embedded_start: bool,
    options: ReconnectOptions,
}

enum ClientConnection {
    Unix(UnixClientConnection),
    Http(HttpClientConnection),
}

struct UnixClientConnection {
    writer: tokio::net::unix::OwnedWriteHalf,
    pending: PendingResponses,
    _reader_task: JoinHandle<()>,
}

struct HttpClientConnection {
    host: String,
    sender: hyper::client::conn::http1::SendRequest<Full<Bytes>>,
    _connection_task: JoinHandle<()>,
}

impl UnixClientConnection {
    async fn connect(
        socket_path: &Path,
        notify_tx: tokio::sync::mpsc::UnboundedSender<Notification>,
    ) -> Result<Self> {
        let stream = UnixStream::connect(socket_path).await.with_context(|| {
            format!(
                "Could not connect to server at {}. Is `crt server` running?",
                socket_path.display()
            )
        })?;

        let (read_half, write_half) = stream.into_split();
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let reader_task = spawn_unix_reader(read_half, Arc::clone(&pending), notify_tx);
        Ok(Self {
            writer: write_half,
            pending,
            _reader_task: reader_task,
        })
    }
}

impl HttpClientConnection {
    async fn connect(host: &str, port: u16) -> Result<Self> {
        let addr = format!("{host}:{port}");
        Self::connect_addr(&addr).await
    }

    async fn connect_addr(addr: &str) -> Result<Self> {
        let stream = TcpStream::connect(&addr)
            .await
            .with_context(|| format!("Could not connect to crt HTTP server at {addr}"))?;
        let io = TokioIo::new(stream);
        let (sender, connection) = hyper::client::conn::http1::handshake(io)
            .await
            .with_context(|| format!("Failed HTTP handshake with crt server at {addr}"))?;
        let connection_task = tokio::spawn(async move {
            let _ = connection.await;
        });

        Ok(Self {
            host: addr.to_string(),
            sender,
            _connection_task: connection_task,
        })
    }
}

#[derive(Debug, Clone)]
struct InitArgs {
    worktree: String,
    base_ref: String,
    root: bool,
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
    /// Channel for server-pushed notifications received from the Unix transport.
    notify_tx: tokio::sync::mpsc::UnboundedSender<Notification>,
    notify_rx: Mutex<tokio::sync::mpsc::UnboundedReceiver<Notification>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentScope {
    CurrentUnresolved,
    CurrentWithResolved,
    PreviousBasesUnresolved,
}

impl CommentScope {
    fn include_resolved(self) -> bool {
        matches!(self, Self::CurrentWithResolved)
    }

    fn include_previous_bases(self) -> bool {
        matches!(self, Self::PreviousBasesUnresolved)
    }
}

impl Client {
    /// Connect to a running server at the given socket path.
    pub async fn connect(socket_path: &Path) -> Result<Self> {
        let (notify_tx, notify_rx) = tokio::sync::mpsc::unbounded_channel();
        let connection = ClientConnection::Unix(
            UnixClientConnection::connect(socket_path, notify_tx.clone()).await?,
        );
        Ok(Self::new(
            Some(connection),
            None,
            None,
            ConnectionState::Connected,
            notify_tx,
            notify_rx,
        ))
    }

    /// Connect to a running server through the localhost HTTP JSON-RPC
    /// transport. This is primarily for MCP hosts that sandbox Unix sockets
    /// but allow outbound localhost HTTP.
    pub async fn connect_http(host: &str, port: u16) -> Result<Self> {
        let (notify_tx, notify_rx) = tokio::sync::mpsc::unbounded_channel();
        let connection = ClientConnection::Http(HttpClientConnection::connect(host, port).await?);
        Ok(Self::new(
            Some(connection),
            None,
            None,
            ConnectionState::Connected,
            notify_tx,
            notify_rx,
        ))
    }

    pub async fn connect_http_from_env() -> Result<Self> {
        let host = std::env::var(crate::server::ENV_HTTP_HOST)
            .unwrap_or_else(|_| crate::server::DEFAULT_HTTP_HOST.to_string());
        let port = std::env::var(crate::server::ENV_HTTP_PORT)
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(crate::server::DEFAULT_HTTP_PORT);
        Self::connect_http(&host, port).await
    }

    /// Connect to a running server or start an embedded server if needed.
    ///
    /// Non-standalone clients use the shared default socket path, so a server
    /// started here is visible to peer clients. Standalone clients use a
    /// process-private socket under the system temp directory; it still uses
    /// the same socket transport but cannot be discovered through the default
    /// socket path. Any embedded server started by this client is
    /// process-coupled: explicit async shutdown cancels it, and `Drop` only
    /// provides fallback best-effort cancellation.
    pub async fn connect_or_start(socket_path: &Path, standalone: bool) -> Result<Self> {
        Self::connect_or_start_with_options(socket_path, standalone, ReconnectOptions::default())
            .await
    }

    pub async fn connect_or_start_with_options(
        socket_path: &Path,
        standalone: bool,
        options: ReconnectOptions,
    ) -> Result<Self> {
        let (socket_path, cleanup_dir) = if standalone {
            let dir = std::env::temp_dir().join(format!("crt-{}", std::process::id()));
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("Failed to create temp dir {}", dir.display()))?;
            (dir.join("server.sock"), Some(dir))
        } else {
            (socket_path.to_path_buf(), None)
        };

        let policy = ReconnectPolicy {
            socket_path: socket_path.clone(),
            cleanup_dir,
            allow_embedded_start: true,
            options,
        };

        let (notify_tx, notify_rx) = tokio::sync::mpsc::unbounded_channel();
        match UnixClientConnection::connect(&socket_path, notify_tx.clone()).await {
            Ok(connection) if !standalone => Ok(Self::new(
                Some(ClientConnection::Unix(connection)),
                Some(policy),
                None,
                ConnectionState::Connected,
                notify_tx,
                notify_rx,
            )),
            _ => {
                let token = crate::server::start_embedded(&socket_path).await?;
                tokio::time::sleep(policy.options.embedded_startup_delay).await;
                let connection = UnixClientConnection::connect(&socket_path, notify_tx.clone())
                    .await
                    .context("Failed to connect to embedded server")?;
                Ok(Self::new(
                    Some(ClientConnection::Unix(connection)),
                    Some(policy),
                    Some(token),
                    ConnectionState::Connected,
                    notify_tx,
                    notify_rx,
                ))
            }
        }
    }

    fn new(
        connection: Option<ClientConnection>,
        reconnect_policy: Option<ReconnectPolicy>,
        embedded_server: Option<CancellationToken>,
        connection_state: ConnectionState,
        notify_tx: tokio::sync::mpsc::UnboundedSender<Notification>,
        notify_rx: tokio::sync::mpsc::UnboundedReceiver<Notification>,
    ) -> Self {
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
        self.init_with_root(worktree, base_ref, false).await
    }

    pub async fn init_root(&self, worktree: &str) -> Result<InitResult> {
        self.init_with_root(worktree, "", true).await
    }

    async fn init_with_root(
        &self,
        worktree: &str,
        base_ref: &str,
        root: bool,
    ) -> Result<InitResult> {
        let result = self
            .call(
                RpcMethod::Init,
                review_types::InitParams {
                    worktree: worktree.to_string(),
                    base_ref: base_ref.to_string(),
                    root,
                },
            )
            .await?;
        *self.init_args.lock().await = Some(InitArgs {
            worktree: worktree.to_string(),
            base_ref: base_ref.to_string(),
            root,
        });
        Ok(result)
    }

    async fn reinit(&self, init_args: &InitArgs) -> Result<InitResult> {
        self.call_once(
            RpcMethod::Init,
            review_types::InitParams {
                worktree: init_args.worktree.clone(),
                base_ref: init_args.base_ref.clone(),
                root: init_args.root,
            },
        )
        .await
    }

    // -----------------------------------------------------------------------
    // Review state (stubs — will return "not implemented" from server)
    // -----------------------------------------------------------------------

    pub async fn list_changed_files(&self) -> Result<review_types::ListChangedFilesResult> {
        self.call(RpcMethod::ListChangedFiles, EmptyParams {}).await
    }

    pub async fn list_file_statuses(&self) -> Result<review_types::ListFileStatusesResult> {
        self.call(RpcMethod::ListFileStatuses, EmptyParams {}).await
    }

    pub async fn get_file_diff(&self, file_path: &str) -> Result<review_types::GetFileDiffResult> {
        self.call(
            RpcMethod::GetFileDiff,
            review_types::GetFileDiffParams {
                file_path: file_path.to_string(),
            },
        )
        .await
    }

    pub async fn get_file_content(
        &self,
        file_path: &str,
        version: review_types::FileVersion,
    ) -> Result<review_types::GetFileContentResult> {
        self.call(
            RpcMethod::GetFileContent,
            review_types::GetFileContentParams {
                file_path: file_path.to_string(),
                version,
            },
        )
        .await
    }

    pub async fn mark_reviewed(&self, file_path: &str) -> Result<review_types::ReviewActionResult> {
        self.call(
            RpcMethod::MarkReviewed,
            review_types::MarkReviewedParams {
                file_path: file_path.to_string(),
            },
        )
        .await
    }

    pub async fn unmark_reviewed(
        &self,
        file_path: &str,
    ) -> Result<review_types::ReviewActionResult> {
        self.call(
            RpcMethod::UnmarkReviewed,
            review_types::UnmarkReviewedParams {
                file_path: file_path.to_string(),
            },
        )
        .await
    }

    pub async fn reset_reviews(&self) -> Result<review_types::ResetReviewsResult> {
        self.call(RpcMethod::ResetReviews, EmptyParams {}).await
    }

    // -----------------------------------------------------------------------
    // Comments
    // -----------------------------------------------------------------------

    pub async fn create_comment(
        &self,
        params: review_types::CreateCommentParams,
    ) -> Result<review_types::CommentResult> {
        self.call(RpcMethod::CreateComment, params).await
    }

    pub async fn list_comments(
        &self,
        file_path: Option<&str>,
        scope: CommentScope,
    ) -> Result<review_types::ListCommentsResult> {
        self.call(
            RpcMethod::ListComments,
            review_types::ListCommentsParams {
                file_path: file_path.map(str::to_string),
                include_resolved: scope.include_resolved(),
                include_previous_bases: scope.include_previous_bases(),
            },
        )
        .await
    }

    pub async fn list_current_and_previous_unresolved_comments(
        &self,
        file_path: Option<&str>,
        current_scope: CommentScope,
    ) -> Result<review_types::ListCommentsResult> {
        if current_scope.include_previous_bases() {
            bail!(
                "current comment scope cannot include previous bases; \
                 previous unresolved comments are appended separately"
            );
        }
        let mut result = self.list_comments(file_path, current_scope).await?;
        let previous_unresolved = self
            .list_comments(file_path, CommentScope::PreviousBasesUnresolved)
            .await?;
        append_unique_comments(&mut result.comments, previous_unresolved.comments);
        Ok(result)
    }

    pub async fn get_comment(&self, id: i64) -> Result<review_types::CommentResult> {
        self.call(RpcMethod::GetComment, review_types::GetCommentParams { id })
            .await
    }

    pub async fn update_comment(&self, id: i64, body: &str) -> Result<review_types::CommentResult> {
        self.call(
            RpcMethod::UpdateComment,
            review_types::UpdateCommentParams {
                id,
                body: body.to_string(),
            },
        )
        .await
    }

    pub async fn resolve_comment(&self, id: i64) -> Result<review_types::CommentResult> {
        self.call(
            RpcMethod::ResolveComment,
            review_types::ResolveCommentParams { id },
        )
        .await
    }

    pub async fn unresolve_comment(&self, id: i64) -> Result<review_types::CommentResult> {
        self.call(
            RpcMethod::UnresolveComment,
            review_types::UnresolveCommentParams { id },
        )
        .await
    }

    pub async fn delete_comment(&self, id: i64) -> Result<review_types::DeleteCommentResult> {
        self.call(
            RpcMethod::DeleteComment,
            review_types::DeleteCommentParams { id },
        )
        .await
    }

    pub async fn list_repos(&self) -> Result<crate::review_types::ListReposResult> {
        self.call(RpcMethod::ListRepos, EmptyParams {}).await
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
                RpcMethod::SearchCodebase,
                review_types::SearchCodebaseParams {
                    pattern: pattern.to_string(),
                    scope: Some(scope.to_string()),
                },
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
                RpcMethod::FindDefinition,
                review_types::FindDefinitionParams {
                    symbol: symbol.to_string(),
                    context_file: context_file.map(str::to_string),
                },
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

    pub async fn shutdown(&self) {
        let mut embedded = self.embedded_server.lock().await;
        if let Some(token) = embedded.take() {
            token.cancel();
            tokio::time::sleep(EMBEDDED_SERVER_STARTUP_DELAY).await;
        }
        if let Some(dir) = self
            .reconnect_policy
            .as_ref()
            .and_then(|policy| policy.cleanup_dir.as_ref())
        {
            let _ = std::fs::remove_dir(dir);
        }
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

    async fn call<R: DeserializeOwned>(
        &self,
        method: RpcMethod,
        params: impl Serialize,
    ) -> Result<R> {
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
        method: RpcMethod,
        params: impl Serialize,
    ) -> Result<R> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let request = JsonRpcCall {
            jsonrpc: "2.0",
            method,
            params,
            id,
        };

        let mut line = serde_json::to_string(&request).context("Failed to serialize request")?;
        line.push('\n');

        let mut connection_guard = self.connection.lock().await;
        let connection = connection_guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Transient transport error: reconnecting"))?;

        let response = match connection {
            ClientConnection::Unix(connection) => {
                unix_call(connection, id, line.as_bytes()).await?
            }
            ClientConnection::Http(connection) => match http_call(connection, &request).await {
                Ok(response) => response,
                Err(error) if is_transport_loss(&error) => {
                    let host = connection.host.clone();
                    *connection = HttpClientConnection::connect_addr(&host).await?;
                    http_call(connection, &request).await?
                }
                Err(error) => return Err(error),
            },
        };

        // Check for error
        if let Some(error) = response.get("error") {
            let code = error.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
            let message = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");
            bail!(
                "Server error ({}): {} [method: {}]",
                code,
                message,
                method.as_str()
            );
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
        let connection = UnixClientConnection::connect(socket_path, self.notify_tx.clone()).await?;
        *self.connection.lock().await = Some(ClientConnection::Unix(connection));
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

async fn unix_call(
    connection: &mut UnixClientConnection,
    id: u64,
    request_line: &[u8],
) -> Result<serde_json::Value> {
    let (response_tx, response_rx) = oneshot::channel();
    connection.pending.lock().await.insert(id, response_tx);

    if let Err(e) = connection.writer.write_all(request_line).await {
        remove_pending_response(&connection.pending, id).await;
        return Err(e).context("Failed to write to server");
    }
    if let Err(e) = connection.writer.flush().await {
        remove_pending_response(&connection.pending, id).await;
        return Err(e).context("Failed to flush to server");
    }

    match response_rx.await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(message)) => bail!("{message}"),
        Err(_) => bail!("Server closed the connection"),
    }
}

fn spawn_unix_reader(
    read_half: tokio::net::unix::OwnedReadHalf,
    pending: PendingResponses,
    notify_tx: tokio::sync::mpsc::UnboundedSender<Notification>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut reader = BufReader::new(read_half);
        loop {
            let mut response_line = String::new();
            let read = reader.read_line(&mut response_line).await;
            let n = match read {
                Ok(n) => n,
                Err(e) => {
                    fail_pending_responses(&pending, format!("Failed to read from server: {e}"))
                        .await;
                    break;
                }
            };
            if n == 0 {
                fail_pending_responses(&pending, "Server closed the connection".to_string()).await;
                break;
            }

            let msg: serde_json::Value = match serde_json::from_str(response_line.trim()) {
                Ok(msg) => msg,
                Err(e) => {
                    fail_pending_responses(
                        &pending,
                        format!("Failed to parse server message: {e}"),
                    )
                    .await;
                    break;
                }
            };

            if msg.get("method").is_some() && msg.get("id").is_none() {
                if let Some(params) = msg.get("params")
                    && let Ok(notif) = serde_json::from_value::<Notification>(params.clone())
                {
                    let _ = notify_tx.send(notif);
                }
                continue;
            }

            if let Some(id) = msg.get("id").and_then(|id| id.as_u64())
                && let Some(response_tx) = pending.lock().await.remove(&id)
            {
                let _ = response_tx.send(Ok(msg));
            }
        }
    })
}

async fn fail_pending_responses(pending: &PendingResponses, message: String) {
    let pending = std::mem::take(&mut *pending.lock().await);
    for (_, response_tx) in pending {
        let _ = response_tx.send(Err(message.clone()));
    }
}

async fn remove_pending_response(pending: &PendingResponses, id: u64) {
    pending.lock().await.remove(&id);
}

async fn http_call<P: Serialize>(
    connection: &mut HttpClientConnection,
    request: &P,
) -> Result<serde_json::Value> {
    let body = serde_json::to_vec(request).context("Failed to serialize HTTP request")?;
    let http_request = Request::builder()
        .method(Method::POST)
        .uri(format!("http://{}/rpc", connection.host))
        .header(CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(body)))
        .context("Failed to build HTTP request")?;

    let response = connection
        .sender
        .send_request(http_request)
        .await
        .context("Failed to send HTTP request")?;
    let status = response.status();
    if !status.is_success() {
        bail!("HTTP server returned {status}");
    }

    let body = response
        .into_body()
        .collect()
        .await
        .context("Failed to read HTTP response body")?
        .to_bytes();
    serde_json::from_slice(&body).context("Failed to parse HTTP response body")
}

fn is_transport_loss(error: &anyhow::Error) -> bool {
    if error.to_string().contains("Server closed the connection") {
        return true;
    }

    error.chain().any(|cause| {
        if cause
            .downcast_ref::<hyper::Error>()
            .is_some_and(|hyper_error| hyper_error.is_canceled() || hyper_error.is_closed())
        {
            return true;
        }

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

fn append_unique_comments(
    comments: &mut Vec<review_types::Comment>,
    new_comments: Vec<review_types::Comment>,
) {
    let mut seen: BTreeSet<i64> = comments.iter().map(|comment| comment.id).collect();
    for comment in new_comments {
        if seen.insert(comment.id) {
            comments.push(comment);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_types::{
        AnchorAggregateStatus, AnchorMatchMethod, AnchorPlacementStatus, AnchorStatus, Comment,
        CommentAnchor, CommentAnchorSegment, CommentAnchorSide,
    };

    #[test]
    fn append_unique_comments_keeps_current_and_adds_previous_unresolved() {
        let mut comments = vec![comment(1), comment(2)];

        append_unique_comments(&mut comments, vec![comment(2), comment(3)]);

        let ids = comments
            .iter()
            .map(|comment| comment.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    fn comment(id: i64) -> Comment {
        Comment::new(crate::review_types::CommentInit {
            id,
            merge_base: "merge-base".to_string(),
            head_ref: "HEAD".to_string(),
            created_head_commit: "head-commit".to_string(),
            anchor: test_anchor(id),
            body: format!("comment {id}"),
            resolved: false,
            created_at: "2026-07-01T00:00:00+10:00".to_string(),
            updated_at: "2026-07-01T00:00:00+10:00".to_string(),
            anchor_status: AnchorStatus::Anchored,
        })
        .expect("test comment anchor should be valid")
    }

    fn test_anchor(line: i64) -> CommentAnchor {
        CommentAnchor {
            segments: vec![CommentAnchorSegment {
                side: CommentAnchorSide::Head,
                file_path: "src/lib.rs".to_string(),
                line_start: line,
                line_end: line,
                char_start: None,
                char_end: None,
                anchor_text: String::new(),
                context_before: String::new(),
                context_after: String::new(),
                placement_status: AnchorPlacementStatus::Anchored,
                match_method: AnchorMatchMethod::ExactAtLine,
            }],
            aggregate_status: AnchorAggregateStatus::Anchored,
        }
    }
}
