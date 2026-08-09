//! JSON-RPC API method implementations.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use tokio::sync::broadcast;

use super::{ConnectionContext, ServerState};
use crate::db::{
    Database, NewAnchorVersion, NewComment, NewCommentAnchor, NewCommentAnchorSegment,
    NewCommentResolutionEvent, StoredComment,
};
use crate::git;
use crate::protocol::{
    ERR_INTERNAL, ERR_INVALID_PARAMS, JsonRpcResponse, Notification, NotificationKind,
};
use crate::review_types;

pub async fn handle_init(
    params: &serde_json::Value,
    id: &serde_json::Value,
    state: &Arc<ServerState>,
    conn_ctx: &mut Option<ConnectionContext>,
    conn_db: &mut Option<Arc<Mutex<Database>>>,
) -> JsonRpcResponse {
    let init_params: review_types::InitParams = match serde_json::from_value(params.clone()) {
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
    if init_params.root && !base_ref.is_empty() {
        return JsonRpcResponse::error(
            id.clone(),
            ERR_INVALID_PARAMS,
            "--root cannot be combined with a base ref".to_string(),
        );
    }

    // Resolve repo context and merge-base via git module (blocking operation)
    let wt = worktree_path.clone();
    let br = base_ref.clone();
    let root = init_params.root;
    let git_result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let repo = git::Repo::open(&wt)?;
        let ctx = repo.context()?;
        let (review_base, merge_base) = if root {
            let review_base = repo.resolve_root_review_base()?;
            let merge_base = review_base.resolved_commit().clone();
            (review_base, merge_base)
        } else {
            let review_base = repo.resolve_review_base(&br)?;
            let merge_base = git::CommitId::new(repo.merge_base(&br, "HEAD")?);
            (review_base, merge_base)
        };
        Ok((ctx, merge_base, review_base))
    })
    .await;

    let (git_ctx, merge_base, review_base) = match git_result {
        Ok(Ok(result)) => result,
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

    let diff_algorithm = init_params
        .diff_algorithm
        .as_deref()
        .and_then(crate::config::DiffAlgorithm::parse)
        .unwrap_or(crate::config::DiffAlgorithm::Patience);

    let ctx = ConnectionContext {
        repo_root: git_ctx.repo_root.clone(),
        worktree: worktree_path.clone(),
        base_ref: if init_params.root {
            review_types::ROOT_REVIEW_BASE_REF.to_string()
        } else {
            base_ref.clone()
        },
        review_base,
        merge_base: merge_base.clone(),
        head: git_ctx.head.clone(),
        db_path,
        diff_algorithm,
    };

    let result = protocol_context(&ctx);

    if let Some(old_ctx) = conn_ctx.as_ref() {
        state.unregister_session(old_ctx).await;
    }
    state.register_session(&ctx).await;
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

fn protocol_context(ctx: &ConnectionContext) -> review_types::ConnectionContext {
    review_types::ConnectionContext {
        repo_root: ctx.repo_root.to_string_lossy().into_owned(),
        worktree: ctx.worktree.to_string_lossy().into_owned(),
        base_ref: ctx.base_ref.clone(),
        head_ref: ctx.head.display_name(),
        merge_base: ctx.merge_base.to_string(),
    }
}

// ---------------------------------------------------------------------------
// list_repos
// ---------------------------------------------------------------------------

pub async fn handle_list_repos(
    id: &serde_json::Value,
    state: &Arc<ServerState>,
) -> JsonRpcResponse {
    let sessions = state.list_sessions().await;
    let repos = sessions
        .iter()
        .map(|session| session.repo_root.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let result = review_types::ListReposResult { repos, sessions };

    match serde_json::to_value(result) {
        Ok(v) => JsonRpcResponse::success(id.clone(), v),
        Err(e) => JsonRpcResponse::error(
            id.clone(),
            ERR_INTERNAL,
            format!("Failed to serialize repo list: {e}"),
        ),
    }
}

// ---------------------------------------------------------------------------
// list_changed_files
// ---------------------------------------------------------------------------

async fn load_reviews_with_migration(
    ctx: &ConnectionContext,
    db: &Arc<Mutex<Database>>,
    notify_tx: &broadcast::Sender<Notification>,
) -> Result<std::collections::HashMap<String, crate::db::StoredReview>, String> {
    let head_ref = ctx.head_scope_key();

    let reviews = {
        let db_guard = db.lock().await;
        db_guard
            .load_reviews(ctx.merge_base_key(), &head_ref)
            .map_err(|e| format!("Failed to load reviews: {e:#}"))?
    };

    if !reviews.is_empty() {
        return Ok(reviews);
    }

    match try_migrate_reviews(ctx, db, notify_tx).await {
        Ok(Some(migrated)) => Ok(migrated),
        Ok(None) => Ok(reviews),
        Err(e) => {
            eprintln!("Warning: rebase migration failed: {e:#}");
            Ok(reviews)
        }
    }
}

fn review_status_for_diff(
    reviews: &std::collections::HashMap<String, crate::db::StoredReview>,
    path: &str,
    diff_hash: &str,
) -> review_types::ReviewStatus {
    match reviews.get(path) {
        None => review_types::ReviewStatus::Unreviewed,
        Some(review) if review.diff_hash == diff_hash => review_types::ReviewStatus::Reviewed {
            at: review.reviewed_at.clone(),
            reviewed_commit: if review.reviewed_commit.is_empty() {
                None
            } else {
                Some(review.reviewed_commit.clone())
            },
        },
        Some(review) => review_types::ReviewStatus::Changed {
            at: review.reviewed_at.clone(),
            reviewed_commit: if review.reviewed_commit.is_empty() {
                None
            } else {
                Some(review.reviewed_commit.clone())
            },
        },
    }
}

pub async fn handle_list_changed_files(
    id: &serde_json::Value,
    ctx: &ConnectionContext,
    db: &Arc<Mutex<Database>>,
    notify_tx: &broadcast::Sender<Notification>,
) -> JsonRpcResponse {
    let reviews = match load_reviews_with_migration(ctx, db, notify_tx).await {
        Ok(reviews) => reviews,
        Err(e) => return JsonRpcResponse::error(id.clone(), ERR_INTERNAL, e),
    };

    let worktree = ctx.worktree.clone();
    let merge_base = ctx.merge_base.to_string();
    let root = matches!(ctx.review_base, git::ReviewBase::Root { .. });
    // Review hashing always uses patience so list endpoints stay on the fast
    // bulk git2 path. Display algorithm is applied client-side when loading a
    // selected file.
    let algorithm = crate::config::DiffAlgorithm::Patience;

    // Git operations are blocking — run on the blocking thread pool.
    let git_result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let repo = git::Repo::open(&worktree)?;
        let base = if root {
            git::DiffBase::EmptyTree
        } else {
            git::DiffBase::Commit(&merge_base)
        };
        let file_diffs = repo.diff_all_files_workdir_for_base(base, algorithm, false, true)?;

        let mut files = Vec::with_capacity(file_diffs.len());
        for (change, diff) in file_diffs {
            let status = review_status_for_diff(&reviews, &change.path, &diff.diff_hash);
            files.push(review_types::FileEntry {
                change,
                status,
                diff,
            });
        }

        Ok(review_types::ListChangedFilesResult { files })
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
        Ok(Err(e)) => JsonRpcResponse::error(id.clone(), ERR_INTERNAL, format!("{e:#}")),
        Err(e) => {
            JsonRpcResponse::error(id.clone(), ERR_INTERNAL, format!("Git task panicked: {e}"))
        }
    }
}

// ---------------------------------------------------------------------------
// list_file_statuses
// ---------------------------------------------------------------------------

pub async fn handle_list_file_statuses(
    id: &serde_json::Value,
    ctx: &ConnectionContext,
    db: &Arc<Mutex<Database>>,
    notify_tx: &broadcast::Sender<Notification>,
) -> JsonRpcResponse {
    let reviews = match load_reviews_with_migration(ctx, db, notify_tx).await {
        Ok(reviews) => reviews,
        Err(e) => return JsonRpcResponse::error(id.clone(), ERR_INTERNAL, e),
    };

    let worktree = ctx.worktree.clone();
    let merge_base = ctx.merge_base.to_string();
    let root = matches!(ctx.review_base, git::ReviewBase::Root { .. });
    // Keep review-status hashing on patience for bulk performance.
    let algorithm = crate::config::DiffAlgorithm::Patience;

    let git_result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let repo = git::Repo::open(&worktree)?;
        let base = if root {
            git::DiffBase::EmptyTree
        } else {
            git::DiffBase::Commit(&merge_base)
        };
        // Status list only needs hashes/stats — skip storing hunk line bodies.
        let file_diffs = repo.summarize_all_files_workdir_for_base(base, algorithm, false)?;

        let mut files = Vec::with_capacity(file_diffs.len());
        for (change, diff) in file_diffs {
            let status = review_status_for_diff(&reviews, &change.path, &diff.diff_hash);
            files.push(review_types::FileStatusEntry {
                change,
                status,
                diff,
            });
        }

        let reviewed_files = files
            .iter()
            .filter(|entry| matches!(entry.status, review_types::ReviewStatus::Reviewed { .. }))
            .count();
        let unreviewed_files = files
            .iter()
            .filter(|entry| matches!(entry.status, review_types::ReviewStatus::Unreviewed))
            .count();
        let changed_files = files
            .iter()
            .filter(|entry| matches!(entry.status, review_types::ReviewStatus::Changed { .. }))
            .count();

        Ok(review_types::ListFileStatusesResult {
            total_files: files.len(),
            reviewed_files,
            unreviewed_files,
            changed_files,
            files,
        })
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
        Ok(Err(e)) => JsonRpcResponse::error(id.clone(), ERR_INTERNAL, format!("{e:#}")),
        Err(e) => {
            JsonRpcResponse::error(id.clone(), ERR_INTERNAL, format!("Git task panicked: {e}"))
        }
    }
}

/// Attempt to migrate reviews from an old scope (different merge_base, same
/// head_ref) to the current scope. This handles the common case where a
/// branch has been rebased, causing the merge_base to change.
///
/// Returns `Ok(Some(reviews))` if migration occurred, `Ok(None)` if no
/// old-scope reviews were found, or `Err` on failure.
async fn try_migrate_reviews(
    ctx: &ConnectionContext,
    db: &Arc<Mutex<Database>>,
    notify_tx: &broadcast::Sender<Notification>,
) -> anyhow::Result<Option<std::collections::HashMap<String, crate::db::StoredReview>>> {
    use std::collections::HashMap;

    if !ctx.review_base.is_migration_eligible() {
        return Ok(None);
    }

    let head_ref = ctx.head_scope_key();
    let current_merge_base = ctx.merge_base.to_string();

    // 1. Find old-scope reviews for this head_ref.
    let old_scopes = {
        let db_guard = db.lock().await;
        db_guard.load_reviews_by_head_ref(&head_ref)?
    };

    // Filter out the current merge_base (shouldn't have any, but be safe).
    let old_scopes: Vec<_> = old_scopes
        .into_iter()
        .filter(|(mb, _)| mb != &current_merge_base)
        .collect();

    if old_scopes.is_empty() {
        return Ok(None);
    }

    // 2. Pick the most recent old scope (first in the list, ordered by
    //    most recent reviewed_at).
    let Some((old_merge_base, old_reviews)) = old_scopes.into_iter().next() else {
        return Ok(None);
    };

    // 3. Run the migration logic on a blocking thread (git operations).
    let worktree = ctx.worktree.clone();
    let new_merge_base = current_merge_base.clone();
    let old_mb = old_merge_base.clone();

    let migration_result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let repo = git::Repo::open(&worktree)?;

        // Get the list of currently changed files so we know which files
        // are still relevant in the new scope.
        let current_changes = repo.list_changed_files_workdir(&new_merge_base)?;
        let current_paths: std::collections::HashSet<String> =
            current_changes.iter().map(|c| c.path.clone()).collect();

        let mut migrated: Vec<(String, String, String)> = Vec::new(); // (file_path, diff_hash, reviewed_commit)

        for (file_path, old_review) in &old_reviews {
            // Skip files no longer in the changed set.
            if !current_paths.contains(file_path) {
                continue;
            }

            let reviewed_commit = &old_review.reviewed_commit;

            if !reviewed_commit.is_empty() && repo.commit_exists(reviewed_commit) {
                // Old reviewed_commit is resolvable.
                // Compare blob at reviewed_commit vs blob at current HEAD.
                let old_blob = repo.file_blob_hash(reviewed_commit, file_path)?;
                let new_blob = repo.file_blob_hash("HEAD", file_path).ok().flatten();

                if old_blob == new_blob {
                    // File content unchanged after rebase.
                    // Recompute diff_hash against new merge_base.
                    let diff = repo.diff_file_workdir(&new_merge_base, file_path)?;
                    migrated.push((
                        file_path.clone(),
                        diff.diff_hash,
                        repo.resolve_commit("HEAD")?,
                    ));
                } else {
                    // File content changed during rebase.
                    // Migrate as Changed — keep the old reviewed_commit so
                    // the diff shows only what changed since the review.
                    // Store the reviewed-state diff hash so current diffs
                    // compare unequal and list as Changed.
                    let diff = repo.diff_file(&new_merge_base, reviewed_commit, file_path)?;
                    migrated.push((file_path.clone(), diff.diff_hash, reviewed_commit.clone()));
                }
            } else {
                // reviewed_commit is empty or GC'd.
                // Fall back to diff_hash comparison.
                let diff = repo.diff_file_workdir(&new_merge_base, file_path)?;
                if diff.diff_hash == old_review.diff_hash {
                    // Diff unchanged — keep as reviewed.
                    migrated.push((file_path.clone(), diff.diff_hash, String::new()));
                }
                // If diff_hash differs, treat as unreviewed (don't migrate).
            }
        }

        Ok((migrated, old_mb))
    })
    .await??;

    let (migrated_entries, old_mb) = migration_result;

    if migrated_entries.is_empty() {
        // Nothing to migrate — clean up old scope anyway.
        let db_guard = db.lock().await;
        db_guard.clear_reviews(&old_mb, &head_ref)?;
        return Ok(None);
    }

    // 4. Store migrated reviews under the new scope and delete old scope.
    let migrated_count = migrated_entries.len();
    let mut new_reviews = HashMap::new();

    {
        let db_guard = db.lock().await;
        for (file_path, diff_hash, reviewed_commit) in &migrated_entries {
            let review = db_guard.store_review(
                &current_merge_base,
                &head_ref,
                file_path,
                diff_hash,
                reviewed_commit,
            )?;
            new_reviews.insert(file_path.clone(), review);
        }
        // Delete old scope records.
        db_guard.clear_reviews(&old_mb, &head_ref)?;
    }

    // 5. Broadcast migration notification.
    let _ = notify_tx.send(Notification {
        base_ref: current_merge_base,
        head_ref: head_ref.clone(),
        kind: NotificationKind::ReviewsMigrated {
            count: migrated_count,
        },
    });

    eprintln!(
        "Migrated {} review(s) from old scope (merge_base: {}..)",
        migrated_count,
        &old_mb[..8.min(old_mb.len())]
    );

    Ok(Some(new_reviews))
}

// ---------------------------------------------------------------------------
// get_file_diff
// ---------------------------------------------------------------------------

