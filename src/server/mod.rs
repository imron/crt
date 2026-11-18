//! Server core: Unix socket listener, connection handling, JSON-RPC dispatch.
//!
//! The server owns connection lifecycle and method dispatch. Shared wire
//! models live in `crate::protocol` so clients do not depend on server
//! internals.

pub mod api;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, broadcast};
use tokio_util::sync::CancellationToken;

use crate::db::Database;
use crate::protocol::{
    ERR_METHOD_NOT_FOUND, ERR_NOT_IMPLEMENTED, ERR_NOT_INITIALIZED, ERR_PARSE, JsonRpcRequest,
    JsonRpcResponse, Notification,
};

// ---------------------------------------------------------------------------
// Connection state
// ---------------------------------------------------------------------------

/// Per-connection state, set during `init`.
#[derive(Debug, Clone)]
pub struct ConnectionContext {
    pub repo_root: PathBuf,
    pub worktree: PathBuf,
    /// The merge-base commit hash (stable scope key).
    pub merge_base: String,
    pub head_ref: String,
    pub db_path: PathBuf,
}

/// Shared state across all connections.
pub struct ServerState {
    /// Open databases keyed by repo root path.
    databases: Mutex<HashMap<PathBuf, Arc<Mutex<Database>>>>,
    /// Broadcast channel for notifications.
    pub notify_tx: broadcast::Sender<Notification>,
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
}

// ---------------------------------------------------------------------------
// Persistent server (foreground, logs to stderr, SIGINT to stop)
// ---------------------------------------------------------------------------

/// Start the persistent server, listening on the given socket path.
pub async fn run_persistent(socket_path: &Path) -> Result<()> {
    let listener = bind_socket(socket_path, true).await?;
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

    accept_loop(listener, state, cancel, true).await;

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
    let state = Arc::new(ServerState::new());
    let cancel = CancellationToken::new();
    let socket_path_owned = socket_path.to_path_buf();

    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        accept_loop(listener, state, cancel_clone, false).await;
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

/// Core accept loop shared by persistent and embedded servers.
async fn accept_loop(
    listener: UnixListener,
    state: Arc<ServerState>,
    cancel: CancellationToken,
    verbose: bool,
) {
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

    // Stop the notification forwarder.
    notify_cancel.cancel();
    let _ = notify_handle.await;

    Ok(())
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

    // Dispatch — the compiler ensures every variant is handled.
    // `init` is the only method that doesn't require prior initialization.
    match method {
        Method::Init => api::handle_init(&request.params, id, state, conn_ctx, conn_db).await,
        _ if conn_ctx.is_none() || conn_db.is_none() => JsonRpcResponse::error(
            id.clone(),
            ERR_NOT_INITIALIZED,
            "Connection not initialized. Send 'init' first.".to_string(),
        ),
        Method::ListChangedFiles => {
            let ctx = conn_ctx.as_ref().unwrap();
            let db = conn_db.as_ref().unwrap();
            api::handle_list_changed_files(id, ctx, db, &state.notify_tx).await
        }
        Method::GetFileDiff => {
            let ctx = conn_ctx.as_ref().unwrap();
            api::handle_get_file_diff(&request.params, id, ctx).await
        }
        Method::MarkReviewed => {
            let ctx = conn_ctx.as_ref().unwrap();
            let db = conn_db.as_ref().unwrap();
            api::handle_mark_reviewed(&request.params, id, ctx, db, &state.notify_tx).await
        }
        Method::UnmarkReviewed => {
            let ctx = conn_ctx.as_ref().unwrap();
            let db = conn_db.as_ref().unwrap();
            api::handle_unmark_reviewed(&request.params, id, ctx, db, &state.notify_tx).await
        }
        Method::ResetReviews => {
            let ctx = conn_ctx.as_ref().unwrap();
            let db = conn_db.as_ref().unwrap();
            api::handle_reset_reviews(id, ctx, db, &state.notify_tx).await
        }
        Method::GetFileContent
        | Method::CreateComment
        | Method::ListComments
        | Method::GetComment
        | Method::UpdateComment
        | Method::ResolveComment
        | Method::UnresolveComment
        | Method::DeleteComment
        | Method::ApplyComments
        | Method::ClearComments
        | Method::TrackRepo
        | Method::ListRepos => JsonRpcResponse::error(
            id.clone(),
            ERR_NOT_IMPLEMENTED,
            format!("Method '{}' is not yet implemented", request.method),
        ),
        Method::SearchCodebase => {
            let ctx = conn_ctx.as_ref().unwrap();
            api::handle_search_codebase(&request.params, id, ctx).await
        }
        Method::FindDefinition => {
            let ctx = conn_ctx.as_ref().unwrap();
            api::handle_find_definition(&request.params, id, ctx).await
        }
    }
}
