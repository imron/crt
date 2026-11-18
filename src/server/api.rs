//! JSON-RPC API method implementations.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::{ConnectionContext, ERR_INTERNAL, ERR_INVALID_PARAMS, JsonRpcResponse, ServerState};
use crate::db::Database;
use crate::git;

// ---------------------------------------------------------------------------
// init
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

pub async fn handle_init(
    params: &serde_json::Value,
    id: &serde_json::Value,
    state: &Arc<ServerState>,
    conn_ctx: &mut Option<ConnectionContext>,
    conn_db: &mut Option<Arc<Mutex<Database>>>,
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
    let base_ref = init_params.base_ref.clone();

    // Resolve repo context via git module (blocking operation)
    let wt = worktree_path.clone();
    let br = base_ref.clone();
    let git_result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let repo = git::Repo::open(&wt)?;
        let ctx = repo.context()?;
        repo.resolve_commit(&br)?; // validate base ref
        Ok(ctx)
    })
    .await;

    let git_ctx = match git_result {
        Ok(Ok(ctx)) => ctx,
        Ok(Err(e)) => {
            return JsonRpcResponse::error(id.clone(), ERR_INVALID_PARAMS, format!("{e}"));
        }
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INTERNAL,
                format!("Git task panicked: {e}"),
            );
        }
    };

    // Ensure .crt directory exists
    let crt_dir = git_ctx.repo_root.join(".crt");
    if !crt_dir.exists() && std::fs::create_dir_all(&crt_dir).is_err() {
        return JsonRpcResponse::error(
            id.clone(),
            ERR_INTERNAL,
            format!("Failed to create .crt directory at {}", crt_dir.display()),
        );
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
        base_ref: base_ref.clone(),
        head_ref: git_ctx.head_ref.clone(),
        db_path,
    };

    let result = InitResult {
        repo_root: git_ctx.repo_root.to_string_lossy().into_owned(),
        worktree: worktree_path.to_string_lossy().into_owned(),
        head_ref: git_ctx.head_ref,
        base_ref,
    };

    *conn_ctx = Some(ctx);
    *conn_db = Some(db_handle);

    match serde_json::to_value(result) {
        Ok(v) => JsonRpcResponse::success(id.clone(), v),
        Err(e) => JsonRpcResponse::error(
            id.clone(),
            ERR_INTERNAL,
            format!("Failed to serialize init result: {e}"),
        ),
    }
}
