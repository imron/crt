//! Server core: Unix socket listener, connection handling, JSON-RPC dispatch.
//!
//! The server owns connection lifecycle and method dispatch. Shared wire
//! models live in `crate::protocol` so clients do not depend on server
//! internals.

pub mod api;

use std::collections::HashMap;
use std::convert::Infallible;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::CONTENT_TYPE;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio::sync::{Mutex, broadcast};
use tokio_util::sync::CancellationToken;

use crate::db::Database;
use crate::git::{CommitId, HeadIdentity, ReviewBase};
use crate::protocol::{
    ERR_INTERNAL, ERR_METHOD_NOT_FOUND, ERR_NOT_IMPLEMENTED, ERR_NOT_INITIALIZED, ERR_PARSE,
    JsonRpcRequest, JsonRpcResponse, Notification,
};
use crate::review_types::ActiveReviewSession;

pub const DEFAULT_HTTP_HOST: &str = "127.0.0.1";
pub const DEFAULT_HTTP_PORT: u16 = 25175;
pub const ENV_HTTP_HOST: &str = "CRT_SERVER_HOST";
pub const ENV_HTTP_PORT: &str = "CRT_SERVER_PORT";

// ---------------------------------------------------------------------------
// Connection state
// ---------------------------------------------------------------------------

/// Per-connection state, set during `init`.
#[derive(Debug, Clone)]
pub struct ConnectionContext {
    pub repo_root: PathBuf,
    pub worktree: PathBuf,
    pub base_ref: String,
    pub review_base: ReviewBase,
    /// The merge-base commit hash (stable scope key).
    pub merge_base: CommitId,
    pub head: HeadIdentity,
    pub db_path: PathBuf,
}

impl ConnectionContext {
    pub fn merge_base_key(&self) -> &str {
        self.merge_base.as_ref()
    }

    pub fn head_scope_key(&self) -> String {
        self.head.scope_key()
    }
}

/// Shared state across all connections.
pub struct ServerState {
    /// Open databases keyed by repo root path.
    databases: Mutex<HashMap<PathBuf, Arc<Mutex<Database>>>>,
    active_sessions: Mutex<HashMap<String, ActiveSessionEntry>>,
    /// Broadcast channel for notifications.
    pub notify_tx: broadcast::Sender<Notification>,
}

#[derive(Debug, Clone)]
struct ActiveSessionEntry {
    session: ActiveReviewSession,
    client_count: usize,
}

struct HttpConnectionState {
    conn_ctx: Option<ConnectionContext>,
    conn_db: Option<Arc<Mutex<Database>>>,
}

impl Default for ServerState {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerState {
    pub fn new() -> Self {
        const NOTIFY_CHANNEL_CAPACITY: usize = 64;
        let (notify_tx, _) = broadcast::channel(NOTIFY_CHANNEL_CAPACITY);
        Self {
            databases: Mutex::new(HashMap::new()),
            active_sessions: Mutex::new(HashMap::new()),
            notify_tx,
        }
    }

    /// Get or open the database for a repo.
    pub async fn get_db(&self, db_path: &Path) -> Result<Arc<Mutex<Database>>> {
        let mut dbs = self.databases.lock().await;
        if let Some(db) = dbs.get(db_path) {
            return Ok(Arc::clone(db));
        }
        let path_owned = db_path.to_path_buf();
        let db = tokio::task::spawn_blocking(move || Database::open(&path_owned))
            .await
            .context("Database task panicked")??;
        let db = Arc::new(Mutex::new(db));
        dbs.insert(db_path.to_path_buf(), Arc::clone(&db));
        Ok(db)
    }

    pub async fn register_session(&self, ctx: &ConnectionContext) {
        let mut sessions = self.active_sessions.lock().await;
        let key = session_key(ctx);
        let entry = sessions.entry(key).or_insert_with(|| ActiveSessionEntry {
            session: ActiveReviewSession {
                repo_root: ctx.repo_root.to_string_lossy().into_owned(),
                worktree: ctx.worktree.to_string_lossy().into_owned(),
                base_ref: ctx.base_ref.clone(),
                head_ref: ctx.head_scope_key(),
                merge_base: ctx.merge_base.to_string(),
                client_count: 0,
            },
            client_count: 0,
        });
        entry.client_count += 1;
        entry.session.client_count = entry.client_count;
    }

