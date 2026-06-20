# Stage 17: Diff from Reviewed Commit

## Status: Backlog

## Goal

When a file is marked as reviewed with `r`, store the HEAD commit hash at
review time. Diffs for reviewed files default to `reviewed_commit..workdir`
instead of `merge_base..workdir`, so the reviewer only sees what changed
since their last review. Pressing `m` toggles between the "since review"
and "full diff from merge base" views.

## Why

After reviewing a file, if the file changes (new commits, working tree
edits), the current behavior shows the entire diff from merge base. This
forces the reviewer to re-read everything, even though they already
reviewed most of it. By diffing from the reviewed commit, only the new
changes are shown.

## Implementation

### 1. DB Schema Migration (`src/db.rs`)
- Add `reviewed_commit TEXT` column to `file_reviews` via `ALTER TABLE`.
- Existing rows get empty string as default; treated as "no reviewed
  commit" (fall back to merge_base).
- Update `StoredReview` struct to include `reviewed_commit`.
- Update `store_review()` to accept and persist the reviewed commit hash.
- Update `load_reviews()` to read the new column.

### 2. Review Type Updates (`src/review_types.rs`)
- Add `reviewed_commit: Option<String>` to `ReviewStatus::Reviewed` and
  `ReviewStatus::Changed` variants.

### 3. Server API Changes (`src/server/api.rs`)
- `handle_mark_reviewed()`: Resolve HEAD to a full commit OID, pass to
  `store_review()`.
- `handle_list_changed_files()`: Populate `reviewed_commit` in the
  `ReviewStatus` variants from stored review data.

### 4. Git Layer (`src/git.rs`)
- Verify existing `diff_file_workdir()` / `diff_file_workdir_opts()` work
  with any commit hash as the base (they accept `base_ref: &str`, so this
  should already work).

### 5. Client-Side Diff Logic (`src/app.rs`)
- Add `show_merge_base: bool` flag to `AppState` (default `false`).
- When computing diffs: if the file has a `reviewed_commit` and
  `show_merge_base` is `false`, use `reviewed_commit` as the diff base;
  otherwise use `merge_base`.
- Unreviewed files always use `merge_base` regardless of the flag.

### 6. Keybinding (`src/keys.rs`)
- Bind `m` to toggle `show_merge_base`.
- Trigger a diff reload after toggling.
- Show status message: `"Diff base: merge base"` /
  `"Diff base: since review"`.

### 7. UI Indicator (`src/ui/`)
- Show the current diff base mode in the status bar or diff header.

## Edge Cases

- **Unreviewed files**: `m` is a no-op or shows "File not yet reviewed".
- **Existing DB rows** (empty `reviewed_commit`): Fall back to
  `merge_base` until re-reviewed.
- **File list**: The file list still uses `merge_base` for listing changed
  files and change detection via `diff_hash` — this feature only affects
  the displayed diff.
