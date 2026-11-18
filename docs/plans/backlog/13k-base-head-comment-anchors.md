# Stage 13k: Compound Comment Anchors

## Status: Planned

## Order

Implement before 13j check commands.

## Depends On

- 13b (comment creation)
- 13c (comment persistence and re-anchoring)
- 13d (gutter display)
- 13e (comment lifecycle)

## Goal

Represent every review comment as a compound anchor with `base` and `head`
side slots. Each side slot contains an anchor segment when the selected content
exists on that side. This supports comments on added lines, deleted lines,
unchanged context, and replacement ranges with one consistent model.
Also record exact creation-head provenance so later check commands can ask
whether unresolved comments still apply to a target head commit.

## Why

The current comment model treats every anchor as one HEAD line range. That
works for comments on added or unchanged new-file lines, but it cannot express
feedback on deleted lines such as "why was this removed?". It also cannot
represent a replacement comment that naturally spans the deleted base text and
the added head text.

The earlier `base` / `head` / `both` anchor-side idea still special-cases
replacement ranges. A compound anchor avoids that: each comment owns an anchor
version, and that version owns up to one base segment and one head segment.
Both slots are populated whenever the selected content exists on both sides.
Only added-only and deleted-only selections naturally have one absent side.

## Core Model

A comment has an append-only sequence of anchor versions. Each anchor version
has two side slots:

```text
comment
  anchor_version
    base_segment: optional segment(line range, text, context)
    head_segment: optional segment(line range, text, context)
```

At least one side slot must be populated. Both slots are populated for context,
replacement, and any selection that includes selected content on both sides.

Segment shapes:

- added line comment: one `head` segment,
- deleted line comment: one `base` segment,
- unchanged context comment: one `base` segment plus one `head` segment,
- replacement comment: one `base` segment plus one `head` segment.

The difference between unchanged context and replacement is the segment
content and diff mapping, not a separate comment type.

## Planning Decisions

1. Anchor sides belong to segments, not comments.

   Segment side values are exactly:

   - `base`: old-side file content,
   - `head`: new-side file content.

2. Context-line comments store both segments.

   Context code exists on both sides of the review range. Store a `base`
   segment and a `head` segment. In unified diff display those segments resolve
   to the same context row; in full-file views each side has the information it
   needs without special handling.

3. Replacement selections store both segments.

   A selection containing deleted and added rows is a valid compound anchor.
   The base segment captures the deleted rows and the head segment captures the
   added rows. There is no single `line_start..line_end` coordinate space for
   the whole comment, so the shared anchor object is the compound version, not
   a single line range.

4. Selections are normalized into base/head side slots.

   For any selected visual range, collect the selected base-side lines into a
   base segment and the selected head-side lines into a head segment. Side
   slots with no selected lines remain absent. This means context plus
   addition, context plus deletion, and replacement selections are all
   represented by the same compound model without special casing.

   An absent side slot occurs naturally when the selected range has no content
   on that side. For example, an added-only selection has no base lines, and a
   deleted-only selection has no head lines.

5. Re-anchoring runs per segment.

   Each segment re-anchors against content for its own side. A comment may be
   fully anchored, partially anchored, or fully orphaned depending on the
   segment results.

   A successful re-anchor produces an `anchored` placement at the new location,
   regardless of how the location was found. The matcher records the method
   separately for confidence and debugging.

6. Use a development schema reset.

   The project is still in development. The implementation should delete
   existing comment, anchor, and resolution rows during the schema change and
   start fresh with the compound-anchor model.

## Data Model

### Comments

Add exact provenance to `comments`:

- `created_head_commit TEXT`

`merge_base` remains the creation/review merge-base scope key. It is also the
base commit used for created-base anchoring and 13j ancestry checks. Do not add
a duplicate `created_merge_base` column.

For new comments:

- `created_head_commit` is `ctx.head.resolved_commit()`,
- `merge_base` is `ctx.merge_base_key()`.

