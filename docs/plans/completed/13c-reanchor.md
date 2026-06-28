# Stage 13c: Server Re-Anchoring for Unresolved Comments

## Status: Complete

## Order

1 of 6 (can run in parallel with 13a)

## Depends On

- Existing comment CRUD handlers (`create_comment`, `list_comments`,
  `get_comment`) and the `review_types` definitions for `Comment` and
  `AnchorStatus`.

## Goal

Implement the re-anchoring logic on the server using an append-only
`anchor_versions` table (recording `file_blob_sha` + fresh
ranges/text/contexts/status) plus the `v_current_anchors` view. When
unresolved comments are loaded they are matched against the current
file content and a new version row is inserted so the view yields the
correct AnchorStatus (Anchored, Shifted, Approximate, or Orphaned).
Resolved comments must never be re-anchored.

## Why

Comments must survive rebases and other changes to the file. The spec
requires only unresolved comments to be re-anchored; resolved ones are
intentionally left pointing at their historical location. This is a core
requirement for both TUI display and MCP consumers.

## Anchor Storage Model

Comments use three tables plus a view:

- `comments`: stable logical comment (id, scope, file_path, body,
  timestamps).
- `anchor_versions`: append-only. Each row captures a specific
  attachment at a point in time:
  `file_blob_sha`, line/char ranges, `anchor_text`,
  `context_before`/`context_after`, and the `AnchorStatus` at that
  time.
- `comment_resolution_events`: append-only resolution records. A
  comment is treated as resolved when the current resolution query finds
  a matching event.
- `v_current_anchors`: a view that selects the latest version per
  comment by `created_at`, using `id` as a tie-breaker when versions
  share the same timestamp.

Creation records the first `anchor_versions` row using the exact
`file_blob_sha` the selection was made against.

Re-anchoring (unresolved comments only) computes against current file
content and `INSERT`s a new version row with fresh context; the view
automatically reflects the latest. Resolved comments are never
re-anchored; their last version remains the historical record.

### Schema (point form)

- `comments` table
  - `id` (PK, AUTOINCREMENT)
  - `merge_base`, `head_ref`
  - `file_path`
  - `body`
  - `created_at`, `updated_at`

- `anchor_versions` table
  - `id` (PK, AUTOINCREMENT)
  - `comment_id` (FK → `comments.id`)
  - `file_blob_sha`
  - `line_start`, `line_end`, `char_start`, `char_end`
  - `anchor_text`
  - `context_before`, `context_after`
  - `status` (Anchored / Shifted / Approximate / Orphaned)
  - `created_at`

- `comment_resolution_events` table
  - `id` (PK, AUTOINCREMENT)
  - `comment_id` (FK -> `comments.id`)
  - `resolved_at`
  - `resolved_commit`, `resolved_head_ref`, `resolved_merge_base`
  - anchor snapshot fields for the resolved location

- Index
  - Covering index on `anchor_versions(comment_id, created_at, id)`
  - Covering index on
    `comment_resolution_events(comment_id, resolved_at, id)`

## Design Decisions

- Append-only `anchor_versions` gives an immutable history of every
  attachment point. No row is ever mutated, so the original creation
  anchors and any subsequent re-anchors are always available for
  audit or future re-anchoring.
- `file_blob_sha` (instead of commit SHA) records the exact file
  content the anchor was derived from. This is more precise and
  simplifies the matching algorithm.
- The `v_current_anchors` view provides a stable, read-only projection
  of the latest version per comment. Using a view keeps the base tables
  simple and avoids a mutable `is_current` flag.
- Only unresolved comments receive fresh context on re-anchor. This
  guarantees that future re-anchoring runs start from the most
  recent known-good text, avoiding drift from stale original
  context. Resolved comments keep their historical snapshot forever.
- Resolution is not stored as a mutable boolean on `comments`. It is
  derived from `comment_resolution_events` so later work can make the
  query range-aware across rebases and branch histories.
- The four-step matcher and the decision to persist a new version on
  success (Anchored/Shifted/Approximate) are the only server-side
  logic required; the TUI and MCP layers simply read from the view.

## Requirements

1. When serving list_comments or get_comment (and similar), for every
   unresolved comment the server performs anchor resolution against the
   current version of the file and inserts a new row into
   `anchor_versions` (with fresh ranges, `anchor_text`, contexts,
   `file_blob_sha`, and status). The `v_current_anchors` view then
   exposes the latest version.

2. The four steps are applied in order:
   - Exact match of anchor_text at the stored line number.
   - Anchor text found elsewhere in the file (shifted).
   - Context before/after provides an approximate location.
   - No usable match (orphaned).

3. The returned Comment objects carry the appropriate AnchorStatus and
   (where possible) updated line/character ranges.

4. Comments treated as resolved by the resolution-event query bypass
   re-anchoring entirely and retain their stored location and resolved
   status.

5. Re-anchoring must not silently lose comments; orphaned unresolved
   comments must still be returned with Orphaned status and their
   last-known context (from the most recent anchor version that existed
   before this re-anchor attempt, which may be the original creation
   version if no successful re-anchor has occurred previously).

6. Add or extend tests that exercise the matcher with exact, shifted,
   approximate, and orphaned cases.

7. The logic lives on the server (as specified) and is exercised for
   TUI and MCP paths alike.

## Acceptance Criteria

- [x] Unresolved comments receive correct non-Always-Anchored status
      after changes to the file.
- [x] Resolved comments are never re-anchored and produce no warnings.
- [x] Orphaned unresolved comments are still returned (visible in panel later).
- [x] list_comments and get_comment both apply the logic.
- [x] Existing comment CRUD and notification behavior is unchanged.

## Implementation Notes

- The spec describes the steps; the implementing agent decides the
  exact matching algorithm (substring, normalized, context windows, etc.)
  as long as the four cases are covered.
- Use the append-only `anchor_versions` table: always INSERT a new
  row for unresolved comments on re-anchor (never UPDATE existing
  rows). The `v_current_anchors` view provides the latest anchor.
- Record `file_blob_sha` of the exact file content used for the match
  (creation uses the selection-time blob; re-anchors use current
  content).
- File content for the current HEAD/worktree (and blob SHA helpers)
  is already accessible via existing git helpers.
- Resolved comments must bypass all re-anchoring and version insertion.
- Re-anchoring logic operates exclusively on source file content and
  stored anchor data. It is independent of visual row mapping
  (including the `Shift-D` double-spacing debug mode from plan 17).
- The `anchor_versions` table stores only anchor data; it does not
  carry `LineType` or presentation metadata. `LineType` lives in the
  presentation layer via the `LineView` abstraction (plan 17).

## Depends On / Enables

- Enables 13d (display needs real anchor status for gutter and blocks).
- Independent of UI slices.

## Progress

- Reworked comment storage into stable `comments` records plus
  append-only `anchor_versions`.
- Added the `v_current_anchors` latest-anchor projection and migrated
  old inline-anchor comment tables when detected.
- Added `comment_resolution_events`, migrated old resolved flags into
  event rows, and removed `comments.resolved` from the current schema.
- Added versioned SQL migrations under `migrations/`, included by
  `src/db.rs`, with legacy schema bootstrapping for existing DBs.
- Creation now writes the initial anchor version with a worktree
  content hash.
- `list_comments` and `get_comment` now re-anchor unresolved comments
  before returning them; resolved comments bypass the matcher.
- Added matcher coverage for exact, shifted, approximate, and orphaned
  outcomes.
- Verified with `cargo check` and `cargo test`.
