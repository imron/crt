//! Server core: Unix socket listener, connection handling, JSON-RPC dispatch.

pub mod api;
pub mod notify;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, broadcast};

use crate::db::Database;
use crate::git;

// ---------------------------------------------------------------------------
// JSON-RPC types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
    pub id: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    pub id: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JsonRpcResponse {
    pub fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: Some(result),
            error: None,
            id,
        }
    }

    pub fn error(id: serde_json::Value, code: i64, message: String) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data: None,
            }),
            id,
        }
    }
}

// JSON-RPC error codes
pub const ERR_PARSE: i64 = -32700;
pub const ERR_METHOD_NOT_FOUND: i64 = -32601;
pub const ERR_INVALID_PARAMS: i64 = -32602;
pub const ERR_INTERNAL: i64 = -32603;
pub const ERR_NOT_INITIALIZED: i64 = -32000;
pub const ERR_NOT_IMPLEMENTED: i64 = -32001;

// ---------------------------------------------------------------------------
// Connection state
// ---------------------------------------------------------------------------

/// Per-connection state, set during `init`.
#[derive(Debug, Clone)]
pub struct ConnectionContext {
    pub repo_root: PathBuf,
    pub worktree: PathBuf,
    pub base_ref: String,
    pub head_ref: String,
    pub db_path: PathBuf,
}

/// Shared state across all connections.
pub struct ServerState {
    /// Open databases keyed by repo root path.
    databases: Mutex<HashMap<PathBuf, Arc<Mutex<Database>>>>,
    /// Broadcast channel for notifications.
    _notify_tx: broadcast::Sender<notify::Notification>,
}

impl ServerState {
    pub fn new() -> Self {
        let (notify_tx, _) = broadcast::channel(64);
        Self {
            databases: Mutex::new(HashMap::new()),
            _notify_tx: notify_tx,
        }
    }