pub async fn handle_get_file_diff(
    params: &serde_json::Value,
    id: &serde_json::Value,
    ctx: &ConnectionContext,
) -> JsonRpcResponse {
    let diff_params: review_types::GetFileDiffParams = match serde_json::from_value(params.clone())
    {
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
    let merge_base = ctx.merge_base.to_string();
    let root = matches!(ctx.review_base, git::ReviewBase::Root { .. });
    let algorithm = ctx.diff_algorithm;
    let file_path = diff_params.file_path;

    let git_result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let repo = git::Repo::open(&worktree)?;
        let base = if root {
            git::DiffBase::EmptyTree
        } else {
            git::DiffBase::Commit(&merge_base)
        };
        let diff = repo.diff_file_workdir_opts_for_base(base, &file_path, algorithm, false)?;
        Ok(review_types::GetFileDiffResult { diff })
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
        Ok(Err(e)) => JsonRpcResponse::error(id.clone(), ERR_INTERNAL, format!("{e:#}")),
        Err(e) => {
            JsonRpcResponse::error(id.clone(), ERR_INTERNAL, format!("Git task panicked: {e}"))
        }
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
    notify_tx: &broadcast::Sender<Notification>,
) -> JsonRpcResponse {
    let p: review_types::MarkReviewedParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INVALID_PARAMS,
                format!("Invalid params: {e}"),
            );
        }
    };

    // Compute the current diff hash using the same patience algorithm as the
    // list endpoints so reviewed/unreviewed comparisons stay consistent.
    let worktree = ctx.worktree.clone();
    let merge_base = ctx.merge_base.to_string();
    let root = matches!(ctx.review_base, git::ReviewBase::Root { .. });
    let algorithm = crate::config::DiffAlgorithm::Patience;
    let head_ref = ctx.head_scope_key();
    let file_path = p.file_path.clone();

    let (diff_hash, reviewed_commit) = {
        let wt = worktree.clone();
        let mb = merge_base.clone();
        let fp = file_path.clone();
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(String, String)> {
            let repo = git::Repo::open(&wt)?;
            let base = if root {
                git::DiffBase::EmptyTree
            } else {
                git::DiffBase::Commit(&mb)
            };
            let diff = repo.diff_file_workdir_opts_for_base(base, &fp, algorithm, false)?;
            let head_oid = repo.resolve_commit("HEAD")?;
            Ok((diff.diff_hash, head_oid))
        })
        .await;

        match result {
            Ok(Ok(pair)) => pair,
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
        match db_guard.store_review(
            &merge_base,
            &head_ref,
            &file_path,
            &diff_hash,
            &reviewed_commit,
        ) {
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

    let result = review_types::ReviewActionResult {
        file_path: file_path.clone(),
        status: review_types::ReviewStatus::Reviewed {
            at: reviewed_at,
            reviewed_commit: Some(reviewed_commit),
        },
    };

    // Broadcast notification with the new status so clients can patch
    // locally without reloading every file diff.
    let _ = notify_tx.send(Notification {
        base_ref: merge_base,
        head_ref,
        kind: NotificationKind::ReviewChanged {
            file_path: file_path.clone(),
            status: Some(result.status.clone()),
        },
    });

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
    notify_tx: &broadcast::Sender<Notification>,
) -> JsonRpcResponse {
    let p: review_types::UnmarkReviewedParams = match serde_json::from_value(params.clone()) {
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
    let merge_base = ctx.merge_base.to_string();
    let head_ref = ctx.head_scope_key();
    {
        let db_guard = db.lock().await;
        if let Err(e) = db_guard.remove_review(&merge_base, &head_ref, &p.file_path) {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INTERNAL,
                format!("Failed to remove review: {e:#}"),
            );
        }
    }

    let result = review_types::ReviewActionResult {
        file_path: p.file_path.clone(),
        status: review_types::ReviewStatus::Unreviewed,
    };

    // Broadcast notification with the new status so clients can patch
    // locally without reloading every file diff.
    let _ = notify_tx.send(Notification {
        base_ref: merge_base,
        head_ref,
        kind: NotificationKind::ReviewChanged {
            file_path: p.file_path.clone(),
            status: Some(result.status.clone()),
        },
    });

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
    notify_tx: &broadcast::Sender<Notification>,
) -> JsonRpcResponse {
    let merge_base = ctx.merge_base.to_string();
    let head_ref = ctx.head_scope_key();
    let cleared = {
        let db_guard = db.lock().await;
        match db_guard.clear_reviews(&merge_base, &head_ref) {
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
    let _ = notify_tx.send(Notification {
        base_ref: merge_base,
        head_ref,
        kind: NotificationKind::ReviewsCleared,
    });

    let result = review_types::ResetReviewsResult { cleared };

    match serde_json::to_value(result) {
        Ok(v) => JsonRpcResponse::success(id.clone(), v),
        Err(e) => JsonRpcResponse::error(
            id.clone(),
            ERR_INTERNAL,
            format!("Serialization error: {e}"),
        ),
    }
}

pub async fn handle_set_merge_base(
    params: &serde_json::Value,
    id: &serde_json::Value,
    state: &Arc<ServerState>,
    ctx: &mut ConnectionContext,
) -> JsonRpcResponse {
    let params: review_types::SetMergeBaseParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INVALID_PARAMS,
                format!("Invalid set_merge_base params: {e}"),
            );
        }
    };
    let refspec = if params.refspec.trim().is_empty() {
        "HEAD".to_string()
    } else {
        params.refspec.trim().to_string()
    };

    let worktree = ctx.worktree.clone();
    let refspec_for_git = refspec.clone();
    let git_result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let repo = git::Repo::open(&worktree)?;
        let resolved = git::CommitId::new(repo.resolve_commit(&refspec_for_git)?);
        Ok(resolved)
    })
    .await;

    let merge_base = match git_result {
        Ok(Ok(commit)) => commit,
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

    let old_ctx = ctx.clone();
    ctx.base_ref = refspec.clone();
    ctx.review_base = git::ReviewBase::Anonymous {
        input: refspec,
        resolved_commit: merge_base.clone(),
    };
    ctx.merge_base = merge_base;
    state.replace_session(&old_ctx, ctx).await;

    json_response(
        id,
        review_types::SetMergeBaseResult {
            context: protocol_context(ctx),
        },
    )
}

// ---------------------------------------------------------------------------
// comments
// ---------------------------------------------------------------------------

pub async fn handle_create_comment(
    params: &serde_json::Value,
    id: &serde_json::Value,
    ctx: &ConnectionContext,
    db: &Arc<Mutex<Database>>,
    notify_tx: &broadcast::Sender<Notification>,
) -> JsonRpcResponse {
    let p: review_types::CreateCommentParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INVALID_PARAMS,
                format!("Invalid params: {e}"),
            );
        }
    };

    let merge_base = ctx.merge_base.to_string();
    let head_ref = ctx.head_scope_key();
    let file_path = match p.anchor.segments.first() {
        Some(segment) => segment.file_path.clone(),
        None => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INVALID_PARAMS,
                "Comment anchor must contain at least one segment".to_string(),
            );
        }
    };
    let anchor = match new_comment_anchor_from_protocol(ctx, &p.anchor).await {
        Ok(anchor) => anchor,
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INTERNAL,
                format!("Failed to prepare comment anchor: {e:#}"),
            );
        }
    };
    let new = NewComment {
        merge_base: merge_base.clone(),
        head_ref: head_ref.clone(),
        created_head_commit: ctx.head.resolved_commit().to_string(),
        file_path,
        body: p.body,
        anchor,
    };

    let stored = {
        let db_guard = db.lock().await;
        match db_guard.create_comment(&new) {
            Ok(comment) => comment,
            Err(e) => {
                return JsonRpcResponse::error(
                    id.clone(),
                    ERR_INTERNAL,
                    format!("Failed to create comment: {e:#}"),
                );
            }
        }
    };

    let _ = notify_tx.send(Notification {
        base_ref: merge_base,
        head_ref,
        kind: NotificationKind::CommentChanged {
            comment_id: stored.id,
        },
    });

    comment_response(id, stored)
}

pub async fn handle_list_comments(
    params: &serde_json::Value,
    id: &serde_json::Value,
    ctx: &ConnectionContext,
    db: &Arc<Mutex<Database>>,
) -> JsonRpcResponse {
    let p: review_types::ListCommentsParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INVALID_PARAMS,
                format!("Invalid params: {e}"),
            );
        }
    };

    let head_ref = ctx.head_scope_key();
    let requested_file_path = p.file_path.clone();
    let comments = {
        let db_guard = db.lock().await;
        match db_guard.list_comments(
            ctx.merge_base_key(),
            &head_ref,
            None,
            p.include_resolved,
            p.include_previous_bases,
        ) {
            Ok(comments) => comments,
            Err(e) => {
                return JsonRpcResponse::error(
                    id.clone(),
                    ERR_INTERNAL,
                    format!("Failed to list comments: {e:#}"),
                );
            }
        }
    };

    let comments = match reanchor_comments(ctx, db, comments).await {
        Ok(comments) => comments,
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INTERNAL,
                format!("Failed to re-anchor comments: {e:#}"),
            );
        }
    };
    let comments = if let Some(file_path) = requested_file_path {
        comments
            .into_iter()
            .filter(|comment| comment_has_file_path(comment, &file_path))
            .collect()
    } else {
        comments
    };

    let result = review_types::ListCommentsResult {
        comments: comments.into_iter().map(comment_from_stored).collect(),
    };

    json_response(id, result)
}

pub async fn handle_get_comment(
    params: &serde_json::Value,
    id: &serde_json::Value,
    ctx: &ConnectionContext,
    db: &Arc<Mutex<Database>>,
) -> JsonRpcResponse {
    let p: review_types::GetCommentParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INVALID_PARAMS,
                format!("Invalid params: {e}"),
            );
        }
    };

    let Some(stored) = load_comment_in_scope(ctx, db, p.id).await else {
        return JsonRpcResponse::error(
            id.clone(),
            ERR_INVALID_PARAMS,
            format!("Comment {} was not found in the current review scope", p.id),
        );
    };

    let stored = match reanchor_comment(ctx, db, stored).await {
        Ok(comment) => comment,
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INTERNAL,
                format!("Failed to re-anchor comment: {e:#}"),
            );
        }
    };

    comment_response(id, stored)
}

pub async fn handle_update_comment(
    params: &serde_json::Value,
    id: &serde_json::Value,
    ctx: &ConnectionContext,
    db: &Arc<Mutex<Database>>,
    notify_tx: &broadcast::Sender<Notification>,
) -> JsonRpcResponse {
    let p: review_types::UpdateCommentParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INVALID_PARAMS,
                format!("Invalid params: {e}"),
            );
        }
    };

    if load_comment_in_scope(ctx, db, p.id).await.is_none() {
        return JsonRpcResponse::error(
            id.clone(),
            ERR_INVALID_PARAMS,
            format!("Comment {} was not found in the current review scope", p.id),
        );
    }

    {
        let db_guard = db.lock().await;
        match db_guard.update_comment(p.id, &p.body) {
            Ok(true) => {}
            Ok(false) => {
                return JsonRpcResponse::error(
                    id.clone(),
                    ERR_INVALID_PARAMS,
                    format!("Comment {} was not found", p.id),
                );
            }
            Err(e) => {
                return JsonRpcResponse::error(
                    id.clone(),
                    ERR_INTERNAL,
                    format!("Failed to update comment: {e:#}"),
                );
            }
        }
    }

    notify_comment_changed(ctx, notify_tx, p.id);
    let Some(stored) = load_comment_in_scope(ctx, db, p.id).await else {
        return JsonRpcResponse::error(
            id.clone(),
            ERR_INTERNAL,
            format!("Comment {} disappeared after update", p.id),
        );
    };

    comment_response(id, stored)
}

pub async fn handle_resolve_comment(
    params: &serde_json::Value,
    id: &serde_json::Value,
    ctx: &ConnectionContext,
    db: &Arc<Mutex<Database>>,
    notify_tx: &broadcast::Sender<Notification>,
) -> JsonRpcResponse {
    let p: review_types::ResolveCommentParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INVALID_PARAMS,
                format!("Invalid params: {e}"),
            );
        }
    };

    update_comment_resolved(id, ctx, db, notify_tx, p.id, true).await
}

pub async fn handle_unresolve_comment(
    params: &serde_json::Value,
    id: &serde_json::Value,
    ctx: &ConnectionContext,
    db: &Arc<Mutex<Database>>,
    notify_tx: &broadcast::Sender<Notification>,
) -> JsonRpcResponse {
    let p: review_types::UnresolveCommentParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INVALID_PARAMS,
                format!("Invalid params: {e}"),
            );
        }
    };

    update_comment_resolved(id, ctx, db, notify_tx, p.id, false).await
}

pub async fn handle_delete_comment(
    params: &serde_json::Value,
    id: &serde_json::Value,
    ctx: &ConnectionContext,
    db: &Arc<Mutex<Database>>,
    notify_tx: &broadcast::Sender<Notification>,
) -> JsonRpcResponse {
    let p: review_types::DeleteCommentParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INVALID_PARAMS,
                format!("Invalid params: {e}"),
            );
        }
    };

    if load_comment_in_scope(ctx, db, p.id).await.is_none() {
        return JsonRpcResponse::error(
            id.clone(),
            ERR_INVALID_PARAMS,
            format!("Comment {} was not found in the current review scope", p.id),
        );
    }

    let deleted = {
        let db_guard = db.lock().await;
        match db_guard.delete_comment(p.id) {
            Ok(deleted) => deleted,
            Err(e) => {
                return JsonRpcResponse::error(
                    id.clone(),
                    ERR_INTERNAL,
                    format!("Failed to delete comment: {e:#}"),
                );
            }
        }
    };

    notify_comment_changed(ctx, notify_tx, p.id);
    json_response(id, review_types::DeleteCommentResult { deleted })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn json_response<T: serde::Serialize>(id: &serde_json::Value, result: T) -> JsonRpcResponse {
    match serde_json::to_value(result) {
        Ok(v) => JsonRpcResponse::success(id.clone(), v),
        Err(e) => JsonRpcResponse::error(
            id.clone(),
            ERR_INTERNAL,
            format!("Serialization error: {e}"),
        ),
    }
}

async fn current_file_hash(ctx: &ConnectionContext, file_path: &str) -> anyhow::Result<String> {
    Ok(match current_file_content(ctx, file_path).await? {
        Some((_, hash)) => hash,
        None => String::new(),
    })
}

async fn new_comment_anchor_from_protocol(
    ctx: &ConnectionContext,
    anchor: &review_types::CommentAnchor,
) -> anyhow::Result<NewCommentAnchor> {
    let validated = review_types::CommentAnchor::try_with_aggregate_status(
        anchor.segments.clone(),
        anchor.aggregate_status,
    )?;
    let mut segments = Vec::with_capacity(validated.segments.len());
    for segment in validated.segments {
        let file_blob_sha =
            file_hash_for_anchor_side(ctx, segment.side, &segment.file_path).await?;
        segments.push(NewCommentAnchorSegment {
            side: segment.side,
            file_path: segment.file_path,
            file_blob_sha,
            line_start: segment.line_start,
            line_end: segment.line_end,
            char_start: segment.char_start,
            char_end: segment.char_end,
            anchor_text: segment.anchor_text,
            context_before: segment.context_before,
            context_after: segment.context_after,
            placement_status: segment.placement_status,
            match_method: segment.match_method,
        });
    }

    Ok(NewCommentAnchor {
        segments,
        aggregate_status: validated.aggregate_status,
    })
}

async fn file_hash_for_anchor_side(
    ctx: &ConnectionContext,
    side: review_types::CommentAnchorSide,
    file_path: &str,
) -> anyhow::Result<String> {
    match side {
        review_types::CommentAnchorSide::Head => current_file_hash(ctx, file_path).await,
        review_types::CommentAnchorSide::Base => base_file_hash(ctx, file_path).await,
    }
}

async fn base_file_hash(ctx: &ConnectionContext, file_path: &str) -> anyhow::Result<String> {
    Ok(match base_file_content(ctx, file_path).await? {
        Some((_, hash)) => hash,
        None => String::new(),
    })
}

async fn base_file_content(
    ctx: &ConnectionContext,
    file_path: &str,
) -> anyhow::Result<Option<(String, String)>> {
    if matches!(ctx.review_base, git::ReviewBase::Root { .. }) {
        return Ok(None);
    }
    commit_file_content(ctx, &ctx.merge_base.to_string(), file_path).await
}

async fn commit_file_content(
    ctx: &ConnectionContext,
    refspec: &str,
    file_path: &str,
) -> anyhow::Result<Option<(String, String)>> {
    let worktree = ctx.worktree.clone();
    let refspec = refspec.to_string();
    let file_path = file_path.to_string();
    let content = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<String>> {
        let repo = git::Repo::open(&worktree)?;
        repo.file_content(&refspec, &file_path)
    })
    .await??;

    Ok(content.map(|content| {
        let hash = hash_content(&content);
        (content, hash)
    }))
}

async fn current_file_content(
    ctx: &ConnectionContext,
    file_path: &str,
) -> anyhow::Result<Option<(String, String)>> {
    let worktree = ctx.worktree.clone();
    let file_path = file_path.to_string();
    let content = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<String>> {
        let repo = git::Repo::open(&worktree)?;
        repo.file_content_workdir(&file_path)
    })
    .await??;

    Ok(content.map(|content| {
        let hash = hash_content(&content);
        (content, hash)
    }))
}

fn hash_content(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("workdir:{:x}", hasher.finalize())
}

async fn reanchor_comments(
    ctx: &ConnectionContext,
    db: &Arc<Mutex<Database>>,
    comments: Vec<StoredComment>,
) -> anyhow::Result<Vec<StoredComment>> {
    let range = CommentReanchorRange::current_session(ctx);
    let mut reanchored = Vec::with_capacity(comments.len());
    for comment in comments {
        reanchored.push(reanchor_comment_for_range(ctx, db, &range, comment).await?);
    }
    Ok(reanchored)
}

