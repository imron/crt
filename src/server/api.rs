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
    let comments = {
        let db_guard = db.lock().await;
        match db_guard.list_comments(
            ctx.merge_base_key(),
            &head_ref,
            p.file_path.as_deref(),
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
    let Some(content) = range_file_content(ctx, range, segment.side, &segment.file_path).await?
    else {
        return Ok(comment.file_blob_sha.is_empty());
    };
    Ok(content.file_blob_sha == comment.file_blob_sha)
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
        let content = range_file_content(ctx, range, segment.side, &segment.file_path).await?;
        let resolved = match content {
            Some(content) => {
                let adjusted_hint = adjusted_hint_for_range_segment(ctx, range, segment).await;
                resolve_anchor_segment(
                    segment,
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
                    content,
                    file_blob_sha,
                }))
        }
        ReanchorEndpoint::Commit { refspec } => Ok(commit_file_content(ctx, refspec, file_path)
            .await?
            .map(|(content, file_blob_sha)| ReanchorFileContent {
                content,
                file_blob_sha,
            })),
    }
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
            return anchor_segment_from_range(
                segment,
                &lines,
                candidate,
                span,
                file_blob_sha,
                review_types::AnchorStatus::Anchored,
            );
        }
    }

    let hint = adjusted_hint.or(stored_index).unwrap_or(0);

    if let Some(index) = find_sequence_nearest(&lines, &anchor_lines, hint) {
        return anchor_segment_from_range(
            segment,
            &lines,
            index,
            span,
            file_blob_sha,
            review_types::AnchorStatus::Shifted,
        );
    }

    if let Some(index) = find_multiline_line_match_nearest(&lines, &anchor_lines, hint) {
        return anchor_segment_from_range(
            segment,
            &lines,
            index,
            span,
            file_blob_sha,
            review_types::AnchorStatus::Approximate,
        );
    }

    if let Some((index, context_span)) = find_context_match(
        &segment.context_before,
        &segment.context_after,
        &lines,
        span,
    ) {
        return anchor_segment_from_range(
            segment,
            &lines,
            index,
            context_span,
            file_blob_sha,
            review_types::AnchorStatus::Approximate,
        );
    }

    orphaned_anchor_segment(segment, file_blob_sha)
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

fn find_sequence(lines: &[&str], needle: &[&str]) -> Option<usize> {
    if needle.is_empty() || needle.len() > lines.len() {
        return None;
    }
    let last_start = lines.len() - needle.len();
    (0..=last_start).find(|index| matches_sequence(lines, *index, needle))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineMatchCandidate {
    start: usize,
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

fn find_multiline_line_match_nearest(
    lines: &[&str],
    needle: &[&str],
    hint: usize,
) -> Option<usize> {
    if needle.len() < 2 || needle.len() > lines.len() {
        return None;
    }

    let last_start = lines.len() - needle.len();
    let hint = hint.min(last_start);
    let mut best: Option<LineMatchCandidate> = None;

    for start in 0..=last_start {
        let Some(candidate) = line_match_candidate(lines, needle, start, hint) else {
            continue;
        };
        if best
            .as_ref()
            .is_none_or(|existing| candidate.is_preferred_to(existing))
        {
            best = Some(candidate);
        }
    }

    best.map(|candidate| candidate.start)
}

fn line_match_candidate(
    lines: &[&str],
    needle: &[&str],
    start: usize,
    hint: usize,
) -> Option<LineMatchCandidate> {
    let mut matched_lines = 0;
    let mut unique_matched_lines = 0;

    for (offset, needle_line) in needle.iter().enumerate() {
        if needle_line.trim().is_empty() || lines[start + offset] != *needle_line {
            continue;
        }

        matched_lines += 1;
        if count_line_occurrences(lines, needle_line) == 1 {
            unique_matched_lines += 1;
        }
    }

    if matched_lines == 0 || (unique_matched_lines == 0 && matched_lines < 2) {
        return None;
    }

    let distance_from_hint = if start >= hint {
        start - hint
    } else {
        hint - start
    };

    Some(LineMatchCandidate {
        start,
        matched_lines,
        unique_matched_lines,
        distance_from_hint,
    })
}

fn count_line_occurrences(lines: &[&str], needle: &str) -> usize {
    lines.iter().filter(|line| **line == needle).count()
}

fn find_context_match(
    context_before: &str,
    context_after: &str,
    lines: &[&str],
    span: usize,
) -> Option<(usize, usize)> {
    let before = context_lines(context_before);
    let after = context_lines(context_after);

    if !before.is_empty() && !after.is_empty() {
        let mut start = 0;
        while let Some(offset) = find_sequence(&lines[start..], &before) {
            let context_start = start + offset;
            let candidate = context_start + before.len();
            if candidate < lines.len() {
                if let Some(after_offset) = find_sequence(&lines[candidate..], &after) {
                    if after_offset > 0 {
                        return Some((candidate, after_offset));
                    }
                }
            }
            start = context_start + 1;
            if start >= lines.len() {
                break;
            }
        }
    }

    if !before.is_empty() && after.is_empty() {
        let mut start = 0;
        while let Some(offset) = find_sequence(&lines[start..], &before) {
            let context_start = start + offset;
            let candidate = context_start + before.len();
            if candidate < lines.len() {
                return Some((candidate, span));
            }
            start = context_start + 1;
            if start >= lines.len() {
                break;
            }
        }
    }

    if !after.is_empty() {
        return find_sequence(lines, &after).map(|after_index| {
            let start = after_index.saturating_sub(span);
            (start, span)
        });
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
        }
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
        failures: &mut Vec<String>,
    ) {
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