    /// Get or open the database for a repo.
    pub async fn get_db(&self, db_path: &Path) -> Result<Arc<Mutex<Database>>> {
        let mut dbs = self.databases.lock().await;
        if let Some(db) = dbs.get(db_path) {
            return Ok(Arc::clone(db));
        }
        let db = Database::open(db_path)
            .with_context(|| format!("Failed to open database at {}", db_path.display()))?;
        let db = Arc::new(Mutex::new(db));
        dbs.insert(db_path.to_path_buf(), Arc::clone(&db));
        Ok(db)
    }
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

/// Start the persistent server, listening on the given socket path.
pub async fn run_persistent(socket_path: &Path) -> Result<()> {
    // Handle stale socket
    if socket_path.exists() {
        match UnixStream::connect(socket_path).await {
            Ok(_) => {
                bail!(
                    "Another server is already running on {}",
                    socket_path.display()
                );
            }
            Err(_) => {
                // Stale socket — remove it
                std::fs::remove_file(socket_path).with_context(|| {
                    format!(
                        "Failed to remove stale socket at {}",
                        socket_path.display()
                    )
                })?;
                eprintln!(
                    "Removed stale socket at {}",
                    socket_path.display()
                );
            }
        }
    }

    // Ensure parent directory exists
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("Failed to create directory {}", parent.display())
        })?;
    }

    let listener = UnixListener::bind(socket_path).with_context(|| {
        format!("Failed to bind to {}", socket_path.display())
    })?;

    eprintln!("crt server listening on {}", socket_path.display());

    let state = Arc::new(ServerState::new());

    // Install shutdown handler
    let socket_path_owned = socket_path.to_path_buf();
    let shutdown = tokio::signal::ctrl_c();

    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            accept = listener.accept() => {
                match accept {
                    Ok((stream, _addr)) => {
                        let state = Arc::clone(&state);
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, state).await {
                                eprintln!("Connection error: {e}");
                            }
                        });
                    }
                    Err(e) => {
                        eprintln!("Accept error: {e}");
                    }
                }
            }
            _ = &mut shutdown => {
                eprintln!("\nShutting down...");
                break;
            }
        }
    }

    // Cleanup socket file
    let _ = std::fs::remove_file(&socket_path_owned);
    eprintln!("Server stopped.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Connection handler
// ---------------------------------------------------------------------------

async fn handle_connection(stream: UnixStream, state: Arc<ServerState>) -> Result<()> {
    let (reader, writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let writer = Arc::new(Mutex::new(writer));
    let mut conn_ctx: Option<ConnectionContext> = None;
    let mut db: Option<Arc<Mutex<Database>>> = None;

    eprintln!("Client connected");

    let mut line = String::new();
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .await
            .context("Failed to read from client")?;

        if n == 0 {
            // Client disconnected
            break;
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

        // Notifications (no id) — we don't handle any yet
        let id = match &request.id {
            Some(id) => id.clone(),
            None => continue,
        };

        // Dispatch
        let response = dispatch(&request, &id, &state, &mut conn_ctx, &mut db).await;
        send_response(&writer, &response).await?;
    }

    eprintln!("Client disconnected");
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

async fn dispatch(
    request: &JsonRpcRequest,
    id: &serde_json::Value,
    state: &Arc<ServerState>,
    conn_ctx: &mut Option<ConnectionContext>,
    db: &mut Option<Arc<Mutex<Database>>>,
) -> JsonRpcResponse {
    // `init` is special — it doesn't require prior initialization
    if request.method == "init" {
        return handle_init(&request.params, id, state, conn_ctx, db).await;
    }

    // All other methods require initialization
    let (_ctx, _db) = match (conn_ctx.as_ref(), db.as_ref()) {
        (Some(c), Some(d)) => (c, d),
        _ => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_NOT_INITIALIZED,
                "Connection not initialized. Send 'init' first.".to_string(),
            );
        }
    };

    // Dispatch to API stubs
    match request.method.as_str() {
        "list_changed_files" | "get_file_diff" | "get_file_content" | "mark_reviewed"
        | "unmark_reviewed" | "reset_reviews" | "create_comment" | "list_comments"
        | "get_comment" | "update_comment" | "resolve_comment" | "unresolve_comment"
        | "delete_comment" | "apply_comments" | "clear_comments" | "search_codebase"
        | "find_definition" | "track_repo" | "list_repos" => JsonRpcResponse::error(
            id.clone(),
            ERR_NOT_IMPLEMENTED,
            format!("Method '{}' is not yet implemented", request.method),
        ),
        _ => JsonRpcResponse::error(
            id.clone(),
            ERR_METHOD_NOT_FOUND,
            format!("Method '{}' not found", request.method),
        ),
    }
}

// ---------------------------------------------------------------------------
// init handler
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct InitParams {
    worktree: String,
    base_ref: String,
}

#[derive(Debug, Serialize)]
struct InitResult {
    repo_root: String,
    worktree: String,
    head_ref: String,
    base_ref: String,
}

async fn handle_init(
    params: &serde_json::Value,
    id: &serde_json::Value,
    state: &Arc<ServerState>,
    conn_ctx: &mut Option<ConnectionContext>,
    db: &mut Option<Arc<Mutex<Database>>>,
) -> JsonRpcResponse {
    let init_params: InitParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INVALID_PARAMS,
                format!("Invalid init params: {e}"),
            );
        }
    };

    let worktree_path = PathBuf::from(&init_params.worktree);

    // Resolve repo context via git module
    let repo = match git::Repo::open(&worktree_path) {
        Ok(r) => r,
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INTERNAL,
                format!("Failed to open repository: {e}"),
            );
        }
    };

    let git_ctx = match repo.context() {
        Ok(c) => c,
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INTERNAL,
                format!("Failed to resolve repository context: {e}"),
            );
        }
    };

    // Validate the base ref
    if let Err(e) = repo.resolve_commit(&init_params.base_ref) {
        return JsonRpcResponse::error(
            id.clone(),
            ERR_INVALID_PARAMS,
            format!("Failed to resolve base ref '{}': {e}", init_params.base_ref),
        );
    }

    // Ensure .crt directory exists
    let crt_dir = git_ctx.repo_root.join(".crt");
    if !crt_dir.exists() {
        if let Err(e) = std::fs::create_dir_all(&crt_dir) {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INTERNAL,
                format!("Failed to create .crt directory: {e}"),
            );
        }
    }

    let db_path = crt_dir.join("reviews.db");

    // Open database
    let db_handle = match state.get_db(&db_path).await {
        Ok(d) => d,
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INTERNAL,
                format!("Failed to open database: {e}"),
            );
        }
    };

    let ctx = ConnectionContext {
        repo_root: git_ctx.repo_root.clone(),
        worktree: worktree_path.clone(),
        base_ref: init_params.base_ref.clone(),
        head_ref: git_ctx.head_ref.clone(),
        db_path,
    };

    let result = InitResult {
        repo_root: git_ctx.repo_root.to_string_lossy().into_owned(),
        worktree: worktree_path.to_string_lossy().into_owned(),
        head_ref: git_ctx.head_ref,
        base_ref: init_params.base_ref,
    };

    *conn_ctx = Some(ctx);
    *db = Some(db_handle);

    JsonRpcResponse::success(
        id.clone(),
        serde_json::to_value(result).unwrap_or(serde_json::Value::Null),
    )
}