Existing comment data can be discarded during this migration. Do not add
fallback paths for old comment rows.

### Anchor Versions

Keep `anchor_versions` as the append-only parent record:

- `id INTEGER PRIMARY KEY`
- `comment_id INTEGER NOT NULL`
- `aggregate_status TEXT NOT NULL`
- `created_at TEXT NOT NULL`

`aggregate_status` is derived from segment placement statuses:

- `anchored`: all populated segments are anchored,
- `partial`: at least one segment is anchored and at least one segment is
  orphaned,
- `orphaned`: all segments are orphaned.

This adds `partial` to the current anchor status vocabulary.

### Anchor Segments

Add an `anchor_segments` table:

- `id INTEGER PRIMARY KEY`
- `anchor_version_id INTEGER NOT NULL`
- `side TEXT NOT NULL`
- `file_path TEXT NOT NULL`
- `file_blob_sha TEXT NOT NULL`
- `line_start INTEGER NOT NULL`
- `line_end INTEGER NOT NULL`
- `char_start INTEGER`
- `char_end INTEGER`
- `anchor_text TEXT NOT NULL`
- `context_before TEXT NOT NULL`
- `context_after TEXT NOT NULL`
- `placement_status TEXT NOT NULL`
- `match_method TEXT NOT NULL`
- `created_at TEXT NOT NULL`

Rules:

- `side` is `base` or `head`.
- `line_start` and `line_end` are in the coordinate space named by `side`.
- `char_start` and `char_end` are relative to the segment text.
- `file_blob_sha` records the side content actually used for the segment
  match.
- `placement_status` is `anchored` when the segment has a current line range
  and `orphaned` when it does not.
- `match_method` records how the latest placement was found:
  `exact_at_line`, `exact_elsewhere`, `context`, or `not_found`.
- Both side slots for one anchor version belong to the same comment anchor.

### Resolution Events

Resolution snapshots should store the resolved compound anchor, not just one
line range. Prefer a `comment_resolution_anchor_versions` relation, or copy the
same segment shape into resolution-specific segment rows.

Keep:

- `resolved_commit`
- `resolved_head_ref`
- `resolved_merge_base`
- `resolved_patch_id`
- resolution-time compound anchor segment snapshots

Resolution state remains derived from events. The stored resolution anchor is
evidence and audit history; it is not a mutable comment flag.

Resolution equivalence should be part of 13k, not deferred. Resolution checks
use evidence in this order:

1. `resolved_commit` is an ancestor of the target head.
2. A commit in the target history has the same stable patch-id as the
   resolution event.
3. Resolution-time anchor evidence proves the same resolved content state is
   present in the target tree.

If none of those checks succeeds, the comment remains unresolved.

## Protocol Model

Add shared types:

```rust
#[serde(rename_all = "snake_case")]
enum CommentAnchorSide {
    Base,
    Head,
}

struct CommentAnchorSegment {
    side: CommentAnchorSide,
    file_path: String,
    line_start: i64,
    line_end: i64,
    char_start: Option<i64>,
    char_end: Option<i64>,
    anchor_text: String,
    context_before: String,
    context_after: String,
    placement_status: AnchorPlacementStatus,
    match_method: AnchorMatchMethod,
}

struct CommentAnchor {
    segments: Vec<CommentAnchorSegment>,
    aggregate_status: AnchorAggregateStatus,
}
```

`AnchorPlacementStatus` is `anchored` or `orphaned`.
`AnchorMatchMethod` is `exact_at_line`, `exact_elsewhere`, `context`, or
`not_found`.
`AnchorAggregateStatus` is `anchored`, `partial`, or `orphaned`.

Extend `CreateCommentParams` to accept a `CommentAnchor`.

Extend `Comment` with `anchor: CommentAnchor`. Remove the old top-level anchor
fields from the protocol as part of the same change, and update TUI, MCP, and
tests to consume the compound anchor directly.

