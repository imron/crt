//! JSON-RPC API method implementations.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;

use tokio::sync::broadcast;

use super::{
    notify, ConnectionContext, ERR_INTERNAL, ERR_INVALID_PARAMS, JsonRpcResponse, ServerState,
};
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
// mark_reviewed
// ---------------------------------------------------------------------------

pub async fn handle_mark_reviewed(
    params: &serde_json::Value,
    id: &serde_json::Value,
    ctx: &ConnectionContext,
    db: &Arc<Mutex<Database>>,
    notify_tx: &broadcast::Sender<notify::Notification>,
) -> JsonRpcResponse {
    let p: model::MarkReviewedParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INVALID_PARAMS,
                format!("Invalid params: {e}"),
            );
        }
    };

    // Compute the current diff hash.
    let worktree = ctx.worktree.clone();
    let merge_base = ctx.merge_base.clone();
    let file_path = p.file_path.clone();

    let diff_hash = {
        let wt = worktree.clone();
        let mb = merge_base.clone();
        let fp = file_path.clone();
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
            let repo = git::Repo::open(&wt)?;
            let diff = repo.diff_file(&mb, "HEAD", &fp)?;
            Ok(diff.diff_hash)
        })
        .await;

        match result {
            Ok(Ok(hash)) => hash,
            Ok(Err(e)) => {
                return JsonRpcResponse::error(id.clone(), ERR_INTERNAL, format!("{e:#}"));
            }
            Err(e) => {
                return JsonRpcResponse::error(
                    id.clone(),
                    ERR_INTERNAL,
                    format!("Git task panicked: {e}"),
                );
            }
        }
    };

    // Store the review in the database.
    let reviewed_at = {
        let db_guard = db.lock().await;
        match db_guard.store_review(&merge_base, &ctx.head_ref, &file_path, &diff_hash) {
            Ok(review) => review.reviewed_at,
            Err(e) => {
                return JsonRpcResponse::error(
                    id.clone(),
                    ERR_INTERNAL,
                    format!("Failed to store review: {e:#}"),
                );
            }
        }
    };

    // Broadcast notification.
    let _ = notify_tx.send(notify::Notification {
        base_ref: merge_base,
        head_ref: ctx.head_ref.clone(),
        kind: notify::NotificationKind::ReviewChanged {
            file_path: file_path.clone(),
        },
    });

    let result = model::ReviewActionResult {
        file_path,
        status: model::ReviewStatus::Reviewed { at: reviewed_at },
    };

    match serde_json::to_value(result) {
        Ok(v) => JsonRpcResponse::success(id.clone(), v),
        Err(e) => JsonRpcResponse::error(
            id.clone(),
            ERR_INTERNAL,
            format!("Serialization error: {e}"),
        ),
    }
}

// ---------------------------------------------------------------------------
// unmark_reviewed
// ---------------------------------------------------------------------------

pub async fn handle_unmark_reviewed(
    params: &serde_json::Value,
    id: &serde_json::Value,
    ctx: &ConnectionContext,
    db: &Arc<Mutex<Database>>,
    notify_tx: &broadcast::Sender<notify::Notification>,
) -> JsonRpcResponse {
    let p: model::UnmarkReviewedParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INVALID_PARAMS,
                format!("Invalid params: {e}"),
            );
        }
    };

    // Remove the review from the database.
    {
        let db_guard = db.lock().await;
        if let Err(e) = db_guard.remove_review(&ctx.merge_base, &ctx.head_ref, &p.file_path) {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INTERNAL,
                format!("Failed to remove review: {e:#}"),
            );
        }
    }

    // Broadcast notification.
    let _ = notify_tx.send(notify::Notification {
        base_ref: ctx.merge_base.clone(),
        head_ref: ctx.head_ref.clone(),
        kind: notify::NotificationKind::ReviewChanged {
            file_path: p.file_path.clone(),
        },
    });

    let result = model::ReviewActionResult {
        file_path: p.file_path,
        status: model::ReviewStatus::Unreviewed,
    };

    match serde_json::to_value(result) {
        Ok(v) => JsonRpcResponse::success(id.clone(), v),
        Err(e) => JsonRpcResponse::error(
            id.clone(),
            ERR_INTERNAL,
            format!("Serialization error: {e}"),
        ),
    }
}

// ---------------------------------------------------------------------------
// reset_reviews
// ---------------------------------------------------------------------------

