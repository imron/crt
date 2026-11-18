//! JSON-RPC API method implementations.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use tokio::sync::broadcast;

use super::{ConnectionContext, ServerState};
use crate::db::{
    Database, NewAnchorVersion, NewComment, NewCommentAnchor, NewCommentResolutionEvent,
    StoredComment,
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

    // Resolve repo context and merge-base via git module (blocking operation)
    let wt = worktree_path.clone();
    let br = base_ref.clone();
    let git_result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let repo = git::Repo::open(&wt)?;
        let ctx = repo.context()?;
        let review_base = repo.resolve_review_base(&br)?;
        let merge_base = repo.merge_base(&br, "HEAD")?;
        Ok((ctx, git::CommitId::new(merge_base), review_base))
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

    let ctx = ConnectionContext {
        repo_root: git_ctx.repo_root.clone(),
        worktree: worktree_path.clone(),
        base_ref: base_ref.clone(),
        review_base,
        merge_base: merge_base.clone(),
        head: git_ctx.head.clone(),
        db_path,
    };

    let result = review_types::ConnectionContext {
        repo_root: git_ctx.repo_root.to_string_lossy().into_owned(),
        worktree: worktree_path.to_string_lossy().into_owned(),
        head_ref: git_ctx.head.display_name(),
        merge_base: merge_base.to_string(),
        base_ref: base_ref.clone(),
    };

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

fn summarize_diff(diff: &review_types::DiffContent) -> review_types::DiffSummary {
    let mut additions = 0;
    let mut deletions = 0;
    for hunk in &diff.hunks {
        for line in &hunk.lines {
            match line.kind {
                review_types::LineKind::Addition => additions += 1,
                review_types::LineKind::Deletion => deletions += 1,
                review_types::LineKind::Context => {}
            }
        }
    }

    review_types::DiffSummary {
        hunks: diff.hunks.len(),
        additions,
        deletions,
        is_binary: diff.is_binary,
        diff_hash: diff.diff_hash.clone(),
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

    // Git operations are blocking — run on the blocking thread pool.
    let git_result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let repo = git::Repo::open(&worktree)?;
        let changes = repo.list_changed_files_workdir(&merge_base)?;

        let mut files = Vec::new();
        for change in changes {
            let diff = repo.diff_file_workdir(&merge_base, &change.path)?;

            let status = match reviews.get(&change.path) {
                None => review_types::ReviewStatus::Unreviewed,
                Some(review) if review.diff_hash == diff.diff_hash => {
                    review_types::ReviewStatus::Reviewed {
                        at: review.reviewed_at.clone(),
                        reviewed_commit: if review.reviewed_commit.is_empty() {
                            None
                        } else {
                            Some(review.reviewed_commit.clone())
                        },
                    }
                }
                Some(review) => review_types::ReviewStatus::Changed {
                    at: review.reviewed_at.clone(),
                    reviewed_commit: if review.reviewed_commit.is_empty() {
                        None
                    } else {
                        Some(review.reviewed_commit.clone())
                    },
                },
            };

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

    let git_result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let repo = git::Repo::open(&worktree)?;
        let changes = repo.list_changed_files_workdir(&merge_base)?;

        let mut files = Vec::new();
        for change in changes {
            let diff = repo.diff_file_workdir(&merge_base, &change.path)?;
            let status = match reviews.get(&change.path) {
                None => review_types::ReviewStatus::Unreviewed,
                Some(review) if review.diff_hash == diff.diff_hash => {
                    review_types::ReviewStatus::Reviewed {
                        at: review.reviewed_at.clone(),
                        reviewed_commit: if review.reviewed_commit.is_empty() {
                            None
                        } else {
                            Some(review.reviewed_commit.clone())
                        },
                    }
                }
                Some(review) => review_types::ReviewStatus::Changed {
                    at: review.reviewed_at.clone(),
                    reviewed_commit: if review.reviewed_commit.is_empty() {
                        None
                    } else {
                        Some(review.reviewed_commit.clone())
                    },
                },
            };

            let diff = summarize_diff(&diff);
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
    let file_path = diff_params.file_path;

    let git_result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let repo = git::Repo::open(&worktree)?;
        let diff = repo.diff_file_workdir(&merge_base, &file_path)?;
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

    // Compute the current diff hash.
    let worktree = ctx.worktree.clone();
    let merge_base = ctx.merge_base.to_string();
    let head_ref = ctx.head_scope_key();
    let file_path = p.file_path.clone();

    let (diff_hash, reviewed_commit) = {
        let wt = worktree.clone();
        let mb = merge_base.clone();
        let fp = file_path.clone();
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(String, String)> {
            let repo = git::Repo::open(&wt)?;
            let diff = repo.diff_file_workdir(&mb, &fp)?;
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

    // Broadcast notification.
    let _ = notify_tx.send(Notification {
        base_ref: merge_base,
        head_ref,
        kind: NotificationKind::ReviewChanged {
            file_path: file_path.clone(),
        },
    });

    let result = review_types::ReviewActionResult {
        file_path,
        status: review_types::ReviewStatus::Reviewed {
            at: reviewed_at,
            reviewed_commit: Some(reviewed_commit),
        },
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

    // Broadcast notification.
    let _ = notify_tx.send(Notification {
        base_ref: merge_base,
        head_ref,
        kind: NotificationKind::ReviewChanged {
            file_path: p.file_path.clone(),
        },
    });

    let result = review_types::ReviewActionResult {
        file_path: p.file_path,
        status: review_types::ReviewStatus::Unreviewed,
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
    let file_path = p.file_path;
    let file_blob_sha = match current_file_hash(ctx, &file_path).await {
        Ok(hash) => hash,
        Err(e) => {
            return JsonRpcResponse::error(
                id.clone(),
                ERR_INTERNAL,
                format!("Failed to hash anchor file: {e:#}"),
            );
        }
    };
    let new = NewComment {
        merge_base: merge_base.clone(),
        head_ref: head_ref.clone(),
        file_path,
        body: p.body,
        anchor: NewCommentAnchor {
            file_blob_sha,
            line_start: p.line_start,
            line_end: p.line_end,
            char_start: p.char_start,
            char_end: p.char_end,
            anchor_text: p.anchor_text,
            context_before: p.context_before,
            context_after: p.context_after,
            status: review_types::AnchorStatus::Anchored,
        },
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
    let comments = {
        let db_guard = db.lock().await;
        match db_guard.list_comments(
            ctx.merge_base_key(),
            &head_ref,
            p.file_path.as_deref(),
            p.include_resolved,
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
    let mut reanchored = Vec::with_capacity(comments.len());
    for comment in comments {
        reanchored.push(reanchor_comment(ctx, db, comment).await?);
    }
    Ok(reanchored)
}

async fn reanchor_comment(
    ctx: &ConnectionContext,
    db: &Arc<Mutex<Database>>,
    comment: StoredComment,
) -> anyhow::Result<StoredComment> {
    if comment.resolved {
        return Ok(comment);
    }

    let anchor = match current_file_content(ctx, &comment.file_path).await? {
        Some((content, file_blob_sha)) => resolve_anchor(&comment, &content, file_blob_sha),
        None => orphaned_anchor(&comment, String::new()),
    };

    {
        let db_guard = db.lock().await;
        db_guard.insert_anchor_version(&NewAnchorVersion {
            comment_id: comment.id,
            anchor: anchor.clone(),
        })?;
    }

    Ok(apply_anchor(comment, &anchor))
}

fn resolve_anchor(
    comment: &StoredComment,
    content: &str,
    file_blob_sha: String,
) -> NewCommentAnchor {
    let lines = content_lines(content);
    let anchor_lines = anchor_lines(&comment.anchor_text);
    let span = anchor_lines.len().max(1);
    let stored_index = comment
        .line_start
        .checked_sub(1)
        .and_then(|line| usize::try_from(line).ok());

    if let Some(index) = stored_index {
        if matches_sequence(&lines, index, &anchor_lines) {
            return anchor_from_range(
                comment,
                &lines,
                index,
                span,
                file_blob_sha,
                review_types::AnchorStatus::Anchored,
            );
        }
    }

    if let Some(index) = find_sequence(&lines, &anchor_lines) {
        return anchor_from_range(
            comment,
            &lines,
            index,
            span,
            file_blob_sha,
            review_types::AnchorStatus::Shifted,
        );
    }

    if let Some(index) = find_context_match(comment, &lines, span) {
        return anchor_from_range(
            comment,
            &lines,
            index,
            span,
            file_blob_sha,
            review_types::AnchorStatus::Approximate,
        );
    }

    orphaned_anchor(comment, file_blob_sha)
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

fn find_sequence(lines: &[&str], needle: &[&str]) -> Option<usize> {
    if needle.is_empty() || needle.len() > lines.len() {
        return None;
    }

    let last_start = lines.len() - needle.len();
    (0..=last_start).find(|index| matches_sequence(lines, *index, needle))
}

fn find_context_match(comment: &StoredComment, lines: &[&str], span: usize) -> Option<usize> {
    let before = context_lines(&comment.context_before);
    let after = context_lines(&comment.context_after);

    if !before.is_empty() {
        let mut start = 0;
        while let Some(offset) = find_sequence(&lines[start..], &before) {
            let context_start = start + offset;
            let candidate = context_start + before.len();
            if candidate < lines.len()
                && (after.is_empty() || matches_sequence(lines, candidate + span, &after))
            {
                return Some(candidate);
            }
            start = context_start + 1;
            if start >= lines.len() {
                break;
            }
        }
    }

    if !after.is_empty() {
        return find_sequence(lines, &after).map(|after_index| after_index.saturating_sub(span));
    }

    None
}

fn context_lines(context: &str) -> Vec<&str> {
    if context.is_empty() {
        Vec::new()
    } else {
        context.lines().collect()
    }
}

fn anchor_from_range(
    comment: &StoredComment,
    lines: &[&str],
    start: usize,
    span: usize,
    file_blob_sha: String,
    status: review_types::AnchorStatus,
) -> NewCommentAnchor {
    let safe_start = start.min(lines.len());
    let end = safe_start.saturating_add(span).min(lines.len());
    let (context_before, context_after) = context_around(lines, safe_start, end);

    NewCommentAnchor {
        file_blob_sha,
        line_start: usize_to_i64_saturating(safe_start + 1),
        line_end: usize_to_i64_saturating(end.max(safe_start + 1)),
        char_start: comment.char_start,
        char_end: comment.char_end,
        anchor_text: lines[safe_start..end].join("\n"),
        context_before,
        context_after,
        status,
    }
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

fn orphaned_anchor(comment: &StoredComment, file_blob_sha: String) -> NewCommentAnchor {
    NewCommentAnchor {
        file_blob_sha,
        line_start: comment.line_start,
        line_end: comment.line_end,
        char_start: comment.char_start,
        char_end: comment.char_end,
        anchor_text: comment.anchor_text.clone(),
        context_before: comment.context_before.clone(),
        context_after: comment.context_after.clone(),
        status: review_types::AnchorStatus::Orphaned,
    }
}

fn apply_anchor(mut comment: StoredComment, anchor: &NewCommentAnchor) -> StoredComment {
    comment.file_blob_sha = anchor.file_blob_sha.clone();
    comment.line_start = anchor.line_start;
    comment.line_end = anchor.line_end;
    comment.char_start = anchor.char_start;
    comment.char_end = anchor.char_end;
    comment.anchor_text = anchor.anchor_text.clone();
    comment.context_before = anchor.context_before.clone();
    comment.context_after = anchor.context_after.clone();
    comment.anchor_status = anchor.status;
    comment
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

    if stored.merge_base == ctx.merge_base_key() && stored.head_ref == ctx.head_scope_key() {
        Some(stored)
    } else {
        None
    }
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
    let Some(stored) = load_comment_in_scope(ctx, db, comment_id).await else {
        return JsonRpcResponse::error(
            id.clone(),
            ERR_INTERNAL,
            format!("Comment {comment_id} disappeared after update"),
        );
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
        match repo.list_changed_files_workdir(&merge_base) {
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

    fn stored_comment(line_start: i64, anchor_text: &str) -> StoredComment {
        StoredComment {
            id: 1,
            merge_base: "base".to_string(),
            head_ref: "head".to_string(),
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

    #[test]
    fn resolves_exact_anchor_at_stored_line() {
        let comment = stored_comment(2, "target");
        let anchor = resolve_anchor(&comment, "before\ntarget\nafter\n", "new".to_string());

        assert_eq!(anchor.status, review_types::AnchorStatus::Anchored);
        assert_eq!(anchor.line_start, 2);
        assert_eq!(anchor.anchor_text, "target");
    }

    #[test]
    fn resolves_shifted_anchor_by_exact_text() {
        let comment = stored_comment(2, "target");
        let anchor = resolve_anchor(
            &comment,
            "before\nother\ntarget\nafter\n",
            "new".to_string(),
        );

        assert_eq!(anchor.status, review_types::AnchorStatus::Shifted);
        assert_eq!(anchor.line_start, 3);
        assert_eq!(anchor.anchor_text, "target");
    }

    #[test]
    fn resolves_approximate_anchor_from_context() {
        let comment = stored_comment(2, "target");
        let anchor = resolve_anchor(
            &comment,
            "intro\nbefore\nreplacement\nafter\n",
            "new".to_string(),
        );

        assert_eq!(anchor.status, review_types::AnchorStatus::Approximate);
        assert_eq!(anchor.line_start, 3);
        assert_eq!(anchor.anchor_text, "replacement");
    }

    #[test]
    fn marks_anchor_orphaned_when_text_and_context_do_not_match() {
        let comment = stored_comment(2, "target");
        let anchor = resolve_anchor(&comment, "unrelated\ncontent\n", "new".to_string());

        assert_eq!(anchor.status, review_types::AnchorStatus::Orphaned);
        assert_eq!(anchor.line_start, 2);
        assert_eq!(anchor.anchor_text, "target");
    }
}