    pub async fn unregister_session(&self, ctx: &ConnectionContext) {
        let mut sessions = self.active_sessions.lock().await;
        let key = session_key(ctx);
        let Some(entry) = sessions.get_mut(&key) else {
            return;
        };
        entry.client_count = entry.client_count.saturating_sub(1);
        if entry.client_count == 0 {
            sessions.remove(&key);
        } else {
            entry.session.client_count = entry.client_count;
        }
    }

    pub async fn list_sessions(&self) -> Vec<ActiveReviewSession> {
        let sessions = self.active_sessions.lock().await;
        let mut out = sessions
            .values()
            .map(|entry| entry.session.clone())
            .collect::<Vec<_>>();
        out.sort_by(|a, b| {
            a.repo_root
                .cmp(&b.repo_root)
                .then_with(|| a.worktree.cmp(&b.worktree))
                .then_with(|| a.base_ref.cmp(&b.base_ref))
                .then_with(|| a.head_ref.cmp(&b.head_ref))
        });
        out
    }
}

fn session_key(ctx: &ConnectionContext) -> String {
    format!(
        "{}\0{}\0{}\0{}",
        ctx.repo_root.display(),
        ctx.worktree.display(),
        ctx.merge_base,
        ctx.head_scope_key()
    )
}

// ---------------------------------------------------------------------------
// Persistent server (foreground, logs to stderr, SIGINT to stop)
// ---------------------------------------------------------------------------

/// Start the persistent server, listening on the given socket path.
pub async fn run_persistent(socket_path: &Path) -> Result<()> {
    let listener = bind_socket(socket_path, true).await?;
    let http_listener = bind_http_listener(true).await?;
    eprintln!("crt server listening on {}", socket_path.display());

    let state = Arc::new(ServerState::new());
    let cancel = CancellationToken::new();

    // Shut down on SIGINT
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        eprintln!("\nShutting down...");
        cancel_clone.cancel();
    });

    accept_loop(listener, http_listener, state, cancel, true).await;

    let _ = std::fs::remove_file(socket_path);
    eprintln!("Server stopped.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Embedded server (background task, silent, socket-based)
// ---------------------------------------------------------------------------

/// Start an embedded server as a background task. Returns a cancellation
/// token that stops the server when dropped/cancelled.
///
/// The server binds to `socket_path` and accepts connections. It produces
/// no stderr output. Other clients can connect to the same socket.
pub async fn start_embedded(socket_path: &Path) -> Result<CancellationToken> {
    let listener = bind_socket(socket_path, false).await?;
    let http_listener = bind_http_listener(false).await?;
    let state = Arc::new(ServerState::new());
    let cancel = CancellationToken::new();
    let socket_path_owned = socket_path.to_path_buf();

    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        accept_loop(listener, http_listener, state, cancel_clone, false).await;
        let _ = std::fs::remove_file(&socket_path_owned);
    });

    Ok(cancel)
}

// ---------------------------------------------------------------------------
// Shared infrastructure
// ---------------------------------------------------------------------------

/// Bind to a Unix socket, handling stale sockets.
async fn bind_socket(socket_path: &Path, verbose: bool) -> Result<UnixListener> {
    if socket_path.exists() {
        // Try connecting to see if another server is running
        let is_alive = UnixStream::connect(socket_path).await.is_ok();

        if is_alive {
            bail!(
                "Another server is already running on {}",
                socket_path.display()
            );
        }

        // Stale socket — remove it
        std::fs::remove_file(socket_path).with_context(|| {
            format!("Failed to remove stale socket at {}", socket_path.display())
        })?;
        if verbose {
            eprintln!("Removed stale socket at {}", socket_path.display());
        }
    }

    // Ensure parent directory exists
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }

    UnixListener::bind(socket_path)
        .with_context(|| format!("Failed to bind to {}", socket_path.display()))
}

async fn bind_http_listener(verbose: bool) -> Result<Option<TcpListener>> {
    let host = std::env::var(ENV_HTTP_HOST).unwrap_or_else(|_| DEFAULT_HTTP_HOST.to_string());
    let port = std::env::var(ENV_HTTP_PORT)
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(DEFAULT_HTTP_PORT);
    let addr = format!("{host}:{port}");

    match TcpListener::bind(&addr).await {
        Ok(listener) => {
            if verbose {
                eprintln!("crt HTTP JSON-RPC listening on http://{addr}");
            }
            Ok(Some(listener))
        }
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
            if verbose {
                eprintln!("crt HTTP JSON-RPC disabled; {addr} is already in use");
            }
            Ok(None)
        }
        Err(e) => Err(e).with_context(|| format!("Failed to bind HTTP JSON-RPC on {addr}")),
    }
}