pub async fn handle_reset_reviews(
    id: &serde_json::Value,
    ctx: &ConnectionContext,
    db: &Arc<Mutex<Database>>,
    notify_tx: &broadcast::Sender<notify::Notification>,
) -> JsonRpcResponse {
    let cleared = {
        let db_guard = db.lock().await;
        match db_guard.clear_reviews(&ctx.merge_base, &ctx.head_ref) {
            Ok(n) => n,
            Err(e) => {
                return JsonRpcResponse::error(
                    id.clone(),
                    ERR_INTERNAL,
                    format!("Failed to clear reviews: {e:#}"),
                );
            }
        }
    };

    // Broadcast notification.
    let _ = notify_tx.send(notify::Notification {
        base_ref: ctx.merge_base.clone(),
        head_ref: ctx.head_ref.clone(),
        kind: notify::NotificationKind::ReviewsCleared,
    });

    let result = model::ResetReviewsResult { cleared };

    match serde_json::to_value(result) {
        Ok(v) => JsonRpcResponse::success(id.clone(), v),
        Err(e) => JsonRpcResponse::error(
            id.clone(),
            ERR_INTERNAL,
            format!("Serialization error: {e}"),
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

// ---------------------------------------------------------------------------
// search_codebase
// ---------------------------------------------------------------------------

pub async fn handle_search_codebase(
    params: &serde_json::Value,
    id: &serde_json::Value,
    ctx: &ConnectionContext,
) -> JsonRpcResponse {
    let search_params: model::SearchCodebaseParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INVALID_PARAMS,
                format!("Invalid search params: {e}"),
            );
        }
    };

    let repo_root = ctx.worktree.clone();
    let pattern = search_params.pattern.clone();
    let scope = search_params.scope.clone();

    // Get diff files if scope is "diff".
    let diff_files = if scope.as_deref() == Some("diff") {
        let repo = match git::Repo::open(&repo_root) {
            Ok(r) => r,
            Err(e) => {
                return JsonRpcResponse::error(
                    id.clone(),
                    ERR_INTERNAL,
                    format!("Failed to open repo: {e:#}"),
                );
            }
        };
        match repo.list_changed_files(&ctx.merge_base, "HEAD") {
            Ok(files) => Some(files.iter().map(|f| f.path.clone()).collect::<Vec<_>>()),
            Err(e) => {
                return JsonRpcResponse::error(
                    id.clone(),
                    ERR_INTERNAL,
                    format!("Failed to list changed files: {e:#}"),
                );
            }
        }
    } else {
        None
    };

    // Run the search on a blocking thread to avoid stalling the runtime.
    let id_owned = id.clone();
    let result = tokio::task::spawn_blocking(move || {
        crate::search::search_codebase(
            &repo_root,
            &pattern,
            diff_files.as_deref(),
        )
    })
    .await;

    match result {
        Ok(Ok(search_result)) => match serde_json::to_value(search_result) {
            Ok(v) => JsonRpcResponse::success(id_owned, v),
            Err(e) => JsonRpcResponse::error(
                id_owned,
                ERR_INTERNAL,
                format!("Serialization error: {e}"),
            ),
        },
        Ok(Err(e)) => JsonRpcResponse::error(
            id_owned,
            ERR_INTERNAL,
            format!("Search failed: {e:#}"),
        ),
        Err(e) => JsonRpcResponse::error(
            id_owned,
            ERR_INTERNAL,
            format!("Search task panicked: {e}"),
        ),
    }
}

// ---------------------------------------------------------------------------
// find_definition
// ---------------------------------------------------------------------------

pub async fn handle_find_definition(
    params: &serde_json::Value,
    id: &serde_json::Value,
    ctx: &ConnectionContext,
) -> JsonRpcResponse {
    let def_params: model::FindDefinitionParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INVALID_PARAMS,
                format!("Invalid find_definition params: {e}"),
            );
        }
    };

    let repo_root = ctx.worktree.clone();
    let symbol = def_params.symbol.clone();
    let context_file = def_params.context_file.clone();
    let id_owned = id.clone();

    let result = tokio::task::spawn_blocking(move || {
        use crate::search::{DefinitionFinder, RegexDefinitionFinder};
        let finder = RegexDefinitionFinder;
        finder.find_definition(&symbol, context_file.as_deref(), &repo_root)
    })
    .await;

    match result {
        Ok(Ok(def_result)) => match serde_json::to_value(def_result) {
            Ok(v) => JsonRpcResponse::success(id_owned, v),
            Err(e) => JsonRpcResponse::error(
                id_owned,
                ERR_INTERNAL,
                format!("Serialization error: {e}"),
            ),
        },
        Ok(Err(e)) => JsonRpcResponse::error(
            id_owned,
            ERR_INTERNAL,
            format!("Definition search failed: {e:#}"),
        ),
        Err(e) => JsonRpcResponse::error(
            id_owned,
            ERR_INTERNAL,
            format!("Definition task panicked: {e}"),
        ),
    }
}
