# Stage 18: Rebase Review Migration

## Status: Complete

## Goal

Automatically detect when a branch has been rebased and migrate existing
review state to the new scope, so the reviewer doesn't lose their progress.

## Why

Reviews are scoped by `(merge_base, head_ref)`. After a rebase, the
`merge_base` changes (e.g., from `a` to `d`), so `load_reviews` returns
nothing for the new scope — even though `head_ref` (branch name) is
the same and the review work is still valid.

```
Before rebase:                 After rebase:

a---b---c---d                  a---b---c---d---e*---f*---g*
 \                              
  \-e---f---g                  (e, f, g are now dangling)
```

Without migration, the reviewer must re-review every file from scratch
after each rebase. With migration, files that haven't changed stay
reviewed, and files that changed show only the delta since the review.

## Detection

In `handle_list_changed_files` (or during `init`):

1. Load reviews for the current `(merge_base, head_ref)`.
2. If no reviews are found, query for reviews with the **same `head_ref`**
   but a **different `merge_base`**.
3. If old reviews are found, a rebase (or merge-base shift) likely
   happened — attempt automatic migration.
4. If multiple old scopes exist (multiple prior rebases), pick the scope
   with the most recent `reviewed_at` timestamp, since those reviews
   supersede older ones.

## Migration Logic

For each old review record:

1. **Verify `reviewed_commit` is resolvable** — the old commit still
   exists in git's object store (dangling objects survive for
   `gc.pruneExpire`, typically 2 weeks).

2. **Compare file content**: get the file's blob hash at
   `reviewed_commit` and at current HEAD (or working tree).

   - **Blob matches** — file content unchanged after rebase. Migrate the
     review to the new scope, recompute `diff_hash` against the new
     `merge_base` so change detection works going forward. Mark as
     Reviewed.

   - **Blob differs** — content changed during rebase. Migrate the review
     to the new scope as Changed, keeping the old `reviewed_commit` so
     the diff shows only what's new since the review.

   - **File no longer in changes** — skip (fully merged or removed).

3. **`reviewed_commit` not resolvable** (GC'd) — check if the file's
   current `diff_hash` (against the new merge_base) matches the stored
   `diff_hash`. If it matches, keep the file as Reviewed but with no
   `reviewed_commit` (fall back to merge_base for diffing). If it
   doesn't match, treat as Unreviewed.

## Post-Migration

- Store migrated reviews under the new `(merge_base, head_ref)` scope.
- Delete the old scope's review records to prevent re-migration on
  subsequent startups.
- Show a status message or notification: e.g.,
  `"Migrated N reviews from previous scope"`.

## Implementation

### 1. DB Layer (`src/db.rs`)

- Add `load_reviews_by_head_ref(head_ref)`.
  - It returns all reviews for a given `head_ref` regardless of `merge_base`.
  - Results are grouped by merge base and ordered by most recent
    `reviewed_at` descending.
- Add a bulk migration helper or reuse `store_review()` in a loop.

### 2. Git Layer (`src/git.rs`)

- Add `file_blob_hash(commit_ref, file_path) -> Result<Option<String>>`
  to get the blob OID of a file at a given commit. Returns `None` if
  the file doesn't exist at that commit.
- Optionally add `commit_exists(oid) -> bool` for a quick reachability
  check.

### 3. Server Logic (`src/server/api.rs`)

In `handle_list_changed_files`:
1. Load reviews for current scope.
2. If empty, call the new DB method to find old-scope reviews.
3. Pick the most recent old scope.
4. For each old review, run the migration logic (blob compare, diff_hash
   recompute).
5. Store migrated reviews under the new scope.
6. Delete old scope records.
7. Continue with normal file list construction using the migrated reviews.

### 4. Notification (`src/server/notify.rs`)

- Optionally add a `ReviewsMigrated { count }` notification kind so the
  client can display a status message.

## Edge Cases

- **No old reviews for head_ref**: No migration needed. Normal startup.
- **Detached HEAD**: `head_ref` is a short hash, not a branch name.
  Migration won't find a match, which is correct — detached HEAD reviews
  are inherently ephemeral.
- **Force push without rebase**: Same effect as rebase — merge_base may
  change, migration still applies.
- **Amended commits**: Same as rebase — old commit becomes dangling, new
  commit has a new OID. Migration handles this identically.
- **Multiple worktrees**: Reviews are stored per-repo (via commondir), so
  migration applies across worktrees sharing the same branch name.