/// Core accept loop shared by persistent and embedded servers.
async fn accept_loop(
    listener: UnixListener,
    http_listener: Option<TcpListener>,
    state: Arc<ServerState>,
    cancel: CancellationToken,
    verbose: bool,
) {
    let mut http_listener = http_listener;
    loop {
        tokio::select! {
            accept = listener.accept() => {
                match accept {
                    Ok((stream, _addr)) => {
                        if verbose {
                            eprintln!("Client connected");
                        }
                        let state = Arc::clone(&state);
                        let v = verbose;
                        tokio::spawn(async move {
                            let result = handle_connection(stream, state).await;
                            if v {
                                if let Err(e) = result {
                                    eprintln!("Connection error: {e}");
                                }
                                eprintln!("Client disconnected");
                            }
                        });
                    }
                    Err(e) => {
                        if verbose {
                            eprintln!("Accept error: {e}");
                        }
                    }
                }
            }
            accept = async {
                match http_listener.as_ref() {
                    Some(listener) => Some(listener.accept().await),
                    None => None,
                }
            }, if http_listener.is_some() => {
                match accept {
                    Some(Ok((stream, _addr))) => {
                        if verbose {
                            eprintln!("HTTP client connected");
                        }
                        let state = Arc::clone(&state);
                        let v = verbose;
                        tokio::spawn(async move {
                            let result = handle_http_connection(stream, state).await;
                            if v {
                                if let Err(e) = result {
                                    eprintln!("HTTP connection error: {e}");
                                }
                                eprintln!("HTTP client disconnected");
                            }
                        });
                    }
                    Some(Err(e)) => {
                        if verbose {
                            eprintln!("HTTP accept error: {e}");
                        }
                        http_listener = None;
                    }
                    None => {}
                }
            }
            _ = cancel.cancelled() => {
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Connection handler
// ---------------------------------------------------------------------------

async fn handle_connection(stream: UnixStream, state: Arc<ServerState>) -> Result<()> {
    let (reader, writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let writer = Arc::new(Mutex::new(writer));
    let mut conn_ctx: Option<ConnectionContext> = None;
    let mut conn_db: Option<Arc<Mutex<Database>>> = None;

    // Subscribe to broadcast notifications. We subscribe early so we
    // don't miss notifications sent between init and the first read.
    let mut notify_rx = state.notify_tx.subscribe();
    let notify_writer = Arc::clone(&writer);

    // Token to cancel the notification forwarder when the connection ends.
    let notify_cancel = CancellationToken::new();
    let notify_cancel_clone = notify_cancel.clone();

    // Spawn a task that forwards broadcast notifications to this client.
    let notify_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                result = notify_rx.recv() => {
                    match result {
                        Ok(notification) => {
                            let msg = serde_json::json!({
                                "jsonrpc": "2.0",
                                "method": "notification",
                                "params": notification,
                            });
                            let mut json = match serde_json::to_string(&msg) {
                                Ok(j) => j,
                                Err(_) => continue,
                            };
                            json.push('\n');
                            let mut w = notify_writer.lock().await;
                            if w.write_all(json.as_bytes()).await.is_err() {
                                break;
                            }
                            let _ = w.flush().await;
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = notify_cancel_clone.cancelled() => break,
            }
        }
    });

    let mut line = String::new();
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .await
            .context("Failed to read from client")?;

        if n == 0 {
            break; // Client disconnected
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Parse JSON-RPC request
        let request: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(req) => req,
            Err(e) => {
                let resp = JsonRpcResponse::error(
                    serde_json::Value::Null,
                    ERR_PARSE,
                    format!("Parse error: {e}"),
                );
                send_response(&writer, &resp).await?;
                continue;
            }
        };

        // Notifications (no id) — we don't handle any client-to-server notifications yet
        let id = match &request.id {
            Some(id) => id.clone(),
            None => continue,
        };

        let response = dispatch(&request, &id, &state, &mut conn_ctx, &mut conn_db).await;
        send_response(&writer, &response).await?;
    }

    if let Some(ctx) = conn_ctx.as_ref() {
        state.unregister_session(ctx).await;
    }

    // Stop the notification forwarder.
    notify_cancel.cancel();
    let _ = notify_handle.await;

    Ok(())
}

async fn handle_http_connection(stream: TcpStream, state: Arc<ServerState>) -> Result<()> {
    let connection_state = Arc::new(Mutex::new(HttpConnectionState {
        conn_ctx: None,
        conn_db: None,
    }));
    let state_for_service = Arc::clone(&state);
    let connection_state_for_service = Arc::clone(&connection_state);

    hyper::server::conn::http1::Builder::new()
        .serve_connection(
            TokioIo::new(stream),
            service_fn(move |request| {
                handle_http_request(
                    request,
                    Arc::clone(&state_for_service),
                    Arc::clone(&connection_state_for_service),
                )
            }),
        )
        .await
        .context("HTTP connection failed")?;

    if let Some(ctx) = connection_state.lock().await.conn_ctx.as_ref() {
        state.unregister_session(ctx).await;
    }

    Ok(())
}

async fn handle_http_request(
    request: Request<Incoming>,
    state: Arc<ServerState>,
    connection_state: Arc<Mutex<HttpConnectionState>>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    if request.method() != hyper::Method::POST || request.uri().path() != "/rpc" {
        return Ok(empty_http_response(StatusCode::NOT_FOUND));
    }

    let body = match request.into_body().collect().await {
        Ok(body) => body.to_bytes(),
        Err(e) => {
            let resp = JsonRpcResponse::error(
                serde_json::Value::Null,
                ERR_PARSE,
                format!("Failed to read request body: {e}"),
            );
            return Ok(json_http_response(StatusCode::BAD_REQUEST, &resp));
        }
    };

    let request: JsonRpcRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => {
            let resp = JsonRpcResponse::error(
                serde_json::Value::Null,
                ERR_PARSE,
                format!("Parse error: {e}"),
            );
            return Ok(json_http_response(StatusCode::OK, &resp));
        }
    };

    let id = match &request.id {
        Some(id) => id.clone(),
        None => return Ok(empty_http_response(StatusCode::NO_CONTENT)),
    };

    let mut connection_state = connection_state.lock().await;
    let HttpConnectionState { conn_ctx, conn_db } = &mut *connection_state;
    let response = dispatch(&request, &id, &state, conn_ctx, conn_db).await;
    Ok(json_http_response(StatusCode::OK, &response))
}

fn json_http_response(status: StatusCode, response: &JsonRpcResponse) -> Response<Full<Bytes>> {
    let body = serde_json::to_vec(response).unwrap_or_else(|_| b"{}".to_vec());
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(body)))
        .unwrap_or_else(|_| empty_http_response(StatusCode::INTERNAL_SERVER_ERROR))
}