async fn reanchor_comment(
    ctx: &ConnectionContext,
    db: &Arc<Mutex<Database>>,
    comment: StoredComment,
) -> anyhow::Result<StoredComment> {
    let range = CommentReanchorRange::current_session(ctx);
    reanchor_comment_for_range(ctx, db, &range, comment).await
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommentReanchorRange {
    base: ReanchorEndpoint,
    head: ReanchorEndpoint,
}

impl CommentReanchorRange {
    fn current_session(ctx: &ConnectionContext) -> Self {
        Self {
            base: ReanchorEndpoint::Commit {
                refspec: ctx.merge_base.to_string(),
            },
            head: ReanchorEndpoint::Worktree {
                compare_to: Some(ctx.head.resolved_commit().to_string()),
            },
        }
    }

    fn endpoint_for_side(&self, side: review_types::CommentAnchorSide) -> &ReanchorEndpoint {
        match side {
            review_types::CommentAnchorSide::Base => &self.base,
            review_types::CommentAnchorSide::Head => &self.head,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReanchorEndpoint {
    #[allow(dead_code)]
    Root,
    Commit {
        refspec: String,
    },
    Worktree {
        compare_to: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReanchorFileContent {
    file_path: String,
    content: String,
    file_blob_sha: String,
}

async fn reanchor_comment_for_range(
    ctx: &ConnectionContext,
    db: &Arc<Mutex<Database>>,
    range: &CommentReanchorRange,
    comment: StoredComment,
) -> anyhow::Result<StoredComment> {
    if comment.resolved {
        return Ok(comment);
    }

    if range_preferred_blob_matches(ctx, range, &comment).await? {
        return Ok(comment);
    }

    let anchor = resolve_anchor_for_range(ctx, range, &comment).await?;

    {
        let db_guard = db.lock().await;
        db_guard.insert_anchor_version(&NewAnchorVersion {
            comment_id: comment.id,
            anchor: anchor.clone(),
        })?;
    }

    Ok(apply_anchor(comment, &anchor))
}

async fn range_preferred_blob_matches(
    ctx: &ConnectionContext,
    range: &CommentReanchorRange,
    comment: &StoredComment,
) -> anyhow::Result<bool> {
    if comment.file_blob_sha.is_empty() {
        return Ok(false);
    }

    let Some(segment) = preferred_anchor_segment(comment) else {
        return Ok(false);
    };
    let Some(content) = range_file_content_for_segment(ctx, range, comment, segment).await? else {
        return Ok(comment.file_blob_sha.is_empty());
    };
    Ok(content.file_path == segment.file_path && content.file_blob_sha == comment.file_blob_sha)
}

fn preferred_anchor_segment(
    comment: &StoredComment,
) -> Option<&review_types::CommentAnchorSegment> {
    comment
        .anchor
        .segments
        .iter()
        .find(|segment| segment.side == review_types::CommentAnchorSide::Head)
        .or_else(|| comment.anchor.segments.first())
}

async fn resolve_anchor_for_range(
    ctx: &ConnectionContext,
    range: &CommentReanchorRange,
    comment: &StoredComment,
) -> anyhow::Result<NewCommentAnchor> {
    let mut segments = Vec::with_capacity(comment.anchor.segments.len());
    for segment in &comment.anchor.segments {
        let content = range_file_content_for_segment(ctx, range, comment, segment).await?;
        let resolved = match content {
            Some(content) => {
                let mut resolved_segment = segment.clone();
                resolved_segment.file_path = content.file_path;
                let adjusted_hint =
                    adjusted_hint_for_range_segment(ctx, range, &resolved_segment).await;
                resolve_anchor_segment(
                    &resolved_segment,
                    &content.content,
                    content.file_blob_sha,
                    adjusted_hint,
                )
            }
            None => orphaned_anchor_segment(segment, String::new()),
        };
        segments.push(resolved);
    }

    Ok(NewCommentAnchor {
        aggregate_status: aggregate_status_for_new_segments(&segments),
        segments,
    })
}

async fn range_file_content_for_segment(
    ctx: &ConnectionContext,
    range: &CommentReanchorRange,
    comment: &StoredComment,
    segment: &review_types::CommentAnchorSegment,
) -> anyhow::Result<Option<ReanchorFileContent>> {
    if let Some(content) = range_file_content(ctx, range, segment.side, &segment.file_path).await? {
        return Ok(Some(content));
    }

    let Some(renamed_path) = renamed_path_for_segment(ctx, range, comment, segment).await? else {
        return Ok(None);
    };
    if renamed_path == segment.file_path {
        return Ok(None);
    }

    range_file_content(ctx, range, segment.side, &renamed_path).await
}

async fn range_file_content(
    ctx: &ConnectionContext,
    range: &CommentReanchorRange,
    side: review_types::CommentAnchorSide,
    file_path: &str,
) -> anyhow::Result<Option<ReanchorFileContent>> {
    endpoint_file_content(ctx, range.endpoint_for_side(side), file_path).await
}

async fn endpoint_file_content(
    ctx: &ConnectionContext,
    endpoint: &ReanchorEndpoint,
    file_path: &str,
) -> anyhow::Result<Option<ReanchorFileContent>> {
    match endpoint {
        ReanchorEndpoint::Root => Ok(None),
        ReanchorEndpoint::Worktree { .. } => {
            Ok(current_file_content(ctx, file_path)
                .await?
                .map(|(content, file_blob_sha)| ReanchorFileContent {
                    file_path: file_path.to_string(),
                    content,
                    file_blob_sha,
                }))
        }
        ReanchorEndpoint::Commit { refspec } => Ok(commit_file_content(ctx, refspec, file_path)
            .await?
            .map(|(content, file_blob_sha)| ReanchorFileContent {
                file_path: file_path.to_string(),
                content,
                file_blob_sha,
            })),
    }
}

async fn renamed_path_for_segment(
    ctx: &ConnectionContext,
    range: &CommentReanchorRange,
    comment: &StoredComment,
    segment: &review_types::CommentAnchorSegment,
) -> anyhow::Result<Option<String>> {
    let endpoint = range.endpoint_for_side(segment.side);
    match endpoint {
        ReanchorEndpoint::Root => Ok(None),
        ReanchorEndpoint::Commit { refspec } => {
            let Some(source_ref) = segment_source_ref(comment, segment.side) else {
                return Ok(None);
            };
            renamed_path_between_refs(ctx, &source_ref, refspec, &segment.file_path).await
        }
        ReanchorEndpoint::Worktree {
            compare_to: Some(compare_to),
        } => {
            let mut candidate = segment.file_path.clone();
            if let Some(source_ref) = segment_source_ref(comment, segment.side) {
                if let Some(committed_path) =
                    renamed_path_between_refs(ctx, &source_ref, compare_to, &candidate).await?
                {
                    candidate = committed_path;
                }
            }
            if let Some(worktree_path) =
                renamed_path_between_ref_and_worktree(ctx, compare_to, &candidate).await?
            {
                candidate = worktree_path;
            }
            if candidate == segment.file_path {
                Ok(None)
            } else {
                Ok(Some(candidate))
            }
        }
        ReanchorEndpoint::Worktree { compare_to: None } => Ok(None),
    }
}

fn segment_source_ref(
    comment: &StoredComment,
    side: review_types::CommentAnchorSide,
) -> Option<String> {
    match side {
        review_types::CommentAnchorSide::Base => {
            (!comment.merge_base.is_empty()).then(|| comment.merge_base.clone())
        }
        review_types::CommentAnchorSide::Head => {
            (!comment.created_head_commit.is_empty()).then(|| comment.created_head_commit.clone())
        }
    }
}

async fn renamed_path_between_refs(
    ctx: &ConnectionContext,
    from_ref: &str,
    to_ref: &str,
    file_path: &str,
) -> anyhow::Result<Option<String>> {
    if from_ref == to_ref {
        return Ok(None);
    }

    let worktree = ctx.worktree.clone();
    let from_ref = from_ref.to_string();
    let to_ref = to_ref.to_string();
    let file_path = file_path.to_string();

    tokio::task::spawn_blocking(move || -> anyhow::Result<Option<String>> {
        let repo = git::Repo::open(&worktree)?;
        let changes = repo.list_changed_files(&from_ref, &to_ref)?;
        Ok(renamed_path_from_changes(&changes, &file_path))
    })
    .await?
}

async fn renamed_path_between_ref_and_worktree(
    ctx: &ConnectionContext,
    from_ref: &str,
    file_path: &str,
) -> anyhow::Result<Option<String>> {
    let worktree = ctx.worktree.clone();
    let from_ref = from_ref.to_string();
    let file_path = file_path.to_string();

    tokio::task::spawn_blocking(move || -> anyhow::Result<Option<String>> {
        let repo = git::Repo::open(&worktree)?;
        let changes = repo.list_changed_files_workdir(&from_ref)?;
        Ok(renamed_path_from_changes(&changes, &file_path))
    })
    .await?
}

fn renamed_path_from_changes(
    changes: &[review_types::FileChange],
    file_path: &str,
) -> Option<String> {
    changes
        .iter()
        .find(|change| {
            change.kind == review_types::ChangeKind::Renamed
                && change.old_path.as_deref() == Some(file_path)
        })
        .map(|change| change.path.clone())
}

async fn adjusted_hint_for_range_segment(
    ctx: &ConnectionContext,
    range: &CommentReanchorRange,
    segment: &review_types::CommentAnchorSegment,
) -> Option<usize> {
    match range.endpoint_for_side(segment.side) {
        ReanchorEndpoint::Worktree {
            compare_to: Some(compare_to),
        } => {
            compute_adjusted_hint_for_line(ctx, &segment.file_path, segment.line_start, compare_to)
                .await
        }
        ReanchorEndpoint::Root
        | ReanchorEndpoint::Worktree { compare_to: None }
        | ReanchorEndpoint::Commit { .. } => None,
    }
}

async fn compute_adjusted_hint_for_line(
    ctx: &ConnectionContext,
    file_path: &str,
    line_start: i64,
    compare_to: &str,
) -> Option<usize> {
    let stored_line = line_start
        .checked_sub(1)
        .and_then(|l| usize::try_from(l).ok())?;

    let worktree = ctx.worktree.clone();
    let head_commit = compare_to.to_string();
    let fp = file_path.to_string();

    let diff = tokio::task::spawn_blocking(move || -> Option<_> {
        let repo = git::Repo::open(&worktree).ok()?;
        repo.diff_file_workdir(&head_commit, &fp).ok()
    })
    .await
    .ok()??;

    // The stored line_start is a 1-based "new side" line number from the
    // last anchor version.  Walk the diff hunks to translate it from the
    // old new-side position to the current new-side position.  Since we
    // are diffing head_commit vs current_workdir, old_start/old_lines
    // correspond to the head-commit content and new_start/new_lines to
    // the current workdir.
    //
    // However, the comment was anchored against a *previous* workdir
    // state, not the head commit.  Its line number is relative to that
    // old workdir.  Both the old and the current workdir are "new side"
    // relative to the head commit, so we need to translate:
    //
    //   old_workdir_line  →  head_commit_line  →  new_workdir_line
    //
    // We don't have the old workdir content, but we can approximate by
    // treating the head commit as a stable reference.  If the file at
    // HEAD hasn't changed relative to the old workdir at the comment's
    // position, the translation is exact.

    Some(translate_line_through_hunks(&diff.hunks, stored_line))
}

/// Translate a 0-indexed line number from the old side of a diff to the
/// new side, accounting for insertions and deletions in earlier hunks.
fn translate_line_through_hunks(hunks: &[review_types::DiffHunk], old_line: usize) -> usize {
    // 1-based line number for comparison with hunk headers.
    let old_1 = (old_line + 1) as u32;
    let mut offset: i64 = 0;

    for hunk in hunks {
        let hunk_old_end = hunk.old_start + hunk.old_lines;
        if old_1 < hunk.old_start {
            // Line is before this hunk; accumulated offset applies.
            break;
        }
        if old_1 < hunk_old_end {
            // Line falls inside this hunk.  Count additions and
            // deletions before the target line within the hunk.
            let mut old_cursor = hunk.old_start;
            let mut net: i64 = 0;
            for line in &hunk.lines {
                if old_cursor > old_1 {
                    break;
                }
                match line.kind {
                    review_types::LineKind::Context => {
                        old_cursor += 1;
                    }
                    review_types::LineKind::Deletion => {
                        old_cursor += 1;
                        net -= 1;
                    }
                    review_types::LineKind::Addition => {
                        net += 1;
                    }
                }
            }
            offset += net;
            break;
        }
        // Line is after this hunk; accumulate net change.
        offset += hunk.new_lines as i64 - hunk.old_lines as i64;
    }

    let adjusted = old_line as i64 + offset;
    if adjusted < 0 { 0 } else { adjusted as usize }
}

#[cfg(test)]
fn resolve_anchor(
    comment: &StoredComment,
    content: &str,
    file_blob_sha: String,
    adjusted_hint: Option<usize>,
) -> NewCommentAnchor {
    let segments: Vec<NewCommentAnchorSegment> = comment
        .anchor
        .segments
        .iter()
        .map(|segment| {
            resolve_anchor_segment(segment, content, file_blob_sha.clone(), adjusted_hint)
        })
        .collect();

    NewCommentAnchor {
        aggregate_status: aggregate_status_for_new_segments(&segments),
        segments,
    }
}

fn resolve_anchor_segment(
    segment: &review_types::CommentAnchorSegment,
    content: &str,
    file_blob_sha: String,
    adjusted_hint: Option<usize>,
) -> NewCommentAnchorSegment {
    resolve_anchor_segment_with_shape(segment, content, file_blob_sha, adjusted_hint).segment
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReanchoredRangeShape {
    ExactSelection,
    ReducedSelection,
    ContextInterior,
    CollapsedBoundary,
}

struct ResolvedAnchorSegment {
    segment: NewCommentAnchorSegment,
    #[allow(dead_code)]
    range_shape: Option<ReanchoredRangeShape>,
}

fn resolve_anchor_segment_with_shape(
    segment: &review_types::CommentAnchorSegment,
    content: &str,
    file_blob_sha: String,
    adjusted_hint: Option<usize>,
) -> ResolvedAnchorSegment {
    let lines = content_lines(content);
    let anchor_lines = anchor_lines(&segment.anchor_text);
    let span = anchor_lines.len().max(1);
    let stored_index = segment
        .line_start
        .checked_sub(1)
        .and_then(|line| usize::try_from(line).ok());

    // Try exact match at the adjusted position first (if we computed
    // one from the diff), then fall back to the stored position.
    let try_indices: [Option<usize>; 2] = [adjusted_hint, stored_index];
    for candidate in try_indices.into_iter().flatten() {
        if matches_sequence(&lines, candidate, &anchor_lines) {
            return ResolvedAnchorSegment {
                segment: anchor_segment_from_range(
                    segment,
                    &lines,
                    candidate,
                    span,
                    file_blob_sha,
                    review_types::AnchorStatus::Anchored,
                ),
                range_shape: Some(ReanchoredRangeShape::ExactSelection),
            };
        }
    }

    let hint = adjusted_hint.or(stored_index).unwrap_or(0);

    if let Some(index) = find_sequence_nearest(&lines, &anchor_lines, hint) {
        return ResolvedAnchorSegment {
            segment: anchor_segment_from_range(
                segment,
                &lines,
                index,
                span,
                file_blob_sha,
                review_types::AnchorStatus::Shifted,
            ),
            range_shape: Some(ReanchoredRangeShape::ExactSelection),
        };
    }

    if let Some((index, reduced_span)) =
        find_reduced_selection_match_nearest(&lines, &anchor_lines, hint)
    {
        return ResolvedAnchorSegment {
            segment: anchor_segment_from_range(
                segment,
                &lines,
                index,
                reduced_span,
                file_blob_sha,
                review_types::AnchorStatus::Approximate,
            ),
            range_shape: Some(ReanchoredRangeShape::ReducedSelection),
        };
    }

    if let Some(decision) = find_context_range_match(
        &segment.context_before,
        &segment.context_after,
        &lines,
        &anchor_lines,
        hint,
    ) {
        return ResolvedAnchorSegment {
            segment: anchor_segment_from_range(
                segment,
                &lines,
                decision.start,
                decision.span,
                file_blob_sha,
                review_types::AnchorStatus::Approximate,
            ),
            range_shape: Some(decision.shape),
        };
    }

    ResolvedAnchorSegment {
        segment: orphaned_anchor_segment(segment, file_blob_sha),
        range_shape: None,
    }
}

fn aggregate_status_for_new_segments(
    segments: &[NewCommentAnchorSegment],
) -> review_types::AnchorAggregateStatus {
    let anchored = segments
        .iter()
        .filter(|segment| segment.placement_status == review_types::AnchorPlacementStatus::Anchored)
        .count();
    if anchored == segments.len() {
        review_types::AnchorAggregateStatus::Anchored
    } else if anchored == 0 {
        review_types::AnchorAggregateStatus::Orphaned
    } else {
        review_types::AnchorAggregateStatus::Partial
    }
}

fn content_lines(content: &str) -> Vec<&str> {
    if content.is_empty() {
        vec![""]
    } else {
        content.lines().collect()
    }
}

fn anchor_lines(anchor_text: &str) -> Vec<&str> {
    if anchor_text.is_empty() {
        vec![""]
    } else {
        anchor_text.lines().collect()
    }
}

fn matches_sequence(lines: &[&str], index: usize, needle: &[&str]) -> bool {
    let Some(end) = index.checked_add(needle.len()) else {
        return false;
    };
    end <= lines.len() && &lines[index..end] == needle
}

fn find_sequence_nearest(lines: &[&str], needle: &[&str], hint: usize) -> Option<usize> {
    if needle.is_empty() || needle.len() > lines.len() {
        return None;
    }

    let last_start = lines.len() - needle.len();
    let hint = hint.min(last_start);

    // Search outward from hint: hint, hint±1, hint±2, …
    // The first match is guaranteed to be the nearest.
    for dist in 0..=last_start {
        let above = hint.checked_sub(dist);
        let below = hint.checked_add(dist).filter(|&i| i <= last_start);

        if let Some(i) = below {
            if matches_sequence(lines, i, needle) {
                return Some(i);
            }
        }
        if dist > 0 {
            if let Some(i) = above {
                if matches_sequence(lines, i, needle) {
                    return Some(i);
                }
            }
        }

        if above.is_none() && below.is_none() {
            break;
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineMatchCandidate {
    start: usize,
    span: usize,
    matched_lines: usize,
    unique_matched_lines: usize,
    distance_from_hint: usize,
}

impl LineMatchCandidate {
    fn is_preferred_to(self, other: &Self) -> bool {
        self.matched_lines > other.matched_lines
            || (self.matched_lines == other.matched_lines
                && (self.unique_matched_lines > other.unique_matched_lines
                    || (self.unique_matched_lines == other.unique_matched_lines
                        && (self.distance_from_hint < other.distance_from_hint
                            || (self.distance_from_hint == other.distance_from_hint
                                && self.start < other.start)))))
    }
}

fn find_reduced_selection_match_nearest(
    lines: &[&str],
    needle: &[&str],
    hint: usize,
) -> Option<(usize, usize)> {
    if needle.len() < 2 || lines.is_empty() {
        return None;
    }

    let hint = hint.min(lines.len().saturating_sub(1));
    let mut best: Option<LineMatchCandidate> = None;

    for start in 0..lines.len() {
        let Some(candidate) = reduced_line_match_candidate(lines, needle, start, lines.len(), hint)
        else {
            continue;
        };
        if best
            .as_ref()
            .is_none_or(|existing| candidate.is_preferred_to(existing))
        {
            best = Some(candidate);
        }
    }

    best.map(|candidate| (candidate.start, candidate.span))
}

fn reduced_line_match_candidate(
    lines: &[&str],
    needle: &[&str],
    start: usize,
    end: usize,
    hint: usize,
) -> Option<LineMatchCandidate> {
    let mut matched_lines = 0;
    let mut unique_matched_lines = 0;
    let mut first_match = None;
    let mut last_match = None;
    let mut needle_index = 0;

    for (line_index, line) in lines.iter().enumerate().take(end).skip(start) {
        while needle_index < needle.len() && needle[needle_index].trim().is_empty() {
            needle_index += 1;
        }

        let Some(match_index) = next_matching_selected_line(needle, needle_index, line) else {
            continue;
        };

        first_match.get_or_insert(line_index);
        last_match = Some(line_index);
        matched_lines += 1;
        if count_line_occurrences(lines, needle[match_index]) == 1 {
            unique_matched_lines += 1;
        }
        needle_index = match_index + 1;
    }

    if matched_lines == 0 || (unique_matched_lines == 0 && matched_lines < 2) {
        return None;
    }

    let first_match = first_match?;
    let last_match = last_match?;
    let distance_from_hint = if first_match >= hint {
        first_match - hint
    } else {
        hint - first_match
    };

    Some(LineMatchCandidate {
        start: first_match,
        span: last_match - first_match + 1,
        matched_lines,
        unique_matched_lines,
        distance_from_hint,
    })
}

fn next_matching_selected_line(needle: &[&str], start: usize, line: &str) -> Option<usize> {
    needle
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, needle_line)| {
            if needle_line.trim().is_empty() {
                None
            } else if *needle_line == line {
                Some(index)
            } else {
                None
            }
        })
}

fn count_line_occurrences(lines: &[&str], needle: &str) -> usize {
    lines.iter().filter(|line| **line == needle).count()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReanchoredRangeDecision {
    start: usize,
    span: usize,
    shape: ReanchoredRangeShape,
}

fn find_context_range_match(
    context_before: &str,
    context_after: &str,
    lines: &[&str],
    anchor_lines: &[&str],
    hint: usize,
) -> Option<ReanchoredRangeDecision> {
    let before = context_lines(context_before);
    let after = context_lines(context_after);

    if !before.is_empty() && !after.is_empty() {
        let mut candidates = Vec::new();
        for context_start in find_all_sequences(lines, &before) {
            let interior_start = context_start + before.len();
            for after_offset in find_all_sequences(&lines[interior_start..], &after) {
                let interior_span = after_offset;
                let decision = if interior_span == 0 {
                    ReanchoredRangeDecision {
                        start: interior_start,
                        span: 1,
                        shape: ReanchoredRangeShape::CollapsedBoundary,
                    }
                } else if let Some((start, span)) = find_reduced_selection_match_in_range(
                    lines,
                    anchor_lines,
                    interior_start,
                    interior_start + interior_span,
                    hint,
                ) {
                    ReanchoredRangeDecision {
                        start,
                        span,
                        shape: ReanchoredRangeShape::ReducedSelection,
                    }
                } else {
                    ReanchoredRangeDecision {
                        start: interior_start,
                        span: interior_span,
                        shape: ReanchoredRangeShape::ContextInterior,
                    }
                };
                candidates.push(decision);
            }
        }
        return unique_context_candidate(candidates);
    }

    if !before.is_empty() && after.is_empty() {
        let candidates: Vec<_> = find_all_sequences(lines, &before)
            .into_iter()
            .filter_map(|context_start| {
                let boundary = context_start + before.len();
                if boundary == lines.len() && boundary > 0 {
                    Some(ReanchoredRangeDecision {
                        start: boundary - 1,
                        span: 1,
                        shape: ReanchoredRangeShape::CollapsedBoundary,
                    })
                } else {
                    None
                }
            })
            .collect();
        return unique_context_candidate(candidates);
    }

    if before.is_empty() && !after.is_empty() {
        let candidates: Vec<_> = find_all_sequences(lines, &after)
            .into_iter()
            .filter_map(|context_start| {
                if context_start == 0 {
                    Some(ReanchoredRangeDecision {
                        start: context_start,
                        span: 1,
                        shape: ReanchoredRangeShape::CollapsedBoundary,
                    })
                } else {
                    None
                }
            })
            .collect();
        return unique_context_candidate(candidates);
    }

    None
}

fn find_reduced_selection_match_in_range(
    lines: &[&str],
    needle: &[&str],
    start: usize,
    end: usize,
    hint: usize,
) -> Option<(usize, usize)> {
    if needle.len() < 2 || start >= end || end > lines.len() {
        return None;
    }

    let mut best: Option<LineMatchCandidate> = None;
    for candidate_start in start..end {
        let Some(candidate) =
            reduced_line_match_candidate(lines, needle, candidate_start, end, hint)
        else {
            continue;
        };
        if best
            .as_ref()
            .is_none_or(|existing| candidate.is_preferred_to(existing))
        {
            best = Some(candidate);
        }
    }

    best.map(|candidate| (candidate.start, candidate.span))
}

fn find_all_sequences(lines: &[&str], needle: &[&str]) -> Vec<usize> {
    if needle.is_empty() || needle.len() > lines.len() {
        return Vec::new();
    }

    let last_start = lines.len() - needle.len();
    (0..=last_start)
        .filter(|index| matches_sequence(lines, *index, needle))
        .collect()
}

fn unique_context_candidate(
    candidates: Vec<ReanchoredRangeDecision>,
) -> Option<ReanchoredRangeDecision> {
    match candidates.as_slice() {
        [candidate] => Some(*candidate),
        [] | [_, ..] => None,
    }
}

fn context_lines(context: &str) -> Vec<&str> {
    if context.is_empty() {
        Vec::new()
    } else {
        context.lines().collect()
    }
}

fn anchor_segment_from_range(
    segment: &review_types::CommentAnchorSegment,
    lines: &[&str],
    start: usize,
    span: usize,
    file_blob_sha: String,
    status: review_types::AnchorStatus,
) -> NewCommentAnchorSegment {
    let safe_start = start.min(lines.len());
    let end = safe_start.saturating_add(span).min(lines.len());
    let (context_before, context_after) = context_around(lines, safe_start, end);

    segment_from_parts(
        segment.side,
        &segment.file_path,
        file_blob_sha,
        usize_to_i64_saturating(safe_start + 1),
        usize_to_i64_saturating(end.max(safe_start + 1)),
        segment.char_start,
        segment.char_end,
        lines[safe_start..end].join("\n"),
        context_before,
        context_after,
        status,
    )
}

fn usize_to_i64_saturating(value: usize) -> i64 {
    match i64::try_from(value) {
        Ok(value) => value,
        Err(_) => i64::MAX,
    }
}

fn context_around(lines: &[&str], start: usize, end: usize) -> (String, String) {
    let before_start = start.saturating_sub(3);
    let after_end = end.saturating_add(3).min(lines.len());
    (
        lines[before_start..start].join("\n"),
        lines[end.min(lines.len())..after_end].join("\n"),
    )
}

fn orphaned_anchor_segment(
    segment: &review_types::CommentAnchorSegment,
    file_blob_sha: String,
) -> NewCommentAnchorSegment {
    segment_from_parts(
        segment.side,
        &segment.file_path,
        file_blob_sha,
        segment.line_start,
        segment.line_end,
        segment.char_start,
        segment.char_end,
        segment.anchor_text.clone(),
        segment.context_before.clone(),
        segment.context_after.clone(),
        review_types::AnchorStatus::Orphaned,
    )
}

#[allow(clippy::too_many_arguments)]
fn segment_from_parts(
    side: review_types::CommentAnchorSide,
    file_path: &str,
    file_blob_sha: String,
    line_start: i64,
    line_end: i64,
    char_start: Option<i64>,
    char_end: Option<i64>,
    anchor_text: String,
    context_before: String,
    context_after: String,
    status: review_types::AnchorStatus,
) -> NewCommentAnchorSegment {
    let (placement_status, match_method) = match status {
        review_types::AnchorStatus::Anchored => (
            review_types::AnchorPlacementStatus::Anchored,
            review_types::AnchorMatchMethod::ExactAtLine,
        ),
        review_types::AnchorStatus::Shifted => (
            review_types::AnchorPlacementStatus::Anchored,
            review_types::AnchorMatchMethod::ExactElsewhere,
        ),
        review_types::AnchorStatus::Approximate => (
            review_types::AnchorPlacementStatus::Anchored,
            review_types::AnchorMatchMethod::Context,
        ),
        review_types::AnchorStatus::Orphaned => (
            review_types::AnchorPlacementStatus::Orphaned,
            review_types::AnchorMatchMethod::NotFound,
        ),
    };

    NewCommentAnchorSegment {
        side,
        file_path: file_path.to_string(),
        file_blob_sha,
        line_start,
        line_end,
        char_start,
        char_end,
        anchor_text,
        context_before,
        context_after,
        placement_status,
        match_method,
    }
}

fn apply_anchor(mut comment: StoredComment, anchor: &NewCommentAnchor) -> StoredComment {
    let Some(segment) = anchor
        .segments
        .iter()
        .find(|segment| segment.side == review_types::CommentAnchorSide::Head)
        .or_else(|| anchor.segments.first())
    else {
        return comment;
    };

    comment.file_blob_sha = segment.file_blob_sha.clone();
    comment.line_start = segment.line_start;
    comment.line_end = segment.line_end;
    comment.char_start = segment.char_start;
    comment.char_end = segment.char_end;
    comment.anchor_text = segment.anchor_text.clone();
    comment.context_before = segment.context_before.clone();
    comment.context_after = segment.context_after.clone();
    comment.anchor_status = match anchor.aggregate_status {
        review_types::AnchorAggregateStatus::Anchored => match segment.match_method {
            review_types::AnchorMatchMethod::ExactAtLine => review_types::AnchorStatus::Anchored,
            review_types::AnchorMatchMethod::ExactElsewhere => review_types::AnchorStatus::Shifted,
            review_types::AnchorMatchMethod::Context => review_types::AnchorStatus::Approximate,
            review_types::AnchorMatchMethod::NotFound => review_types::AnchorStatus::Orphaned,
        },
        review_types::AnchorAggregateStatus::Partial => review_types::AnchorStatus::Approximate,
        review_types::AnchorAggregateStatus::Orphaned => review_types::AnchorStatus::Orphaned,
    };
    comment.anchor = review_types::CommentAnchor {
        segments: anchor
            .segments
            .iter()
            .map(|segment| review_types::CommentAnchorSegment {
                side: segment.side,
                file_path: segment.file_path.clone(),
                line_start: segment.line_start,
                line_end: segment.line_end,
                char_start: segment.char_start,
                char_end: segment.char_end,
                anchor_text: segment.anchor_text.clone(),
                context_before: segment.context_before.clone(),
                context_after: segment.context_after.clone(),
                placement_status: segment.placement_status,
                match_method: segment.match_method,
            })
            .collect(),
        aggregate_status: anchor.aggregate_status,
    };
    comment
}

fn comment_has_file_path(comment: &StoredComment, file_path: &str) -> bool {
    comment.file_path == file_path
        || comment
            .anchor
            .segments
            .iter()
            .any(|segment| segment.file_path == file_path)
}

fn comment_response(id: &serde_json::Value, stored: StoredComment) -> JsonRpcResponse {
    json_response(
        id,
        review_types::CommentResult {
            comment: comment_from_stored(stored),
        },
    )
}

fn comment_from_stored(stored: StoredComment) -> review_types::Comment {
    review_types::Comment::from_stored(&stored)
}

async fn load_comment_in_scope(
    ctx: &ConnectionContext,
    db: &Arc<Mutex<Database>>,
    comment_id: i64,
) -> Option<StoredComment> {
    let stored = {
        let db_guard = db.lock().await;
        match db_guard.get_comment(comment_id) {
            Ok(comment) => comment,
            Err(e) => {
                eprintln!("Failed to load comment {comment_id}: {e:#}");
                return None;
            }
        }
    }?;

    if (stored.merge_base == ctx.merge_base_key() && stored.head_ref == ctx.head_scope_key())
        || !stored.resolved
    {
        return Some(stored);
    }

    let has_current_scope_resolution = {
        let db_guard = db.lock().await;
        match db_guard.comment_has_resolution_in_scope(
            comment_id,
            ctx.merge_base_key(),
            &ctx.head_scope_key(),
        ) {
            Ok(has_resolution) => has_resolution,
            Err(e) => {
                eprintln!("Failed to check comment {comment_id} resolution scope: {e:#}");
                false
            }
        }
    };

    has_current_scope_resolution.then_some(stored)
}

async fn update_comment_resolved(
    id: &serde_json::Value,
    ctx: &ConnectionContext,
    db: &Arc<Mutex<Database>>,
    notify_tx: &broadcast::Sender<Notification>,
    comment_id: i64,
    resolved: bool,
) -> JsonRpcResponse {
    let Some(stored) = load_comment_in_scope(ctx, db, comment_id).await else {
        return JsonRpcResponse::error(
            id.clone(),
            ERR_INVALID_PARAMS,
            format!("Comment {comment_id} was not found in the current review scope"),
        );
    };

    let resolution_event = if resolved && !stored.resolved {
        let stored = match reanchor_comment(ctx, db, stored).await {
            Ok(comment) => comment,
            Err(e) => {
                return JsonRpcResponse::error(
                    id.clone(),
                    ERR_INTERNAL,
                    format!("Failed to re-anchor comment before resolving: {e:#}"),
                );
            }
        };
        Some(comment_resolution_event(ctx, &stored))
    } else {
        None
    };

    {
        let db_guard = db.lock().await;
        let result = if resolved {
            match &resolution_event {
                Some(event) => db_guard.resolve_comment(event),
                None => Ok(true),
            }
        } else {
            db_guard.unresolve_comment(comment_id)
        };
        match result {
            Ok(true) => {}
            Ok(false) => {
                return JsonRpcResponse::error(
                    id.clone(),
                    ERR_INVALID_PARAMS,
                    format!("Comment {comment_id} was not found"),
                );
            }
            Err(e) => {
                return JsonRpcResponse::error(
                    id.clone(),
                    ERR_INTERNAL,
                    format!("Failed to update comment: {e:#}"),
                );
            }
        }
    }

    notify_comment_changed(ctx, notify_tx, comment_id);
    let stored = {
        let db_guard = db.lock().await;
        match db_guard.get_comment(comment_id) {
            Ok(Some(stored)) => stored,
            Ok(None) => {
                return JsonRpcResponse::error(
                    id.clone(),
                    ERR_INTERNAL,
                    format!("Comment {comment_id} disappeared after update"),
                );
            }
            Err(e) => {
                return JsonRpcResponse::error(
                    id.clone(),
                    ERR_INTERNAL,
                    format!("Failed to load comment after update: {e:#}"),
                );
            }
        }
    };

    comment_response(id, stored)
}

fn comment_resolution_event(
    ctx: &ConnectionContext,
    stored: &StoredComment,
) -> NewCommentResolutionEvent {
    NewCommentResolutionEvent {
        comment_id: stored.id,
        resolved_commit: ctx.head.resolved_commit().to_string(),
        resolved_head_ref: ctx.head_scope_key(),
        resolved_merge_base: ctx.merge_base.to_string(),
        anchor: stored.anchor.clone(),
        file_path: stored.file_path.clone(),
        line_start: stored.line_start,
        line_end: stored.line_end,
        char_start: stored.char_start,
        char_end: stored.char_end,
        anchor_text: stored.anchor_text.clone(),
        context_before: stored.context_before.clone(),
        context_after: stored.context_after.clone(),
        anchor_status: stored.anchor_status,
    }
}

fn notify_comment_changed(
    ctx: &ConnectionContext,
    notify_tx: &broadcast::Sender<Notification>,
    comment_id: i64,
) {
    let _ = notify_tx.send(Notification {
        base_ref: ctx.merge_base.to_string(),
        head_ref: ctx.head_scope_key(),
        kind: NotificationKind::CommentChanged { comment_id },
    });
}

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
    let search_params: review_types::SearchCodebaseParams =
        match serde_json::from_value(params.clone()) {
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
    let merge_base = ctx.merge_base.to_string();
    let root = matches!(ctx.review_base, git::ReviewBase::Root { .. });

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
        let base = if root {
            git::DiffBase::EmptyTree
        } else {
            git::DiffBase::Commit(&merge_base)
        };
        match repo.list_changed_files_workdir_for_base(base) {
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
        crate::search::search_codebase(&repo_root, &pattern, diff_files.as_deref())
    })
    .await;

    match result {
        Ok(Ok(search_result)) => match serde_json::to_value(search_result) {
            Ok(v) => JsonRpcResponse::success(id_owned, v),
            Err(e) => {
                JsonRpcResponse::error(id_owned, ERR_INTERNAL, format!("Serialization error: {e}"))
            }
        },
        Ok(Err(e)) => {
            JsonRpcResponse::error(id_owned, ERR_INTERNAL, format!("Search failed: {e:#}"))
        }
        Err(e) => {
            JsonRpcResponse::error(id_owned, ERR_INTERNAL, format!("Search task panicked: {e}"))
        }
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
    let def_params: review_types::FindDefinitionParams =
        match serde_json::from_value(params.clone()) {
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
            Err(e) => {
                JsonRpcResponse::error(id_owned, ERR_INTERNAL, format!("Serialization error: {e}"))
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn init_test_repo(path: &std::path::Path) -> git2::Repository {
        let repo = git2::Repository::init(path).expect("init repo");
        let mut config = repo.config().expect("repo config");
        config
            .set_str("user.email", "test@test.com")
            .expect("set email");
        config.set_str("user.name", "Test").expect("set name");
        drop(config);
        repo
    }

    fn commit_all(repo: &git2::Repository, message: &str) -> git2::Oid {
        let mut index = repo.index().expect("repo index");
        index
            .add_all(["."], git2::IndexAddOption::DEFAULT, None)
            .expect("add all");
        index.write().expect("write index");
        let tree_id = index.write_tree().expect("write tree");
        let tree = repo.find_tree(tree_id).expect("find tree");
        let signature = git2::Signature::now("Test", "test@test.com").expect("signature");
        let parent = repo
            .head()
            .ok()
            .and_then(|head| head.target())
            .and_then(|oid| repo.find_commit(oid).ok());
        let parents = parent.iter().collect::<Vec<_>>();

        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            parents.as_slice(),
        )
        .expect("commit")
    }

    fn setup_merge_base_repo() -> (tempfile::TempDir, String, String) {
        let dir = tempfile::tempdir().expect("temp repo");
        let repo = init_test_repo(dir.path());
        std::fs::write(dir.path().join("file.txt"), "base\n").expect("write base file");
        let base = commit_all(&repo, "base").to_string();
        std::fs::write(dir.path().join("file.txt"), "head\n").expect("write head file");
        let head = commit_all(&repo, "head").to_string();
        (dir, base, head)
    }

    fn stored_comment(line_start: i64, anchor_text: &str) -> StoredComment {
        StoredComment {
            id: 1,
            merge_base: "base".to_string(),
            head_ref: "head".to_string(),
            created_head_commit: "head-commit".to_string(),
            anchor: review_types::CommentAnchor {
                segments: vec![review_types::CommentAnchorSegment {
                    side: review_types::CommentAnchorSide::Head,
                    file_path: "src/lib.rs".to_string(),
                    line_start,
                    line_end: line_start,
                    char_start: None,
                    char_end: None,
                    anchor_text: anchor_text.to_string(),
                    context_before: "before".to_string(),
                    context_after: "after".to_string(),
                    placement_status: review_types::AnchorPlacementStatus::Anchored,
                    match_method: review_types::AnchorMatchMethod::ExactAtLine,
                }],
                aggregate_status: review_types::AnchorAggregateStatus::Anchored,
            },
            file_path: "src/lib.rs".to_string(),
            line_start,
            line_end: line_start,
            char_start: None,
            char_end: None,
            anchor_text: anchor_text.to_string(),
            context_before: "before".to_string(),
            context_after: "after".to_string(),
            body: "comment".to_string(),
            resolved: false,
            created_at: "2026-06-27T12:00:00+10:00".to_string(),
            updated_at: "2026-06-27T12:00:00+10:00".to_string(),
            file_blob_sha: "old".to_string(),
            anchor_status: review_types::AnchorStatus::Anchored,
        }
    }

    fn base_only_comment(line_start: i64, anchor_text: &str) -> StoredComment {
        let mut comment = stored_comment(line_start, anchor_text);
        comment.anchor = review_types::CommentAnchor {
            segments: vec![review_types::CommentAnchorSegment {
                side: review_types::CommentAnchorSide::Base,
                file_path: "src/lib.rs".to_string(),
                line_start,
                line_end: line_start,
                char_start: None,
                char_end: None,
                anchor_text: anchor_text.to_string(),
                context_before: "before".to_string(),
                context_after: "after".to_string(),
                placement_status: review_types::AnchorPlacementStatus::Anchored,
                match_method: review_types::AnchorMatchMethod::ExactAtLine,
            }],
            aggregate_status: review_types::AnchorAggregateStatus::Anchored,
        };
        comment.anchor_text = anchor_text.to_string();
        comment.context_before = "before".to_string();
        comment.context_after = "after".to_string();
        comment
    }

    fn compound_comment_with_segments(
        segments: Vec<review_types::CommentAnchorSegment>,
    ) -> StoredComment {
        let mut comment = stored_comment(1, "compound");
        comment.anchor = review_types::CommentAnchor {
            segments,
            aggregate_status: review_types::AnchorAggregateStatus::Anchored,
        };
        comment
    }

    fn anchor_segment(
        side: review_types::CommentAnchorSide,
        line_start: i64,
        anchor_text: &str,
        context_before: &str,
        context_after: &str,
    ) -> review_types::CommentAnchorSegment {
        review_types::CommentAnchorSegment {
            side,
            file_path: "src/lib.rs".to_string(),
            line_start,
            line_end: line_start,
            char_start: None,
            char_end: None,
            anchor_text: anchor_text.to_string(),
            context_before: context_before.to_string(),
            context_after: context_after.to_string(),
            placement_status: review_types::AnchorPlacementStatus::Anchored,
            match_method: review_types::AnchorMatchMethod::ExactAtLine,
        }
    }

    fn multiline_anchor_segment(
        line_start: i64,
        line_end: i64,
        anchor_text: &str,
        context_before: &str,
        context_after: &str,
    ) -> review_types::CommentAnchorSegment {
        review_types::CommentAnchorSegment {
            side: review_types::CommentAnchorSide::Head,
            file_path: "src/lib.rs".to_string(),
            line_start,
            line_end,
            char_start: None,
            char_end: None,
            anchor_text: anchor_text.to_string(),
            context_before: context_before.to_string(),
            context_after: context_after.to_string(),
            placement_status: review_types::AnchorPlacementStatus::Anchored,
            match_method: review_types::AnchorMatchMethod::ExactAtLine,
        }
    }

    fn head_segment(anchor: &NewCommentAnchor) -> &NewCommentAnchorSegment {
        anchor
            .segments
            .iter()
            .find(|segment| segment.side == review_types::CommentAnchorSide::Head)
            .unwrap()
    }

    fn segment_for_side(
        anchor: &NewCommentAnchor,
        side: review_types::CommentAnchorSide,
    ) -> &NewCommentAnchorSegment {
        anchor
            .segments
            .iter()
            .find(|segment| segment.side == side)
            .unwrap()
    }

    fn anchor_status(anchor: &NewCommentAnchor) -> review_types::AnchorStatus {
        match anchor.aggregate_status {
            review_types::AnchorAggregateStatus::Anchored => {
                match head_segment(anchor).match_method {
                    review_types::AnchorMatchMethod::ExactAtLine => {
                        review_types::AnchorStatus::Anchored
                    }
                    review_types::AnchorMatchMethod::ExactElsewhere => {
                        review_types::AnchorStatus::Shifted
                    }
                    review_types::AnchorMatchMethod::Context => {
                        review_types::AnchorStatus::Approximate
                    }
                    review_types::AnchorMatchMethod::NotFound => {
                        review_types::AnchorStatus::Orphaned
                    }
                }
            }
            review_types::AnchorAggregateStatus::Partial => review_types::AnchorStatus::Approximate,
            review_types::AnchorAggregateStatus::Orphaned => review_types::AnchorStatus::Orphaned,
        }
    }

    fn resolve_anchor_with_side_content(
        comment: &StoredComment,
        base_content: Option<&str>,
        head_content: Option<&str>,
    ) -> NewCommentAnchor {
        let segments: Vec<NewCommentAnchorSegment> = comment
            .anchor
            .segments
            .iter()
            .map(|segment| match segment.side {
                review_types::CommentAnchorSide::Base => base_content
                    .map(|content| {
                        resolve_anchor_segment(segment, content, "base-blob".to_string(), None)
                    })
                    .unwrap_or_else(|| orphaned_anchor_segment(segment, String::new())),
                review_types::CommentAnchorSide::Head => head_content
                    .map(|content| {
                        resolve_anchor_segment(segment, content, "head-blob".to_string(), None)
                    })
                    .unwrap_or_else(|| orphaned_anchor_segment(segment, String::new())),
            })
            .collect();

        NewCommentAnchor {
            aggregate_status: aggregate_status_for_new_segments(&segments),
            segments,
        }
    }

    fn resolve_head_segment_with_shape(
        segment: &review_types::CommentAnchorSegment,
        content: &str,
    ) -> ResolvedAnchorSegment {
        resolve_anchor_segment_with_shape(segment, content, "new".to_string(), None)
    }

    #[test]
    fn deleted_multiline_selection_collapses_to_after_boundary() {
        let segment =
            multiline_anchor_segment(2, 4, "delete a\ndelete b\ndelete c", "before", "after");

        let resolved = resolve_head_segment_with_shape(&segment, "before\nafter\n");

        assert_eq!(
            resolved.range_shape,
            Some(ReanchoredRangeShape::CollapsedBoundary)
        );
        assert_eq!(resolved.segment.line_start, 2);
        assert_eq!(resolved.segment.line_end, 2);
        assert_eq!(resolved.segment.anchor_text, "after");
        assert_eq!(
            resolved.segment.match_method,
            review_types::AnchorMatchMethod::Context
        );
    }

    #[test]
    fn deleted_multiline_selection_at_end_collapses_to_before_boundary() {
        let segment = multiline_anchor_segment(2, 3, "delete a\ndelete b", "before", "");

        let resolved = resolve_head_segment_with_shape(&segment, "before\n");

        assert_eq!(
            resolved.range_shape,
            Some(ReanchoredRangeShape::CollapsedBoundary)
        );
        assert_eq!(resolved.segment.line_start, 1);
        assert_eq!(resolved.segment.line_end, 1);
        assert_eq!(resolved.segment.anchor_text, "before");
    }

    #[test]
    fn deleted_multiline_selection_at_start_collapses_to_after_boundary() {
        let segment = multiline_anchor_segment(1, 2, "delete a\ndelete b", "", "after");

        let resolved = resolve_head_segment_with_shape(&segment, "after\n");

        assert_eq!(
            resolved.range_shape,
            Some(ReanchoredRangeShape::CollapsedBoundary)
        );
        assert_eq!(resolved.segment.line_start, 1);
        assert_eq!(resolved.segment.line_end, 1);
        assert_eq!(resolved.segment.anchor_text, "after");
    }

    #[test]
    fn replaced_multiline_selection_uses_current_interior_range() {
        let segment = multiline_anchor_segment(2, 4, "old a\nold b\nold c", "before", "after");

        let resolved = resolve_head_segment_with_shape(&segment, "before\nreplacement\nafter\n");

        assert_eq!(
            resolved.range_shape,
            Some(ReanchoredRangeShape::ContextInterior)
        );
        assert_eq!(resolved.segment.line_start, 2);
        assert_eq!(resolved.segment.line_end, 2);
        assert_eq!(resolved.segment.anchor_text, "replacement");
    }

    #[test]
    fn replaced_multiline_selection_can_shrink_or_grow_to_current_interior() {
        let segment =
            multiline_anchor_segment(2, 6, "old a\nold b\nold c\nold d\nold e", "before", "after");

        let shrunk = resolve_head_segment_with_shape(&segment, "before\nnew a\nnew b\nafter\n");

        assert_eq!(
            shrunk.range_shape,
            Some(ReanchoredRangeShape::ContextInterior)
        );
        assert_eq!(shrunk.segment.line_start, 2);
        assert_eq!(shrunk.segment.line_end, 3);
        assert_eq!(shrunk.segment.anchor_text, "new a\nnew b");

        let segment = multiline_anchor_segment(2, 3, "old a\nold b", "before", "after");

        let grown = resolve_head_segment_with_shape(
            &segment,
            "before\nnew a\nnew b\nnew c\nnew d\nnew e\nafter\n",
        );

        assert_eq!(
            grown.range_shape,
            Some(ReanchoredRangeShape::ContextInterior)
        );
        assert_eq!(grown.segment.line_start, 2);
        assert_eq!(grown.segment.line_end, 6);
        assert_eq!(
            grown.segment.anchor_text,
            "new a\nnew b\nnew c\nnew d\nnew e"
        );
    }

    #[test]
    fn partially_deleted_selection_reduces_to_surviving_span() {
        let segment = multiline_anchor_segment(2, 4, "keep a\ndelete b\nkeep c", "before", "after");

        let resolved = resolve_head_segment_with_shape(&segment, "before\nkeep a\nkeep c\nafter\n");

        assert_eq!(
            resolved.range_shape,
            Some(ReanchoredRangeShape::ReducedSelection)
        );
        assert_eq!(resolved.segment.line_start, 2);
        assert_eq!(resolved.segment.line_end, 3);
        assert_eq!(resolved.segment.anchor_text, "keep a\nkeep c");
    }

    #[test]
    fn moved_exact_selected_text_wins_over_context_collapse() {
        let segment = multiline_anchor_segment(2, 3, "target a\ntarget b", "before", "after");

        let resolved = resolve_head_segment_with_shape(
            &segment,
            "before\nafter\nintro\ntarget a\ntarget b\noutro\n",
        );

        assert_eq!(
            resolved.range_shape,
            Some(ReanchoredRangeShape::ExactSelection)
        );
        assert_eq!(
            resolved.segment.match_method,
            review_types::AnchorMatchMethod::ExactElsewhere
        );
        assert_eq!(resolved.segment.line_start, 4);
        assert_eq!(resolved.segment.line_end, 5);
    }

    #[test]
    fn duplicate_context_deleted_selection_orphans() {
        let segment = multiline_anchor_segment(2, 3, "delete a\ndelete b", "before", "after");

        let resolved = resolve_head_segment_with_shape(&segment, "before\nafter\nbefore\nafter\n");

        assert_eq!(resolved.range_shape, None);
        assert_eq!(
            resolved.segment.placement_status,
            review_types::AnchorPlacementStatus::Orphaned
        );
        assert_eq!(resolved.segment.anchor_text, "delete a\ndelete b");
    }

    #[test]
    fn compound_segments_reduce_independently() {
        let mut base = multiline_anchor_segment(2, 3, "old base a\nold base b", "base before", "");
        base.side = review_types::CommentAnchorSide::Base;
        let head = multiline_anchor_segment(
            2,
            4,
            "keep head\nold head\nend head",
            "head before",
            "head after",
        );
        let comment = compound_comment_with_segments(vec![base, head]);

        let anchor = resolve_anchor_with_side_content(
            &comment,
            Some("base before\n"),
            Some("head before\nkeep head\nend head\nhead after\n"),
        );

        assert_eq!(
            anchor.aggregate_status,
            review_types::AnchorAggregateStatus::Anchored
        );
        let base = segment_for_side(&anchor, review_types::CommentAnchorSide::Base);
        let head = segment_for_side(&anchor, review_types::CommentAnchorSide::Head);
        assert_eq!(base.line_start, 1);
        assert_eq!(base.line_end, 1);
        assert_eq!(base.anchor_text, "base before");
        assert_eq!(head.line_start, 2);
        assert_eq!(head.line_end, 3);
        assert_eq!(head.anchor_text, "keep head\nend head");
    }

    #[test]
    fn range_reduction_does_not_change_comment_lifecycle_state() {
        let mut comment = stored_comment(2, "delete a\ndelete b");
        comment.line_end = 3;
        comment.anchor = review_types::CommentAnchor {
            segments: vec![multiline_anchor_segment(
                2,
                3,
                "delete a\ndelete b",
                "before",
                "after",
            )],
            aggregate_status: review_types::AnchorAggregateStatus::Anchored,
        };
        comment.resolved = true;
        let anchor = resolve_anchor(&comment, "before\nafter\n", "new".to_string(), None);

        let resolved_comment = apply_anchor(comment.clone(), &anchor);

        assert!(resolved_comment.resolved);
        assert_eq!(resolved_comment.line_start, 2);
        assert_eq!(resolved_comment.line_end, 2);

        comment.resolved = false;
        let unresolved_comment = apply_anchor(comment, &anchor);

        assert!(!unresolved_comment.resolved);
        assert_eq!(unresolved_comment.line_start, 2);
        assert_eq!(unresolved_comment.line_end, 2);
    }

    fn test_server_context() -> ConnectionContext {
        ConnectionContext {
            repo_root: PathBuf::from("/repo"),
            worktree: PathBuf::from("/repo"),
            base_ref: "main".to_string(),
            review_base: git::ReviewBase::Named {
                input: "main".to_string(),
                resolved_commit: git::CommitId::new("base-commit"),
                kind: git::NamedRefKind::Branch,
            },
            merge_base: git::CommitId::new("merge-base-commit"),
            head: git::HeadIdentity::Branch {
                name: "feature".to_string(),
                resolved_commit: git::CommitId::new("head-commit"),
            },
            db_path: PathBuf::from("/repo/.crt/reviews.db"),
            diff_algorithm: crate::config::DiffAlgorithm::Patience,
        }
    }

    fn test_context_for_repo(
        repo_path: &std::path::Path,
        merge_base: String,
        head: String,
    ) -> ConnectionContext {
        ConnectionContext {
            repo_root: repo_path.to_path_buf(),
            worktree: repo_path.to_path_buf(),
            base_ref: merge_base.clone(),
            review_base: git::ReviewBase::Anonymous {
                input: merge_base.clone(),
                resolved_commit: git::CommitId::new(merge_base.clone()),
            },
            merge_base: git::CommitId::new(merge_base),
            head: git::HeadIdentity::Detached {
                commit: git::CommitId::new(head),
            },
            db_path: repo_path.join(".crt/reviews.db"),
            diff_algorithm: crate::config::DiffAlgorithm::Patience,
        }
    }

    #[tokio::test]
    async fn set_merge_base_resolves_ref_to_full_hash_and_updates_session() {
        let (dir, base, head) = setup_merge_base_repo();
        let repo = git::Repo::open(dir.path()).expect("open repo");
        let repo_context = repo.context().expect("repo context");
        let state = Arc::new(ServerState::new());
        let mut ctx = ConnectionContext {
            repo_root: repo_context.repo_root.clone(),
            worktree: repo_context.worktree.clone(),
            base_ref: "base".to_string(),
            review_base: git::ReviewBase::Anonymous {
                input: "base".to_string(),
                resolved_commit: git::CommitId::new(base),
            },
            merge_base: git::CommitId::new(repo.resolve_commit("HEAD~1").expect("base commit")),
            head: repo_context.head,
            db_path: repo_context.repo_root.join(".crt/reviews.db"),
            diff_algorithm: crate::config::DiffAlgorithm::Patience,
        };
        state.register_session(&ctx).await;

        let short_head = &head[..7];
        let response = handle_set_merge_base(
            &serde_json::to_value(review_types::SetMergeBaseParams {
                refspec: short_head.to_string(),
            })
            .expect("serialize params"),
            &serde_json::json!(1),
            &state,
            &mut ctx,
        )
        .await;

        assert!(response.error.is_none(), "{:?}", response.error);
        let result: review_types::SetMergeBaseResult =
            serde_json::from_value(response.result.expect("result")).expect("deserialize result");
        assert_eq!(ctx.merge_base.to_string(), head);
        assert_eq!(ctx.base_ref, short_head);
        assert_eq!(result.context.merge_base, head);
        assert_eq!(result.context.base_ref, short_head);
        match &ctx.review_base {
            git::ReviewBase::Anonymous {
                input,
                resolved_commit,
            } => {
                assert_eq!(input, short_head);
                assert_eq!(resolved_commit.as_ref(), head);
            }
            other => panic!("expected anonymous review base, got {other:?}"),
        }

        let sessions = state.list_sessions().await;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].merge_base, head);
        assert_eq!(sessions[0].client_count, 1);
    }

    #[test]
    fn current_session_reanchor_range_uses_merge_base_and_worktree_head() {
        let ctx = test_server_context();

        let range = CommentReanchorRange::current_session(&ctx);

        assert_eq!(
            range.endpoint_for_side(review_types::CommentAnchorSide::Base),
            &ReanchorEndpoint::Commit {
                refspec: "merge-base-commit".to_string()
            }
        );
        assert_eq!(
            range.endpoint_for_side(review_types::CommentAnchorSide::Head),
            &ReanchorEndpoint::Worktree {
                compare_to: Some("head-commit".to_string())
            }
        );
    }

    #[test]
    fn explicit_root_base_range_orphans_base_segments_only() {
        let comment = compound_comment_with_segments(vec![
            anchor_segment(
                review_types::CommentAnchorSide::Base,
                2,
                "base target",
                "base before",
                "base after",
            ),
            anchor_segment(
                review_types::CommentAnchorSide::Head,
                2,
                "head target",
                "head before",
                "head after",
            ),
        ]);

        let anchor = resolve_anchor_with_side_content(
            &comment,
            None,
            Some("head before\nhead target\nhead after\n"),
        );

        let base = segment_for_side(&anchor, review_types::CommentAnchorSide::Base);
        let head = segment_for_side(&anchor, review_types::CommentAnchorSide::Head);
        assert_eq!(
            anchor.aggregate_status,
            review_types::AnchorAggregateStatus::Partial
        );
        assert_eq!(
            base.placement_status,
            review_types::AnchorPlacementStatus::Orphaned
        );
        assert_eq!(
            head.placement_status,
            review_types::AnchorPlacementStatus::Anchored
        );
    }

    #[tokio::test]
    async fn head_reanchor_follows_committed_file_rename() {
        let dir = tempfile::tempdir().expect("temp repo");
        let repo = init_test_repo(dir.path());
        std::fs::write(dir.path().join("old.rs"), "before\ntarget\nafter\n")
            .expect("write old file");
        let created_head = commit_all(&repo, "initial").to_string();
        std::fs::rename(dir.path().join("old.rs"), dir.path().join("new.rs")).expect("rename file");
        let head = commit_all(&repo, "rename").to_string();
        let ctx = test_context_for_repo(dir.path(), created_head.clone(), head);
        let mut comment = stored_comment(2, "target");
        comment.file_path = "old.rs".to_string();
        comment.created_head_commit = created_head;
        comment.anchor = review_types::CommentAnchor {
            segments: vec![anchor_segment(
                review_types::CommentAnchorSide::Head,
                2,
                "target",
                "before",
                "after",
            )],
            aggregate_status: review_types::AnchorAggregateStatus::Anchored,
        };
        comment.anchor.segments[0].file_path = "old.rs".to_string();
        let range = CommentReanchorRange::current_session(&ctx);

        let anchor = resolve_anchor_for_range(&ctx, &range, &comment)
            .await
            .expect("resolve anchor");

        let head = segment_for_side(&anchor, review_types::CommentAnchorSide::Head);
        assert_eq!(
            anchor.aggregate_status,
            review_types::AnchorAggregateStatus::Anchored
        );
        assert_eq!(head.file_path, "new.rs");
        assert_eq!(head.line_start, 2);
        assert_eq!(
            head.match_method,
            review_types::AnchorMatchMethod::ExactAtLine
        );
    }

    #[tokio::test]
    async fn base_reanchor_follows_merge_base_file_rename() {
        let dir = tempfile::tempdir().expect("temp repo");
        let repo = init_test_repo(dir.path());
        std::fs::write(dir.path().join("old.rs"), "before\ntarget\nafter\n")
            .expect("write old file");
        let old_base = commit_all(&repo, "initial").to_string();
        std::fs::rename(dir.path().join("old.rs"), dir.path().join("new.rs")).expect("rename file");
        let new_base = commit_all(&repo, "rename").to_string();
        let ctx = test_context_for_repo(dir.path(), new_base.clone(), new_base);
        let mut comment = base_only_comment(2, "target");
        comment.file_path = "old.rs".to_string();
        comment.merge_base = old_base;
        comment.anchor.segments[0].file_path = "old.rs".to_string();
        let range = CommentReanchorRange::current_session(&ctx);

        let anchor = resolve_anchor_for_range(&ctx, &range, &comment)
            .await
            .expect("resolve anchor");

        let base = segment_for_side(&anchor, review_types::CommentAnchorSide::Base);
        assert_eq!(
            anchor.aggregate_status,
            review_types::AnchorAggregateStatus::Anchored
        );
        assert_eq!(base.file_path, "new.rs");
        assert_eq!(base.line_start, 2);
        assert_eq!(
            base.match_method,
            review_types::AnchorMatchMethod::ExactAtLine
        );
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum AnchorShape {
        BaseOnly,
        HeadOnly,
        PairedSameRow,
        PairedBaseBeforeHead,
        PairedHeadBeforeBase,
        PairedDifferentLinesSameDisplay,
    }

    impl AnchorShape {
        fn all() -> [Self; 6] {
            [
                Self::BaseOnly,
                Self::HeadOnly,
                Self::PairedSameRow,
                Self::PairedBaseBeforeHead,
                Self::PairedHeadBeforeBase,
                Self::PairedDifferentLinesSameDisplay,
            ]
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum HistoryTopology {
        SameBranch,
        Rebase,
        Merge,
        Squash,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum SideMutation {
        SameLine,
        Moved,
        StartContextChanged,
        EndContextChanged,
        BothContextChanged,
        SelectedTextChangedWithContext,
        SelectedTextRemovedWithContext,
        SelectedTextRemovedNoContext,
        StartContextRemoved,
        EndContextRemoved,
        BothContextRemoved,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ExpectedMatch {
        ExactAtLine,
        ExactElsewhere,
        ExactAny,
        Context,
        NotFound,
    }

    #[derive(Debug, Clone, Copy)]
    struct HistoryMatrixCase {
        id: &'static str,
        base: SideMutation,
        head: SideMutation,
        expected_base: ExpectedMatch,
        expected_head: ExpectedMatch,
    }

    fn same_branch_history_cases() -> Vec<HistoryMatrixCase> {
        use ExpectedMatch::{Context, ExactAtLine, NotFound};
        use SideMutation::{
            BothContextChanged, EndContextChanged, SameLine, SelectedTextChangedWithContext,
            SelectedTextRemovedNoContext, StartContextChanged,
        };
        vec![
            HistoryMatrixCase {
                id: "1.1 same branch code unchanged",
                base: SameLine,
                head: SameLine,
                expected_base: ExactAtLine,
                expected_head: ExactAtLine,
            },
            HistoryMatrixCase {
                id: "1.2 same branch start context changes",
                base: StartContextChanged,
                head: StartContextChanged,
                expected_base: ExactAtLine,
                expected_head: ExactAtLine,
            },
            HistoryMatrixCase {
                id: "1.3 same branch end context changes",
                base: EndContextChanged,
                head: EndContextChanged,
                expected_base: ExactAtLine,
                expected_head: ExactAtLine,
            },
            HistoryMatrixCase {
                id: "1.4 same branch both context sides change",
                base: BothContextChanged,
                head: BothContextChanged,
                expected_base: ExactAtLine,
                expected_head: ExactAtLine,
            },
            HistoryMatrixCase {
                id: "1.5a same branch selected base text changes with context",
                base: SelectedTextChangedWithContext,
                head: SameLine,
                expected_base: Context,
                expected_head: ExactAtLine,
            },
            HistoryMatrixCase {
                id: "1.5b same branch selected base text removed without context",
                base: SelectedTextRemovedNoContext,
                head: SameLine,
                expected_base: NotFound,
                expected_head: ExactAtLine,
            },
            HistoryMatrixCase {
                id: "1.6a same branch selected head text changes with context",
                base: SameLine,
                head: SelectedTextChangedWithContext,
                expected_base: ExactAtLine,
                expected_head: Context,
            },
            HistoryMatrixCase {
                id: "1.6b same branch selected head text removed without context",
                base: SameLine,
                head: SelectedTextRemovedNoContext,
                expected_base: ExactAtLine,
                expected_head: NotFound,
            },
            HistoryMatrixCase {
                id: "1.7a same branch selected base and head text change with context",
                base: SelectedTextChangedWithContext,
                head: SelectedTextChangedWithContext,
                expected_base: Context,
                expected_head: Context,
            },
            HistoryMatrixCase {
                id: "1.7b same branch base orphaned and head context anchors",
                base: SelectedTextRemovedNoContext,
                head: SelectedTextChangedWithContext,
                expected_base: NotFound,
                expected_head: Context,
            },
            HistoryMatrixCase {
                id: "1.7c same branch base context anchors and head orphaned",
                base: SelectedTextChangedWithContext,
                head: SelectedTextRemovedNoContext,
                expected_base: Context,
                expected_head: NotFound,
            },
            HistoryMatrixCase {
                id: "1.7d same branch base and head orphaned",
                base: SelectedTextRemovedNoContext,
                head: SelectedTextRemovedNoContext,
                expected_base: NotFound,
                expected_head: NotFound,
            },
        ]
    }

    fn rebase_equivalent_history_cases() -> Vec<HistoryMatrixCase> {
        use ExpectedMatch::{Context, ExactAny, ExactElsewhere, NotFound};
        use SideMutation::{
            BothContextChanged, BothContextRemoved, EndContextChanged, EndContextRemoved, Moved,
            SelectedTextChangedWithContext, SelectedTextRemovedNoContext,
            SelectedTextRemovedWithContext, StartContextChanged, StartContextRemoved,
        };
        vec![
            HistoryMatrixCase {
                id: "2.1 rebase code unchanged",
                base: Moved,
                head: Moved,
                expected_base: ExactElsewhere,
                expected_head: ExactElsewhere,
            },
            HistoryMatrixCase {
                id: "2.2 rebase base start context changes",
                base: StartContextChanged,
                head: Moved,
                expected_base: ExactAny,
                expected_head: ExactElsewhere,
            },
            HistoryMatrixCase {
                id: "2.3 rebase base end context changes",
                base: EndContextChanged,
                head: Moved,
                expected_base: ExactAny,
                expected_head: ExactElsewhere,
            },
            HistoryMatrixCase {
                id: "2.4 rebase base both context sides change",
                base: BothContextChanged,
                head: Moved,
                expected_base: ExactAny,
                expected_head: ExactElsewhere,
            },
            HistoryMatrixCase {
                id: "2.5a rebase selected base text changes with context",
                base: SelectedTextChangedWithContext,
                head: Moved,
                expected_base: Context,
                expected_head: ExactElsewhere,
            },
            HistoryMatrixCase {
                id: "2.5b rebase selected base text lacks sufficient context",
                base: SelectedTextRemovedNoContext,
                head: Moved,
                expected_base: NotFound,
                expected_head: ExactElsewhere,
            },
            HistoryMatrixCase {
                id: "2.6 rebase head start context changes",
                base: Moved,
                head: StartContextChanged,
                expected_base: ExactElsewhere,
                expected_head: ExactAny,
            },
            HistoryMatrixCase {
                id: "2.7 rebase head end context changes",
                base: Moved,
                head: EndContextChanged,
                expected_base: ExactElsewhere,
                expected_head: ExactAny,
            },
            HistoryMatrixCase {
                id: "2.8 rebase head both context sides change",
                base: Moved,
                head: BothContextChanged,
                expected_base: ExactElsewhere,
                expected_head: ExactAny,
            },
            HistoryMatrixCase {
                id: "2.9a rebase selected head text changes with context",
                base: Moved,
                head: SelectedTextChangedWithContext,
                expected_base: ExactElsewhere,
                expected_head: Context,
            },
            HistoryMatrixCase {
                id: "2.9b rebase selected head text lacks sufficient context",
                base: Moved,
                head: SelectedTextRemovedNoContext,
                expected_base: ExactElsewhere,
                expected_head: NotFound,
            },
            HistoryMatrixCase {
                id: "2.10 rebase base and head start context change",
                base: StartContextChanged,
                head: StartContextChanged,
                expected_base: ExactAny,
                expected_head: ExactAny,
            },
            HistoryMatrixCase {
                id: "2.11 rebase base and head end context change",
                base: EndContextChanged,
                head: EndContextChanged,
                expected_base: ExactAny,
                expected_head: ExactAny,
            },
            HistoryMatrixCase {
                id: "2.12 rebase base and head context both change",
                base: BothContextChanged,
                head: BothContextChanged,
                expected_base: ExactAny,
                expected_head: ExactAny,
            },
            HistoryMatrixCase {
                id: "2.13a rebase selected base and head text change with context",
                base: SelectedTextChangedWithContext,
                head: SelectedTextChangedWithContext,
                expected_base: Context,
                expected_head: Context,
            },
            HistoryMatrixCase {
                id: "2.13b rebase base orphaned and head context anchors",
                base: SelectedTextRemovedNoContext,
                head: SelectedTextChangedWithContext,
                expected_base: NotFound,
                expected_head: Context,
            },
            HistoryMatrixCase {
                id: "2.13c rebase base context anchors and head orphaned",
                base: SelectedTextChangedWithContext,
                head: SelectedTextRemovedNoContext,
                expected_base: Context,
                expected_head: NotFound,
            },
            HistoryMatrixCase {
                id: "2.13d rebase base and head orphaned",
                base: SelectedTextRemovedNoContext,
                head: SelectedTextRemovedNoContext,
                expected_base: NotFound,
                expected_head: NotFound,
            },
            HistoryMatrixCase {
                id: "2.14 rebase base start context removed",
                base: StartContextRemoved,
                head: Moved,
                expected_base: ExactAny,
                expected_head: ExactElsewhere,
            },
            HistoryMatrixCase {
                id: "2.15 rebase base end context removed",
                base: EndContextRemoved,
                head: Moved,
                expected_base: ExactAny,
                expected_head: ExactElsewhere,
            },
            HistoryMatrixCase {
                id: "2.16 rebase base context removed",
                base: BothContextRemoved,
                head: Moved,
                expected_base: ExactAny,
                expected_head: ExactElsewhere,
            },
            HistoryMatrixCase {
                id: "2.17a rebase selected base text removed with context",
                base: SelectedTextRemovedWithContext,
                head: Moved,
                expected_base: Context,
                expected_head: ExactElsewhere,
            },
            HistoryMatrixCase {
                id: "2.17b rebase selected base text removed without context",
                base: SelectedTextRemovedNoContext,
                head: Moved,
                expected_base: NotFound,
                expected_head: ExactElsewhere,
            },
            HistoryMatrixCase {
                id: "2.18 rebase head start context removed",
                base: Moved,
                head: StartContextRemoved,
                expected_base: ExactElsewhere,
                expected_head: ExactAny,
            },
            HistoryMatrixCase {
                id: "2.19 rebase head end context removed",
                base: Moved,
                head: EndContextRemoved,
                expected_base: ExactElsewhere,
                expected_head: ExactAny,
            },
            HistoryMatrixCase {
                id: "2.20 rebase head context removed",
                base: Moved,
                head: BothContextRemoved,
                expected_base: ExactElsewhere,
                expected_head: ExactAny,
            },
            HistoryMatrixCase {
                id: "2.21a rebase selected head text removed with context",
                base: Moved,
                head: SelectedTextRemovedWithContext,
                expected_base: ExactElsewhere,
                expected_head: Context,
            },
            HistoryMatrixCase {
                id: "2.21b rebase selected head text removed without context",
                base: Moved,
                head: SelectedTextRemovedNoContext,
                expected_base: ExactElsewhere,
                expected_head: NotFound,
            },
        ]
    }

    fn history_matrix_cases(topology: HistoryTopology) -> Vec<HistoryMatrixCase> {
        match topology {
            HistoryTopology::SameBranch => same_branch_history_cases(),
            HistoryTopology::Rebase | HistoryTopology::Merge | HistoryTopology::Squash => {
                rebase_equivalent_history_cases()
            }
        }
    }

    fn comment_for_shape(shape: AnchorShape) -> StoredComment {
        let segments = match shape {
            AnchorShape::BaseOnly => vec![anchor_segment(
                review_types::CommentAnchorSide::Base,
                2,
                "base target",
                "base before",
                "base after",
            )],
            AnchorShape::HeadOnly => vec![anchor_segment(
                review_types::CommentAnchorSide::Head,
                2,
                "head target",
                "head before",
                "head after",
            )],
            AnchorShape::PairedSameRow => vec![
                anchor_segment(
                    review_types::CommentAnchorSide::Base,
                    2,
                    "base target",
                    "base before",
                    "base after",
                ),
                anchor_segment(
                    review_types::CommentAnchorSide::Head,
                    2,
                    "head target",
                    "head before",
                    "head after",
                ),
            ],
            AnchorShape::PairedBaseBeforeHead => vec![
                anchor_segment(
                    review_types::CommentAnchorSide::Base,
                    2,
                    "base target",
                    "base before",
                    "base after",
                ),
                anchor_segment(
                    review_types::CommentAnchorSide::Head,
                    5,
                    "head target",
                    "head before",
                    "head after",
                ),
            ],
            AnchorShape::PairedHeadBeforeBase => vec![
                anchor_segment(
                    review_types::CommentAnchorSide::Base,
                    5,
                    "base target",
                    "base before",
                    "base after",
                ),
                anchor_segment(
                    review_types::CommentAnchorSide::Head,
                    2,
                    "head target",
                    "head before",
                    "head after",
                ),
            ],
            AnchorShape::PairedDifferentLinesSameDisplay => vec![
                anchor_segment(
                    review_types::CommentAnchorSide::Base,
                    2,
                    "base target",
                    "base before",
                    "base after",
                ),
                anchor_segment(
                    review_types::CommentAnchorSide::Head,
                    8,
                    "head target",
                    "head before",
                    "head after",
                ),
            ],
        };
        compound_comment_with_segments(segments)
    }

    fn content_for_mutation(
        segment: &review_types::CommentAnchorSegment,
        mutation: SideMutation,
    ) -> String {
        match mutation {
            SideMutation::SameLine => content_at_line(
                segment.line_start,
                &segment.context_before,
                &segment.anchor_text,
                &segment.context_after,
            ),
            SideMutation::Moved => content_at_line(
                segment.line_start + 2,
                &segment.context_before,
                &segment.anchor_text,
                &segment.context_after,
            ),
            SideMutation::StartContextChanged => content_at_line(
                segment.line_start,
                &format!("changed {}", segment.context_before),
                &segment.anchor_text,
                &segment.context_after,
            ),
            SideMutation::EndContextChanged => content_at_line(
                segment.line_start,
                &segment.context_before,
                &segment.anchor_text,
                &format!("changed {}", segment.context_after),
            ),
            SideMutation::BothContextChanged => content_at_line(
                segment.line_start,
                &format!("changed {}", segment.context_before),
                &segment.anchor_text,
                &format!("changed {}", segment.context_after),
            ),
            SideMutation::SelectedTextChangedWithContext => content_at_line(
                segment.line_start,
                &segment.context_before,
                &replacement_text(segment),
                &segment.context_after,
            ),
            SideMutation::SelectedTextRemovedWithContext => content_at_line(
                segment.line_start,
                &segment.context_before,
                &replacement_text(segment),
                &segment.context_after,
            ),
            SideMutation::SelectedTextRemovedNoContext => "unrelated\ncontent\n".to_string(),
            SideMutation::StartContextRemoved => {
                format!("{}\n{}\n", segment.anchor_text, segment.context_after)
            }
            SideMutation::EndContextRemoved => content_at_line(
                segment.line_start,
                &segment.context_before,
                &segment.anchor_text,
                "",
            ),
            SideMutation::BothContextRemoved => format!("{}\n", segment.anchor_text),
        }
    }

    fn content_at_line(line_start: i64, before: &str, anchor_text: &str, after: &str) -> String {
        let mut lines = Vec::new();
        let filler_count = line_start.saturating_sub(2);
        for idx in 0..filler_count {
            lines.push(format!("filler {}", idx + 1));
        }
        if !before.is_empty() {
            lines.push(before.to_string());
        }
        lines.push(anchor_text.to_string());
        if !after.is_empty() {
            lines.push(after.to_string());
        }
        format!("{}\n", lines.join("\n"))
    }

    fn replacement_text(segment: &review_types::CommentAnchorSegment) -> String {
        match segment.side {
            review_types::CommentAnchorSide::Base => "changed base target".to_string(),
            review_types::CommentAnchorSide::Head => "changed head target".to_string(),
        }
    }

    fn assert_history_matrix(topology: HistoryTopology) {
        let mut failures = Vec::new();
        for case in history_matrix_cases(topology) {
            for shape in AnchorShape::all() {
                collect_history_matrix_failures(topology, shape, case, &mut failures);
            }
        }

        if !failures.is_empty() {
            panic!(
                "history matrix failures for {topology:?}:\n{}",
                failures.join("\n")
            );
        }
    }

    fn collect_history_matrix_failures(
        topology: HistoryTopology,
        shape: AnchorShape,
        case: HistoryMatrixCase,
        failures: &mut Vec<String>,
    ) {
        let comment = comment_for_shape(shape);
        let base_segment = comment
            .anchor
            .segments
            .iter()
            .find(|segment| segment.side == review_types::CommentAnchorSide::Base);
        let head_segment = comment
            .anchor
            .segments
            .iter()
            .find(|segment| segment.side == review_types::CommentAnchorSide::Head);
        let base_content = base_segment.map(|segment| content_for_mutation(segment, case.base));
        let head_content = head_segment.map(|segment| content_for_mutation(segment, case.head));
        let anchor = resolve_anchor_with_side_content(
            &comment,
            base_content.as_deref(),
            head_content.as_deref(),
        );

        if let Some(segment) = base_segment {
            let actual = segment_for_side(&anchor, review_types::CommentAnchorSide::Base);
            check_segment_expectation(
                topology,
                shape,
                case,
                segment,
                actual,
                case.expected_base,
                base_content.as_deref(),
                "base-blob",
                failures,
            );
        }
        if let Some(segment) = head_segment {
            let actual = segment_for_side(&anchor, review_types::CommentAnchorSide::Head);
            check_segment_expectation(
                topology,
                shape,
                case,
                segment,
                actual,
                case.expected_head,
                head_content.as_deref(),
                "head-blob",
                failures,
            );
        }

        let expected_aggregate = expected_aggregate_status(
            base_segment.map(|_| case.expected_base),
            head_segment.map(|_| case.expected_head),
        );
        if anchor.aggregate_status != expected_aggregate {
            failures.push(format!(
                "{topology:?} {shape:?} {} aggregate: expected {:?}, got {:?}",
                case.id, expected_aggregate, anchor.aggregate_status
            ));
        }
    }

    fn check_segment_expectation(
        topology: HistoryTopology,
        shape: AnchorShape,
        case: HistoryMatrixCase,
        original: &review_types::CommentAnchorSegment,
        actual: &NewCommentAnchorSegment,
        expected: ExpectedMatch,
        expected_content: Option<&str>,
        expected_blob: &str,
        failures: &mut Vec<String>,
    ) {
        if actual.file_blob_sha != expected_blob {
            failures.push(format!(
                "{topology:?} {shape:?} {} {:?}: expected blob {:?}, got {:?}",
                case.id, original.side, expected_blob, actual.file_blob_sha
            ));
        }

        let expected_placement = match expected {
            ExpectedMatch::NotFound => review_types::AnchorPlacementStatus::Orphaned,
            ExpectedMatch::ExactAtLine
            | ExpectedMatch::ExactElsewhere
            | ExpectedMatch::ExactAny
            | ExpectedMatch::Context => review_types::AnchorPlacementStatus::Anchored,
        };
        if actual.placement_status != expected_placement {
            failures.push(format!(
                "{topology:?} {shape:?} {} {:?}: expected placement {:?}, got {:?}",
                case.id, original.side, expected_placement, actual.placement_status
            ));
        }

        let method_ok = match expected {
            ExpectedMatch::ExactAtLine => {
                actual.match_method == review_types::AnchorMatchMethod::ExactAtLine
            }
            ExpectedMatch::ExactElsewhere => {
                actual.match_method == review_types::AnchorMatchMethod::ExactElsewhere
            }
            ExpectedMatch::ExactAny => {
                actual.match_method == review_types::AnchorMatchMethod::ExactAtLine
                    || actual.match_method == review_types::AnchorMatchMethod::ExactElsewhere
            }
            ExpectedMatch::Context => {
                actual.match_method == review_types::AnchorMatchMethod::Context
            }
            ExpectedMatch::NotFound => {
                actual.match_method == review_types::AnchorMatchMethod::NotFound
            }
        };
        if !method_ok {
            failures.push(format!(
                "{topology:?} {shape:?} {} {:?}: expected method {:?}, got {:?}",
                case.id, original.side, expected, actual.match_method
            ));
        }

        match expected {
            ExpectedMatch::Context => {
                let replacement = replacement_text(original);
                if actual.anchor_text != replacement {
                    failures.push(format!(
                        "{topology:?} {shape:?} {} {:?}: expected replacement text {:?}, got {:?}",
                        case.id, original.side, replacement, actual.anchor_text
                    ));
                }
            }
            ExpectedMatch::NotFound => {
                if actual.anchor_text != original.anchor_text {
                    failures.push(format!(
                        "{topology:?} {shape:?} {} {:?}: orphaned segment should preserve text {:?}, got {:?}",
                        case.id, original.side, original.anchor_text, actual.anchor_text
                    ));
                }
            }
            ExpectedMatch::ExactAtLine
            | ExpectedMatch::ExactElsewhere
            | ExpectedMatch::ExactAny => {}
        }

        check_context_snapshot(
            topology,
            shape,
            case,
            original,
            actual,
            expected,
            expected_content,
            failures,
        );
    }

    fn check_context_snapshot(
        topology: HistoryTopology,
        shape: AnchorShape,
        case: HistoryMatrixCase,
        original: &review_types::CommentAnchorSegment,
        actual: &NewCommentAnchorSegment,
        expected: ExpectedMatch,
        expected_content: Option<&str>,
        failures: &mut Vec<String>,
    ) {
        let expected_context = match expected {
            ExpectedMatch::NotFound => (
                original.context_before.clone(),
                original.context_after.clone(),
            ),
            ExpectedMatch::ExactAtLine
            | ExpectedMatch::ExactElsewhere
            | ExpectedMatch::ExactAny
            | ExpectedMatch::Context => {
                let Some(content) = expected_content else {
                    failures.push(format!(
                        "{topology:?} {shape:?} {} {:?}: anchored segment has no source content",
                        case.id, original.side
                    ));
                    return;
                };
                context_snapshot_for_segment(content, actual)
            }
        };

        if actual.context_before != expected_context.0 {
            failures.push(format!(
                "{topology:?} {shape:?} {} {:?}: expected before context {:?}, got {:?}",
                case.id, original.side, expected_context.0, actual.context_before
            ));
        }
        if actual.context_after != expected_context.1 {
            failures.push(format!(
                "{topology:?} {shape:?} {} {:?}: expected after context {:?}, got {:?}",
                case.id, original.side, expected_context.1, actual.context_after
            ));
        }
    }

    fn context_snapshot_for_segment(
        content: &str,
        segment: &NewCommentAnchorSegment,
    ) -> (String, String) {
        let lines = content_lines(content);
        let start = segment
            .line_start
            .checked_sub(1)
            .and_then(|line| usize::try_from(line).ok())
            .unwrap_or(0);
        let end = usize::try_from(segment.line_end).unwrap_or(start + 1);
        context_around(&lines, start, end)
    }

    fn expected_aggregate_status(
        base: Option<ExpectedMatch>,
        head: Option<ExpectedMatch>,
    ) -> review_types::AnchorAggregateStatus {
        let expectations = [base, head].into_iter().flatten();
        let mut anchored = 0;
        let mut orphaned = 0;
        for expected in expectations {
            match expected {
                ExpectedMatch::NotFound => orphaned += 1,
                ExpectedMatch::ExactAtLine
                | ExpectedMatch::ExactElsewhere
                | ExpectedMatch::ExactAny
                | ExpectedMatch::Context => anchored += 1,
            }
        }
        if anchored > 0 && orphaned == 0 {
            review_types::AnchorAggregateStatus::Anchored
        } else if anchored > 0 {
            review_types::AnchorAggregateStatus::Partial
        } else {
            review_types::AnchorAggregateStatus::Orphaned
        }
    }

    #[test]
    fn same_branch_history_matrix_covers_all_anchor_shapes() {
        assert_history_matrix(HistoryTopology::SameBranch);
    }

    #[test]
    fn rebase_history_matrix_covers_all_anchor_shapes() {
        assert_history_matrix(HistoryTopology::Rebase);
    }

    #[test]
    fn merge_history_matrix_covers_all_anchor_shapes() {
        assert_history_matrix(HistoryTopology::Merge);
    }

    #[test]
    fn squash_history_matrix_covers_all_anchor_shapes() {
        assert_history_matrix(HistoryTopology::Squash);
    }

    #[test]
    fn resolves_exact_anchor_at_stored_line() {
        let comment = stored_comment(2, "target");
        let anchor = resolve_anchor(&comment, "before\ntarget\nafter\n", "new".to_string(), None);

        assert_eq!(anchor_status(&anchor), review_types::AnchorStatus::Anchored);
        assert_eq!(head_segment(&anchor).line_start, 2);
        assert_eq!(head_segment(&anchor).anchor_text, "target");
    }

    #[test]
    fn compound_reanchor_preserves_base_and_head_segments() {
        let mut comment = stored_comment(2, "head target");
        comment.anchor = review_types::CommentAnchor {
            segments: vec![
                review_types::CommentAnchorSegment {
                    side: review_types::CommentAnchorSide::Base,
                    file_path: "src/lib.rs".to_string(),
                    line_start: 4,
                    line_end: 4,
                    char_start: None,
                    char_end: None,
                    anchor_text: "base target".to_string(),
                    context_before: "base before".to_string(),
                    context_after: "base after".to_string(),
                    placement_status: review_types::AnchorPlacementStatus::Anchored,
                    match_method: review_types::AnchorMatchMethod::ExactAtLine,
                },
                review_types::CommentAnchorSegment {
                    side: review_types::CommentAnchorSide::Head,
                    file_path: "src/lib.rs".to_string(),
                    line_start: 2,
                    line_end: 2,
                    char_start: None,
                    char_end: None,
                    anchor_text: "head target".to_string(),
                    context_before: "head before".to_string(),
                    context_after: "head after".to_string(),
                    placement_status: review_types::AnchorPlacementStatus::Anchored,
                    match_method: review_types::AnchorMatchMethod::ExactAtLine,
                },
            ],
            aggregate_status: review_types::AnchorAggregateStatus::Anchored,
        };

        let anchor = resolve_anchor(
            &comment,
            "head before\nhead target\nhead after\n",
            "head-blob".to_string(),
            None,
        );

        assert_eq!(anchor.segments.len(), 2);
        assert!(
            anchor
                .segments
                .iter()
                .any(|segment| segment.side == review_types::CommentAnchorSide::Base)
        );
        assert!(
            anchor
                .segments
                .iter()
                .any(|segment| segment.side == review_types::CommentAnchorSide::Head)
        );
    }

    #[test]
    fn compound_reanchor_resolves_each_segment_independently() {
        let mut comment = stored_comment(8, "head target");
        comment.anchor = review_types::CommentAnchor {
            segments: vec![
                review_types::CommentAnchorSegment {
                    side: review_types::CommentAnchorSide::Base,
                    file_path: "src/lib.rs".to_string(),
                    line_start: 10,
                    line_end: 10,
                    char_start: None,
                    char_end: None,
                    anchor_text: "base target".to_string(),
                    context_before: "base before".to_string(),
                    context_after: "base after".to_string(),
                    placement_status: review_types::AnchorPlacementStatus::Anchored,
                    match_method: review_types::AnchorMatchMethod::ExactAtLine,
                },
                review_types::CommentAnchorSegment {
                    side: review_types::CommentAnchorSide::Head,
                    file_path: "src/lib.rs".to_string(),
                    line_start: 8,
                    line_end: 8,
                    char_start: None,
                    char_end: None,
                    anchor_text: "head target".to_string(),
                    context_before: "head before".to_string(),
                    context_after: "head after".to_string(),
                    placement_status: review_types::AnchorPlacementStatus::Anchored,
                    match_method: review_types::AnchorMatchMethod::ExactAtLine,
                },
            ],
            aggregate_status: review_types::AnchorAggregateStatus::Anchored,
        };

        let anchor = resolve_anchor(
            &comment,
            "base before\nbase target\nbase after\nhead before\nhead target\nhead after\n",
            "combined-blob".to_string(),
            None,
        );

        assert_eq!(
            anchor.aggregate_status,
            review_types::AnchorAggregateStatus::Anchored
        );
        let base = segment_for_side(&anchor, review_types::CommentAnchorSide::Base);
        let head = segment_for_side(&anchor, review_types::CommentAnchorSide::Head);
        assert_eq!(base.line_start, 2);
        assert_eq!(base.line_end, 2);
        assert_eq!(base.anchor_text, "base target");
        assert_eq!(
            base.match_method,
            review_types::AnchorMatchMethod::ExactElsewhere
        );
        assert_eq!(head.line_start, 5);
        assert_eq!(head.line_end, 5);
        assert_eq!(head.anchor_text, "head target");
        assert_eq!(
            head.match_method,
            review_types::AnchorMatchMethod::ExactElsewhere
        );
    }

    #[test]
    fn compound_reanchor_marks_partial_when_one_segment_is_orphaned() {
        let mut comment = stored_comment(8, "head target");
        comment.anchor = review_types::CommentAnchor {
            segments: vec![
                review_types::CommentAnchorSegment {
                    side: review_types::CommentAnchorSide::Base,
                    file_path: "src/lib.rs".to_string(),
                    line_start: 2,
                    line_end: 2,
                    char_start: None,
                    char_end: None,
                    anchor_text: "base target".to_string(),
                    context_before: "base before".to_string(),
                    context_after: "base after".to_string(),
                    placement_status: review_types::AnchorPlacementStatus::Anchored,
                    match_method: review_types::AnchorMatchMethod::ExactAtLine,
                },
                review_types::CommentAnchorSegment {
                    side: review_types::CommentAnchorSide::Head,
                    file_path: "src/lib.rs".to_string(),
                    line_start: 8,
                    line_end: 8,
                    char_start: None,
                    char_end: None,
                    anchor_text: "head target".to_string(),
                    context_before: "head before".to_string(),
                    context_after: "head after".to_string(),
                    placement_status: review_types::AnchorPlacementStatus::Anchored,
                    match_method: review_types::AnchorMatchMethod::ExactAtLine,
                },
            ],
            aggregate_status: review_types::AnchorAggregateStatus::Anchored,
        };

        let anchor = resolve_anchor(
            &comment,
            "head before\nhead target\nhead after\n",
            "head-blob".to_string(),
            None,
        );

        assert_eq!(
            anchor.aggregate_status,
            review_types::AnchorAggregateStatus::Partial
        );
        let base = segment_for_side(&anchor, review_types::CommentAnchorSide::Base);
        let head = segment_for_side(&anchor, review_types::CommentAnchorSide::Head);
        assert_eq!(
            base.placement_status,
            review_types::AnchorPlacementStatus::Orphaned
        );
        assert_eq!(base.match_method, review_types::AnchorMatchMethod::NotFound);
        assert_eq!(
            head.placement_status,
            review_types::AnchorPlacementStatus::Anchored
        );
        assert_eq!(head.line_start, 2);
    }

    #[test]
    fn compound_reanchor_uses_context_for_base_segment() {
        let mut comment = stored_comment(5, "head target");
        comment.anchor = review_types::CommentAnchor {
            segments: vec![
                review_types::CommentAnchorSegment {
                    side: review_types::CommentAnchorSide::Base,
                    file_path: "src/lib.rs".to_string(),
                    line_start: 5,
                    line_end: 5,
                    char_start: None,
                    char_end: None,
                    anchor_text: "old base".to_string(),
                    context_before: "base before".to_string(),
                    context_after: "base after".to_string(),
                    placement_status: review_types::AnchorPlacementStatus::Anchored,
                    match_method: review_types::AnchorMatchMethod::ExactAtLine,
                },
                review_types::CommentAnchorSegment {
                    side: review_types::CommentAnchorSide::Head,
                    file_path: "src/lib.rs".to_string(),
                    line_start: 8,
                    line_end: 8,
                    char_start: None,
                    char_end: None,
                    anchor_text: "head target".to_string(),
                    context_before: "head before".to_string(),
                    context_after: "head after".to_string(),
                    placement_status: review_types::AnchorPlacementStatus::Anchored,
                    match_method: review_types::AnchorMatchMethod::ExactAtLine,
                },
            ],
            aggregate_status: review_types::AnchorAggregateStatus::Anchored,
        };

        let anchor = resolve_anchor(
            &comment,
            "base before\nnew base\nbase after\nhead before\nhead target\nhead after\n",
            "combined-blob".to_string(),
            None,
        );

        let base = segment_for_side(&anchor, review_types::CommentAnchorSide::Base);
        assert_eq!(
            anchor.aggregate_status,
            review_types::AnchorAggregateStatus::Anchored
        );
        assert_eq!(base.line_start, 2);
        assert_eq!(base.line_end, 2);
        assert_eq!(base.anchor_text, "new base");
        assert_eq!(base.match_method, review_types::AnchorMatchMethod::Context);
    }

    #[test]
    fn compound_reanchor_marks_orphaned_when_all_segments_are_missing() {
        let mut comment = stored_comment(5, "head target");
        comment.anchor = review_types::CommentAnchor {
            segments: vec![
                review_types::CommentAnchorSegment {
                    side: review_types::CommentAnchorSide::Base,
                    file_path: "src/lib.rs".to_string(),
                    line_start: 5,
                    line_end: 5,
                    char_start: None,
                    char_end: None,
                    anchor_text: "base target".to_string(),
                    context_before: "base before".to_string(),
                    context_after: "base after".to_string(),
                    placement_status: review_types::AnchorPlacementStatus::Anchored,
                    match_method: review_types::AnchorMatchMethod::ExactAtLine,
                },
                review_types::CommentAnchorSegment {
                    side: review_types::CommentAnchorSide::Head,
                    file_path: "src/lib.rs".to_string(),
                    line_start: 8,
                    line_end: 8,
                    char_start: None,
                    char_end: None,
                    anchor_text: "head target".to_string(),
                    context_before: "head before".to_string(),
                    context_after: "head after".to_string(),
                    placement_status: review_types::AnchorPlacementStatus::Anchored,
                    match_method: review_types::AnchorMatchMethod::ExactAtLine,
                },
            ],
            aggregate_status: review_types::AnchorAggregateStatus::Anchored,
        };

        let anchor = resolve_anchor(&comment, "unrelated\ncontent\n", "new".to_string(), None);

        assert_eq!(
            anchor.aggregate_status,
            review_types::AnchorAggregateStatus::Orphaned
        );
        assert!(anchor.segments.iter().all(|segment| {
            segment.placement_status == review_types::AnchorPlacementStatus::Orphaned
                && segment.match_method == review_types::AnchorMatchMethod::NotFound
        }));
    }

    #[test]
    fn same_branch_head_start_context_change_keeps_exact_anchor() {
        let comment = stored_comment(3, "target");

        let anchor = resolve_anchor(
            &comment,
            "changed before\ntarget\nafter\n",
            "new".to_string(),
            None,
        );
        let head = segment_for_side(&anchor, review_types::CommentAnchorSide::Head);

        assert_eq!(
            anchor.aggregate_status,
            review_types::AnchorAggregateStatus::Anchored
        );
        assert_eq!(head.line_start, 2);
        assert_eq!(
            head.match_method,
            review_types::AnchorMatchMethod::ExactElsewhere
        );
        assert_eq!(head.context_before, "changed before");
    }

    #[test]
    fn same_branch_head_selected_text_change_anchors_by_context() {
        let comment = stored_comment(2, "old target");

        let anchor = resolve_anchor(
            &comment,
            "before\nnew target\nafter\n",
            "new".to_string(),
            None,
        );
        let head = segment_for_side(&anchor, review_types::CommentAnchorSide::Head);

        assert_eq!(
            anchor.aggregate_status,
            review_types::AnchorAggregateStatus::Anchored
        );
        assert_eq!(head.line_start, 2);
        assert_eq!(head.anchor_text, "new target");
        assert_eq!(head.match_method, review_types::AnchorMatchMethod::Context);
    }

    #[test]
    fn same_branch_head_selected_text_removed_without_context_orphans() {
        let mut comment = stored_comment(2, "target");
        comment.context_before = "missing before".to_string();
        comment.context_after = "missing after".to_string();
        comment.anchor = review_types::CommentAnchor {
            segments: vec![review_types::CommentAnchorSegment {
                side: review_types::CommentAnchorSide::Head,
                file_path: "src/lib.rs".to_string(),
                line_start: 2,
                line_end: 2,
                char_start: None,
                char_end: None,
                anchor_text: "target".to_string(),
                context_before: "missing before".to_string(),
                context_after: "missing after".to_string(),
                placement_status: review_types::AnchorPlacementStatus::Anchored,
                match_method: review_types::AnchorMatchMethod::ExactAtLine,
            }],
            aggregate_status: review_types::AnchorAggregateStatus::Anchored,
        };

        let anchor = resolve_anchor(&comment, "unrelated\ncontent\n", "new".to_string(), None);
        let head = segment_for_side(&anchor, review_types::CommentAnchorSide::Head);

        assert_eq!(
            anchor.aggregate_status,
            review_types::AnchorAggregateStatus::Orphaned
        );
        assert_eq!(
            head.placement_status,
            review_types::AnchorPlacementStatus::Orphaned
        );
        assert_eq!(head.match_method, review_types::AnchorMatchMethod::NotFound);
    }

    #[test]
    fn rebase_base_only_deleted_line_reanchors_exact_elsewhere() {
        let comment = base_only_comment(10, "deleted target");

        let anchor = resolve_anchor(
            &comment,
            "before\ndeleted target\nafter\n",
            "base-blob".to_string(),
            None,
        );

        assert_eq!(anchor.segments.len(), 1);
        let base = segment_for_side(&anchor, review_types::CommentAnchorSide::Base);
        assert_eq!(
            anchor.aggregate_status,
            review_types::AnchorAggregateStatus::Anchored
        );
        assert_eq!(base.line_start, 2);
        assert_eq!(
            base.match_method,
            review_types::AnchorMatchMethod::ExactElsewhere
        );
    }

    #[test]
    fn rebase_base_only_selected_text_removed_with_context_anchors_by_context() {
        let comment = base_only_comment(10, "deleted target");

        let anchor = resolve_anchor(
            &comment,
            "before\nreplacement base\nafter\n",
            "base-blob".to_string(),
            None,
        );

        assert_eq!(anchor.segments.len(), 1);
        let base = segment_for_side(&anchor, review_types::CommentAnchorSide::Base);
        assert_eq!(
            anchor.aggregate_status,
            review_types::AnchorAggregateStatus::Anchored
        );
        assert_eq!(base.line_start, 2);
        assert_eq!(base.anchor_text, "replacement base");
        assert_eq!(base.match_method, review_types::AnchorMatchMethod::Context);
    }

    #[test]
    fn resolves_shifted_anchor_by_exact_text() {
        let comment = stored_comment(2, "target");
        let anchor = resolve_anchor(
            &comment,
            "before\nother\ntarget\nafter\n",
            "new".to_string(),
            None,
        );

        assert_eq!(anchor_status(&anchor), review_types::AnchorStatus::Shifted);
        assert_eq!(head_segment(&anchor).line_start, 3);
        assert_eq!(head_segment(&anchor).anchor_text, "target");
    }

    #[test]
    fn resolves_approximate_anchor_from_context() {
        let comment = stored_comment(2, "target");
        let anchor = resolve_anchor(
            &comment,
            "intro\nbefore\nreplacement\nafter\n",
            "new".to_string(),
            None,
        );

        assert_eq!(
            anchor_status(&anchor),
            review_types::AnchorStatus::Approximate
        );
        assert_eq!(head_segment(&anchor).line_start, 3);
        assert_eq!(head_segment(&anchor).anchor_text, "replacement");
    }

    #[test]
    fn approximate_anchor_spans_replacement_between_context() {
        let comment = stored_comment(2, "old one\nold two");
        let anchor = resolve_anchor(
            &comment,
            "intro\nbefore\nnew one\nnew two\nnew three\nafter\noutro\n",
            "new".to_string(),
            None,
        );

        assert_eq!(
            anchor_status(&anchor),
            review_types::AnchorStatus::Approximate
        );
        assert_eq!(head_segment(&anchor).line_start, 3);
        assert_eq!(head_segment(&anchor).line_end, 5);
        assert_eq!(
            head_segment(&anchor).anchor_text,
            "new one\nnew two\nnew three"
        );
    }

    #[test]
    fn approximate_anchor_uses_multiline_line_matches() {
        let comment = stored_comment(5, "keep one\nold middle\nkeep two");
        let anchor = resolve_anchor(
            &comment,
            "intro\nother\nkeep one\nnew middle\nkeep two\noutro\n",
            "new".to_string(),
            None,
        );

        assert_eq!(
            anchor_status(&anchor),
            review_types::AnchorStatus::Approximate
        );
        assert_eq!(head_segment(&anchor).line_start, 3);
        assert_eq!(head_segment(&anchor).line_end, 5);
        assert_eq!(
            head_segment(&anchor).anchor_text,
            "keep one\nnew middle\nkeep two"
        );
    }

    #[test]
    fn multiline_line_match_prefers_nearest_candidate() {
        let comment = stored_comment(5, "same one\nold middle\nsame two");
        let anchor = resolve_anchor(
            &comment,
            "same one\nleft\nsame two\nspacer\nsame one\nright\nsame two\n",
            "new".to_string(),
            None,
        );

        assert_eq!(
            anchor_status(&anchor),
            review_types::AnchorStatus::Approximate
        );
        assert_eq!(head_segment(&anchor).line_start, 5);
        assert_eq!(head_segment(&anchor).line_end, 7);
        assert_eq!(
            head_segment(&anchor).anchor_text,
            "same one\nright\nsame two"
        );
    }

    #[test]
    fn marks_anchor_orphaned_when_text_and_context_do_not_match() {
        let comment = stored_comment(2, "target");
        let anchor = resolve_anchor(&comment, "unrelated\ncontent\n", "new".to_string(), None);

        assert_eq!(anchor_status(&anchor), review_types::AnchorStatus::Orphaned);
        assert_eq!(head_segment(&anchor).line_start, 2);
        assert_eq!(head_segment(&anchor).anchor_text, "target");
    }

    #[test]
    fn shifted_anchor_prefers_nearest_match() {
        // "target" appears at lines 1, 4, and 7.  Stored position is
        // line 5, so the nearest occurrence is line 4.
        let comment = stored_comment(5, "target");
        let anchor = resolve_anchor(
            &comment,
            "target\naaa\nbbb\ntarget\nccc\nddd\ntarget\n",
            "new".to_string(),
            None,
        );

        assert_eq!(anchor_status(&anchor), review_types::AnchorStatus::Shifted);
        assert_eq!(head_segment(&anchor).line_start, 4);
    }

    #[test]
    fn duplicate_text_keeps_exact_at_line_match_when_available() {
        let comment = stored_comment(4, "target");
        let anchor = resolve_anchor(
            &comment,
            "target\naaa\nbbb\ntarget\nccc\n",
            "new".to_string(),
            None,
        );

        let head = head_segment(&anchor);
        assert_eq!(anchor_status(&anchor), review_types::AnchorStatus::Anchored);
        assert_eq!(head.line_start, 4);
        assert_eq!(
            head.match_method,
            review_types::AnchorMatchMethod::ExactAtLine
        );
    }

    #[test]
    fn start_context_without_end_context_does_not_false_anchor_changed_text() {
        let comment = stored_comment(2, "old target");
        let anchor = resolve_anchor(
            &comment,
            "before\nunrelated\nmissing after\n",
            "new".to_string(),
            None,
        );

        let head = head_segment(&anchor);
        assert_eq!(anchor_status(&anchor), review_types::AnchorStatus::Orphaned);
        assert_eq!(
            head.placement_status,
            review_types::AnchorPlacementStatus::Orphaned
        );
        assert_eq!(head.match_method, review_types::AnchorMatchMethod::NotFound);
    }

    #[test]
    fn end_context_without_start_context_does_not_false_anchor_changed_text() {
        let comment = stored_comment(2, "old target");
        let anchor = resolve_anchor(
            &comment,
            "missing before\nunrelated\nafter\n",
            "new".to_string(),
            None,
        );

        let head = head_segment(&anchor);
        assert_eq!(anchor_status(&anchor), review_types::AnchorStatus::Orphaned);
        assert_eq!(
            head.placement_status,
            review_types::AnchorPlacementStatus::Orphaned
        );
        assert_eq!(head.match_method, review_types::AnchorMatchMethod::NotFound);
    }

    #[test]
    fn deleted_base_side_marks_paired_anchor_partial_when_head_anchors() {
        let comment = compound_comment_with_segments(vec![
            anchor_segment(
                review_types::CommentAnchorSide::Base,
                2,
                "base target",
                "base before",
                "base after",
            ),
            anchor_segment(
                review_types::CommentAnchorSide::Head,
                2,
                "head target",
                "head before",
                "head after",
            ),
        ]);

        let anchor = resolve_anchor_with_side_content(
            &comment,
            None,
            Some("head before\nhead target\nhead after\n"),
        );

        let base = segment_for_side(&anchor, review_types::CommentAnchorSide::Base);
        let head = segment_for_side(&anchor, review_types::CommentAnchorSide::Head);
        assert_eq!(
            anchor.aggregate_status,
            review_types::AnchorAggregateStatus::Partial
        );
        assert_eq!(
            base.placement_status,
            review_types::AnchorPlacementStatus::Orphaned
        );
        assert_eq!(
            head.placement_status,
            review_types::AnchorPlacementStatus::Anchored
        );
        assert_eq!(head.line_start, 2);
    }

    #[test]
    fn recreated_unrelated_file_does_not_false_anchor_segment() {
        let comment = base_only_comment(2, "deleted target");

        let anchor =
            resolve_anchor_with_side_content(&comment, Some("new unrelated\nfile content\n"), None);

        let base = segment_for_side(&anchor, review_types::CommentAnchorSide::Base);
        assert_eq!(
            anchor.aggregate_status,
            review_types::AnchorAggregateStatus::Orphaned
        );
        assert_eq!(
            base.placement_status,
            review_types::AnchorPlacementStatus::Orphaned
        );
        assert_eq!(base.match_method, review_types::AnchorMatchMethod::NotFound);
    }

    #[test]
    fn renamed_file_without_lookup_orphans_segment() {
        let comment = base_only_comment(2, "deleted target");

        let anchor = resolve_anchor_with_side_content(&comment, None, None);

        let base = segment_for_side(&anchor, review_types::CommentAnchorSide::Base);
        assert_eq!(
            anchor.aggregate_status,
            review_types::AnchorAggregateStatus::Orphaned
        );
        assert_eq!(
            base.placement_status,
            review_types::AnchorPlacementStatus::Orphaned
        );
        assert_eq!(base.file_blob_sha, "");
    }

    #[test]
    fn shifted_anchor_nearest_at_end_of_file() {
        // "target" appears at lines 1 and 6.  Stored position is line 5,
        // so the nearest occurrence is line 6.
        let comment = stored_comment(5, "target");
        let anchor = resolve_anchor(
            &comment,
            "target\naaa\nbbb\nccc\nddd\ntarget\n",
            "new".to_string(),
            None,
        );

        assert_eq!(anchor_status(&anchor), review_types::AnchorStatus::Shifted);
        assert_eq!(head_segment(&anchor).line_start, 6);
    }

    #[test]
    fn adjusted_hint_finds_exact_match_at_shifted_position() {
        // "target" at line 2, but diff inserted a line before it so the
        // adjusted hint points to line 3 (0-indexed: 2).
        let comment = stored_comment(2, "target");
        let anchor = resolve_anchor(
            &comment,
            "before\nnew_line\ntarget\nafter\n",
            "new".to_string(),
            Some(2), // adjusted hint: 0-indexed line 2
        );

        // Should be Anchored, not Shifted, because the adjusted hint
        // points directly at the correct line.
        assert_eq!(anchor_status(&anchor), review_types::AnchorStatus::Anchored);
        assert_eq!(head_segment(&anchor).line_start, 3);
    }

    #[test]
    fn translate_line_accounts_for_insertions() {
        // Hunk inserts 2 lines at the start: old 1-1 → new 1-3.
        let hunks = vec![review_types::DiffHunk {
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 3,
            header: String::new(),
            lines: vec![
                review_types::DiffLine {
                    kind: review_types::LineKind::Deletion,
                    content: "old\n".to_string(),
                    old_lineno: Some(1),
                    new_lineno: None,
                },
                review_types::DiffLine {
                    kind: review_types::LineKind::Addition,
                    content: "new1\n".to_string(),
                    old_lineno: None,
                    new_lineno: Some(1),
                },
                review_types::DiffLine {
                    kind: review_types::LineKind::Addition,
                    content: "new2\n".to_string(),
                    old_lineno: None,
                    new_lineno: Some(2),
                },
                review_types::DiffLine {
                    kind: review_types::LineKind::Addition,
                    content: "new3\n".to_string(),
                    old_lineno: None,
                    new_lineno: Some(3),
                },
            ],
        }];
        // Old line 5 (0-indexed 4) should shift by +2 (3 added - 1 deleted).
        assert_eq!(translate_line_through_hunks(&hunks, 4), 6);
    }

    #[test]
    fn translate_line_accounts_for_deletions() {
        // Hunk deletes 3 lines: old 2-4 → new 2-1.
        let hunks = vec![review_types::DiffHunk {
            old_start: 2,
            old_lines: 3,
            new_start: 2,
            new_lines: 0,
            header: String::new(),
            lines: vec![
                review_types::DiffLine {
                    kind: review_types::LineKind::Deletion,
                    content: "a\n".to_string(),
                    old_lineno: Some(2),
                    new_lineno: None,
                },
                review_types::DiffLine {
                    kind: review_types::LineKind::Deletion,
                    content: "b\n".to_string(),
                    old_lineno: Some(3),
                    new_lineno: None,
                },
                review_types::DiffLine {
                    kind: review_types::LineKind::Deletion,
                    content: "c\n".to_string(),
                    old_lineno: Some(4),
                    new_lineno: None,
                },
            ],
        }];
        // Old line 6 (0-indexed 5) should shift by -3.
        assert_eq!(translate_line_through_hunks(&hunks, 5), 2);
    }
}