## Creation Semantics

The App owns comment behavior and derives anchor segments from the current diff
row or selection. The TUI only forwards input and renders the resulting model.

Unified diff:

- added rows -> one `head` segment,
- deleted rows -> one `base` segment,
- context rows -> `base` and `head` segments,
- context plus addition -> `base` and `head` segments,
- context plus deletion -> `base` and `head` segments,
- deletion plus addition replacement -> `base` and `head` segments.

Side-by-side diff:

- left-only/deleted rows -> one `base` segment,
- right-only/added rows -> one `head` segment,
- aligned context rows -> `base` and `head` segments,
- selected ranges spanning both columns -> `base` and `head` segments.

Full-file modes:

- full-file base -> one `base` segment,
- full-file head -> one `head` segment.

If a selection produces no selected lines on a side, that side slot is absent.
Reject only selections that produce neither a base nor a head segment.

## Re-Anchoring Semantics

Re-anchor each segment independently using a four-step matcher:

1. exact anchor text at stored line -> `anchored`, `exact_at_line`,
2. anchor text elsewhere in side content -> `anchored`, `exact_elsewhere`,
3. context-assisted match -> `anchored`, `context`,
4. no match -> `orphaned`, `not_found`.

`exact_elsewhere` and `context` are match methods, not persistent placement
states. Once the server chooses a new line range, the segment is anchored at
that range. Consumers can still use `match_method` to show confidence or debug
why a segment moved.

Target content by mode:

- current TUI/session display:
  - `head` segments re-anchor against current head/worktree content,
  - `base` segments re-anchor against the session merge-base content.
- `crt check HEAD` / MCP head mode:
  - `head` segments re-anchor against the target head commit,
  - `base` segments re-anchor against the comment's `merge_base`.
- explicit range mode:
  - `head` segments re-anchor against the range head,
  - `base` segments re-anchor against the range base.

After all segments are evaluated, insert one new `anchor_versions` row and its
segments. Do not update the previous version in place.

Partial re-anchor is valid. For example, a replacement comment may still find
the head segment while the original deleted base segment is gone. The comment
remains unresolved unless a resolution event applies.

Do not mark a comment resolved because re-anchoring fails. Orphaned and
partially orphaned unresolved comments remain visible and count as unresolved
for 13j.

## Display Semantics

Rendering consumes the comment-level compound anchor:

- unified diff shows one marker spanning the full visual range covered by the
  base and head segments,
- side-by-side diff shows one marker spanning the full visual range covered by
  the base and head segments across both columns,
- context comments naturally collapse to a single-row marker because both
  sides resolve to the same visual row,
- replacement comments show one continuous marker spanning the deleted and
  added rows,
- full-file base shows comments with base segments,
- full-file head shows comments with head segments.

The comments panel displays the comment body once. Segments are storage and
navigation data, not separate comments.

The unresolved comments file-panel section still lists one row per comment.
Navigation should prefer the segment visible in the current render mode. If no
segment is visible in the current render mode, keep the current view unchanged
and flash a status message explaining that the comment is not visible in this
view. Let the user change render modes explicitly.

## Applicability And Resolution

13j needs to ask whether unresolved comments apply to a target head commit.
13k provides the storage and helper semantics for that question.

For head-mode checks:

1. Resolve the target head to an exact commit.
2. Consider unresolved comments with a non-empty `created_head_commit` that is
   an ancestor of the target head.
3. Treat a comment as resolved if resolution evidence applies to the target
   head:
   - resolution commit ancestry,
   - stable patch-id match in target history,
   - resolution-time anchor evidence in target content.
4. If no positive resolution evidence exists, report the comment.

For range-mode checks:

1. Resolve `BASE` and `HEAD`.
2. Compute `merge_base(BASE, HEAD)`.
3. Include exact range comments.
4. Include unresolved carry-over comments whose `created_head_commit` is an
   ancestor of the range head.