fn empty_http_response(status: StatusCode) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .body(Full::new(Bytes::new()))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::new())))
}

async fn send_response(
    writer: &Arc<Mutex<tokio::net::unix::OwnedWriteHalf>>,
    response: &JsonRpcResponse,
) -> Result<()> {
    let mut json = serde_json::to_string(response).context("Failed to serialize response")?;
    json.push('\n');
    let mut w = writer.lock().await;
    w.write_all(json.as_bytes())
        .await
        .context("Failed to write response")?;
    w.flush().await.context("Failed to flush response")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// All API methods the server supports. As methods are implemented,
/// their dispatch arms move from returning `ERR_NOT_IMPLEMENTED` to
/// calling real handlers. The compiler enforces exhaustive matching.
#[derive(Clone, Copy)]
enum Method {
    Init,
    ListChangedFiles,
    GetFileDiff,
    GetFileContent,
    MarkReviewed,
    UnmarkReviewed,
    ResetReviews,
    CreateComment,
    ListComments,
    GetComment,
    UpdateComment,
    ResolveComment,
    UnresolveComment,
    DeleteComment,
    ApplyComments,
    ClearComments,
    SearchCodebase,
    FindDefinition,
    TrackRepo,
    ListRepos,
}

impl Method {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "init" => Some(Self::Init),
            "list_changed_files" => Some(Self::ListChangedFiles),
            "get_file_diff" => Some(Self::GetFileDiff),
            "get_file_content" => Some(Self::GetFileContent),
            "mark_reviewed" => Some(Self::MarkReviewed),
            "unmark_reviewed" => Some(Self::UnmarkReviewed),
            "reset_reviews" => Some(Self::ResetReviews),
            "create_comment" => Some(Self::CreateComment),
            "list_comments" => Some(Self::ListComments),
            "get_comment" => Some(Self::GetComment),
            "update_comment" => Some(Self::UpdateComment),
            "resolve_comment" => Some(Self::ResolveComment),
            "unresolve_comment" => Some(Self::UnresolveComment),
            "delete_comment" => Some(Self::DeleteComment),
            "apply_comments" => Some(Self::ApplyComments),
            "clear_comments" => Some(Self::ClearComments),
            "search_codebase" => Some(Self::SearchCodebase),
            "find_definition" => Some(Self::FindDefinition),
            "track_repo" => Some(Self::TrackRepo),
            "list_repos" => Some(Self::ListRepos),
            _ => None,
        }
    }
}

