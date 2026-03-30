//! JSON-RPC API method implementations.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;

use super::{ConnectionContext, ERR_INTERNAL, ERR_INVALID_PARAMS, JsonRpcResponse, ServerState};
use crate::db::Database;
use crate::git;
use crate::model;

pub async fn handle_init(
    params: &serde_json::Value,
    id: &serde_json::Value,
    state: &Arc<ServerState>,
    conn_ctx: &mut Option<ConnectionContext>,
    conn_db: &mut Option<Arc<Mutex<Database>>>,
) -> JsonRpcResponse {
    let init_params: model::InitParams = match serde_json::from_value(params.clone()) {
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

    // Resolve repo context and merge-base via git module (blocking operation)
    let wt = worktree_path.clone();
    let br = base_ref.clone();
    let git_result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let repo = git::Repo::open(&wt)?;
        let ctx = repo.context()?;
        repo.resolve_commit(&br)?; // validate base ref
        let merge_base = repo.merge_base(&br, "HEAD")?;
        Ok((ctx, merge_base))
    })
    .await;

    let (git_ctx, merge_base) = match git_result {
        Ok(Ok(pair)) => pair,
        Ok(Err(e)) => {
            return JsonRpcResponse::error(id.clone(), ERR_INVALID_PARAMS, format!("{e:#}"));
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
    if let Err(e) = std::fs::create_dir_all(&crt_dir) {
        return JsonRpcResponse::error(
            id.clone(),
            ERR_INTERNAL,
            format!(
                "Failed to create .crt directory at {}: {e}",
                crt_dir.display()
            ),
        );
    }

    // Warn if .crt/ is not gitignored
    check_gitignore(&git_ctx.repo_root);

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
        merge_base: merge_base.clone(),
        head_ref: git_ctx.head_ref.clone(),
        db_path,
    };

    let result = model::ConnectionContext {
        repo_root: git_ctx.repo_root.to_string_lossy().into_owned(),
        worktree: worktree_path.to_string_lossy().into_owned(),
        head_ref: git_ctx.head_ref.clone(),
        merge_base: merge_base.clone(),
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

// ---------------------------------------------------------------------------
// list_changed_files
// ---------------------------------------------------------------------------

pub async fn handle_list_changed_files(
    id: &serde_json::Value,
    ctx: &ConnectionContext,
    db: &Arc<Mutex<Database>>,
) -> JsonRpcResponse {
    // Load stored reviews from DB (async lock, then sync DB call).
    let reviews = {
        let db_guard = db.lock().await;
        match db_guard.load_reviews(&ctx.merge_base, &ctx.head_ref) {
            Ok(r) => r,
            Err(e) => {
                return JsonRpcResponse::error(
                    id.clone(),
                    ERR_INTERNAL,
                    format!("Failed to load reviews: {e:#}"),
                );
            }
        }
    };

    let worktree = ctx.worktree.clone();
    let merge_base = ctx.merge_base.clone();

    // Git operations are blocking — run on the blocking thread pool.
    let git_result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let repo = git::Repo::open(&worktree)?;
        let changes = repo.list_changed_files(&merge_base, "HEAD")?;

        let mut files = Vec::new();
        for change in changes {
            let diff = repo.diff_file(&merge_base, "HEAD", &change.path)?;

            // Basic review status reconciliation via diff-hash comparison.
            let status = match reviews.get(&change.path) {
                None => model::ReviewStatus::Unreviewed,
                Some(review) if review.diff_hash == diff.diff_hash => {
                    model::ReviewStatus::Reviewed {
                        at: review.reviewed_at.clone(),
                    }
                }
                Some(review) => model::ReviewStatus::Changed {
                    at: review.reviewed_at.clone(),
                },
            };

            files.push(model::FileEntry {
                change,
                status,
                diff,
            });
        }

        Ok(model::ListChangedFilesResult { files })
    })
    .await;

    match git_result {
        Ok(Ok(list)) => match serde_json::to_value(list) {
            Ok(v) => JsonRpcResponse::success(id.clone(), v),
            Err(e) => JsonRpcResponse::error(
                id.clone(),
                ERR_INTERNAL,
                format!("Serialization error: {e}"),
            ),
        },
        Ok(Err(e)) => {
            JsonRpcResponse::error(id.clone(), ERR_INTERNAL, format!("{e:#}"))
        }
        Err(e) => JsonRpcResponse::error(
            id.clone(),
            ERR_INTERNAL,
            format!("Git task panicked: {e}"),
        ),
    }
}

// ---------------------------------------------------------------------------
// get_file_diff
// ---------------------------------------------------------------------------

pub async fn handle_get_file_diff(
    params: &serde_json::Value,
    id: &serde_json::Value,
    ctx: &ConnectionContext,
) -> JsonRpcResponse {
    let diff_params: model::GetFileDiffParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INVALID_PARAMS,
                format!("Invalid params: {e}"),
            );
        }
    };

    let worktree = ctx.worktree.clone();
    let merge_base = ctx.merge_base.clone();
    let file_path = diff_params.file_path;

    let git_result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let repo = git::Repo::open(&worktree)?;
        let diff = repo.diff_file(&merge_base, "HEAD", &file_path)?;
        Ok(model::GetFileDiffResult { diff })
    })
    .await;

    match git_result {
        Ok(Ok(r)) => match serde_json::to_value(r) {
            Ok(v) => JsonRpcResponse::success(id.clone(), v),
            Err(e) => JsonRpcResponse::error(
                id.clone(),
                ERR_INTERNAL,
                format!("Serialization error: {e}"),
            ),
        },
        Ok(Err(e)) => {
            JsonRpcResponse::error(id.clone(), ERR_INTERNAL, format!("{e:#}"))
        }
        Err(e) => JsonRpcResponse::error(
            id.clone(),
            ERR_INTERNAL,
            format!("Git task panicked: {e}"),
        ),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn check_gitignore(repo_root: &Path) {
    let gitignore_path = repo_root.join(".gitignore");
    if let Ok(contents) = std::fs::read_to_string(&gitignore_path) {
        let has_crt = contents.lines().any(|line| {
            let trimmed = line.trim();
            trimmed == ".crt" || trimmed == ".crt/" || trimmed == "/.crt" || trimmed == "/.crt/"
        });
        if !has_crt {
            eprintln!(
                "Warning: .crt/ is not in .gitignore. \
                 Consider adding it to avoid committing review state."
            );
        }
    }
}