5. Re-anchor comments in the explicit range context.
6. Use the same resolution evidence checks against the range head.

This is intentionally conservative. Even with patch-id and anchor evidence,
13k should not hide comments without positive evidence.

## UI Contract

1. Unified diff:

   - deletion rows can receive base segments,
   - addition rows receive head segments,
   - context rows create paired base/head segments,
   - replacement selections create paired base/head segments.

2. Side-by-side diff:

   - left column creates base segments,
   - right column creates head segments,
   - aligned context rows create paired base/head segments.

3. Full-file base mode:

   - show comments with base segments,
   - hide comments that have only head segments.

4. Full-file head mode:

   - show comments with head segments,
   - hide comments that have only base segments.

5. Comments panel and unresolved file-panel section:

   - keep one logical row per comment,
   - navigate to an appropriate visible segment,
   - keep orphaned and partial unresolved comments visible.

## Test Plan

The detailed test matrix lives in `13k-tests.md`. Keep the implementation
slices and acceptance criteria below aligned with that test plan.

## Implementation Slices

1. Add protocol structs for compound anchors and segments.
2. Add database migration that deletes existing comment, anchor, and
   resolution rows, then creates compound-anchor storage.
3. Add the full `13k-tests.md` test suite as intentionally failing red tests.
   At this point the structs and database shape exist, but behavior is not
   implemented. Do not skip or soften assertions to make this slice green.
4. Update server create/list/get/resolve paths.
5. Update re-anchoring to operate per segment, placement status, match method,
   and aggregate status.
6. Update App anchor capture for unified, side-by-side, and full-file modes.
7. Update marker models and rendering to consume segments.
8. Add shared applicability/re-anchor helper needed by 13j.
9. Turn the remaining red tests green without weakening the planned coverage.

## Implementation Notes

- Use Rust enums for all in-memory state values:
  - segment side,
  - segment placement status,
  - segment match method,
  - aggregate anchor status.
- Database columns may remain `TEXT`, but all reads and writes must map through
  enum conversion helpers. Do not pass string literals through app, server, TUI,
  or MCP logic.
- Keep navigation behavior in the App layer. The TUI renders the model and
  forwards input; it does not decide whether to switch views or show status
  messages.

## Acceptance Criteria

- [ ] A user can create a comment on a deleted line.
- [ ] Added-line comments are stored as one `head` segment.
- [ ] Deleted-line comments are stored as one `base` segment.
- [ ] Context-line comments are stored as paired `base` and `head` segments.
- [ ] Context plus addition selections are stored as paired `base` and `head`
      segments.
- [ ] Context plus deletion selections are stored as paired `base` and `head`
      segments.
- [ ] Replacement selections are stored as paired `base` and `head` segments.
- [ ] `list_comments` returns the compound anchor for TUI and MCP clients.
- [ ] Re-anchoring evaluates each segment against its own side content.
- [ ] Successful re-anchors store `placement_status = anchored` and the
      appropriate `match_method`.
- [ ] Replacement comments can become partially anchored without being hidden.
- [ ] Full-file base mode shows comments with base segments only.
- [ ] Full-file head mode shows comments with head segments only.
- [ ] Navigating to a comment that is hidden in the current render mode keeps
      the render mode unchanged and flashes a status message.
- [ ] Runtime code uses Rust enums for anchor side, placement status, match
      method, and aggregate status.
- [ ] Existing development comment rows are deleted during the schema reset.
- [ ] New comments record exact creation-head provenance.
- [ ] Resolution derivation uses commit ancestry, patch-id equivalence, and
      resolution-time anchor evidence for a target head.
- [ ] Orphaned and partial unresolved comments remain visible and count as
      unresolved.

## Open Follow-Ups

- Rename-aware comment applicability.
- Multi-segment anchors beyond one base slot plus one head slot.

## Progress

- Planning questions answered with a compound-anchor model.