async fn dispatch(
    request: &JsonRpcRequest,
    id: &serde_json::Value,
    state: &Arc<ServerState>,
    conn_ctx: &mut Option<ConnectionContext>,
    conn_db: &mut Option<Arc<Mutex<Database>>>,
) -> JsonRpcResponse {
    let method = match Method::from_str(&request.method) {
        Some(m) => m,
        None => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_METHOD_NOT_FOUND,
                format!("Method '{}' not found", request.method),
            );
        }
    };

    match method {
        Method::Init => api::handle_init(&request.params, id, state, conn_ctx, conn_db).await,
        Method::ListRepos => api::handle_list_repos(id, state).await,
        _ => dispatch_initialized(request, id, method, state, conn_ctx, conn_db).await,
    }
}

async fn dispatch_initialized(
    request: &JsonRpcRequest,
    id: &serde_json::Value,
    method: Method,
    state: &Arc<ServerState>,
    conn_ctx: &mut Option<ConnectionContext>,
    conn_db: &mut Option<Arc<Mutex<Database>>>,
) -> JsonRpcResponse {
    let (Some(ctx), Some(db)) = (conn_ctx.as_ref(), conn_db.as_ref()) else {
        return JsonRpcResponse::error(
            id.clone(),
            ERR_NOT_INITIALIZED,
            "Connection not initialized. Send 'init' first.".to_string(),
        );
    };

    // Dispatch — the compiler ensures every variant is handled.
    match method {
        Method::ListChangedFiles => {
            api::handle_list_changed_files(id, ctx, db, &state.notify_tx).await
        }
        Method::GetFileDiff => api::handle_get_file_diff(&request.params, id, ctx).await,
        Method::MarkReviewed => {
            api::handle_mark_reviewed(&request.params, id, ctx, db, &state.notify_tx).await
        }
        Method::UnmarkReviewed => {
            api::handle_unmark_reviewed(&request.params, id, ctx, db, &state.notify_tx).await
        }
        Method::ResetReviews => api::handle_reset_reviews(id, ctx, db, &state.notify_tx).await,
        Method::CreateComment => {
            api::handle_create_comment(&request.params, id, ctx, db, &state.notify_tx).await
        }
        Method::ListComments => api::handle_list_comments(&request.params, id, ctx, db).await,
        Method::GetComment => api::handle_get_comment(&request.params, id, ctx, db).await,
        Method::UpdateComment => {
            api::handle_update_comment(&request.params, id, ctx, db, &state.notify_tx).await
        }
        Method::ResolveComment => {
            api::handle_resolve_comment(&request.params, id, ctx, db, &state.notify_tx).await
        }
        Method::UnresolveComment => {
            api::handle_unresolve_comment(&request.params, id, ctx, db, &state.notify_tx).await
        }
        Method::DeleteComment => {
            api::handle_delete_comment(&request.params, id, ctx, db, &state.notify_tx).await
        }
        Method::GetFileContent
        | Method::ApplyComments
        | Method::ClearComments
        | Method::TrackRepo => JsonRpcResponse::error(
            id.clone(),
            ERR_NOT_IMPLEMENTED,
            format!("Method '{}' is not yet implemented", request.method),
        ),
        Method::SearchCodebase => api::handle_search_codebase(&request.params, id, ctx).await,
        Method::FindDefinition => api::handle_find_definition(&request.params, id, ctx).await,
        Method::Init | Method::ListRepos => JsonRpcResponse::error(
            id.clone(),
            ERR_INTERNAL,
            format!(
                "Method '{}' was dispatched through the wrong path",
                request.method
            ),
        ),
    }
}
