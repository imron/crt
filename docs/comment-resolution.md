# Comment Resolution Semantics

## Current Problem

Review comments are currently stored with a single `resolved` boolean and
a single `(merge_base, head_ref)` scope. That is not expressive enough for
rebases, independent branches, or merged resolutions.

An unresolved comment is active work, not just historical review state. It
should not disappear only because a branch was rebased and the current
`merge_base` changed. At the same time, resolving a comment on one feature
branch must not make it resolved on a different branch that has not received
the resolving change.

## Example Scenario

Start with a feature branch `A` and unresolved comment `X`.

1. `X` is resolved on `A`.
2. `A` is rebased on `master`, producing `A*`.
3. `X` should still be resolved on `A*` if the equivalent resolving change
   is still in the rebased branch.
4. A different feature branch `B`, also based on `master`, should still see
   `X` as unresolved if it does not contain the resolving change.
5. If `X` is resolved independently on `B`, that creates another valid
   resolution.
6. When `A*` is merged to `master`, `X` is resolved on `master`.
7. When `B` is later rebased on `master`, producing `B*`, `X` should be
   resolved there because the resolution is now in the ancestry.

This implies that comment resolution is not a global boolean. It is a
derived state for a specific review range.

## Decisions So Far

- A comment is an issue attached to code.
- A resolution is an event, not a boolean field on the comment.
- The API can still expose `resolved: bool`, but it should be computed for
  the current review context.
- Resolved historical comments should not be surfaced by default.
- Unresolved comments that apply to the current review should be surfaced,
  even if they were created under an older `merge_base`.
- Re-anchoring and resolution detection should share Git range logic with
  reviewed-file migration where possible.
- A full-file `file_blob_sha` is not a good primary resolution proof. Any
  unrelated change in the same file invalidates it.

## Resolution Evidence

The strongest evidence that a comment is resolved is commit ancestry:

- Store the commit where the comment was marked resolved.
- For a later review, treat the comment as resolved if that resolution commit
  is an ancestor of the current `HEAD`.

This handles normal merges cleanly. It also handles the `B*` case after
`A*` has been merged to `master`, because the resolution commit is then in
the ancestry of `B*`.

Rebases complicate this because the original resolution commit may be
rewritten. A rebased branch may contain an equivalent commit with a different
OID. We need a way to detect that the equivalent resolving change survived
the rebase.

## Rebase Detection Direction

For file reviews, CRT already migrates review state by comparing old review
commits, current `HEAD`, and file blobs. Comment resolution needs similar
range-aware logic, but file blobs are too coarse as the main proof.

We likely need reusable Git helpers that can answer questions like:

- Is commit `A` an ancestor of commit `B`?
- Did a branch rebase from an old head to a new head?
- Is there an equivalent commit in the rebased ancestry?
- Can a specific resolution event be mapped from old history to new history?

Possible evidence for equivalent commits:

- Patch identity / patch-id for the resolving commit.
- Git cherry-pick equivalence, if available through libgit2 or CLI fallback.
- A stored resolution patch or diff summary.
- Anchor-level evidence that the commented code changed in the resolving way.

We have not chosen the final equivalence mechanism yet.

## Squash Merges

Squash merges are harder than rebases because the original resolution commit
does not appear in the target branch ancestry.

Potential approaches:

- Store and compare patch identity for the resolution change.
- Store anchor-level before/after evidence and evaluate it against the
  current tree.
- Treat squash as unresolved unless equivalence can be proven.

The conservative default should be: do not mark a comment resolved unless
there is positive evidence that the resolving change is present.

## Data Model Direction

The current model:

```text
comments.resolved: bool
comments.merge_base
comments.head_ref
```

is not sufficient.

The likely model is:

```text
comments
- id
- creation scope/range
- file_path
- body
- created_at

anchor_versions
- comment_id
- anchor evidence
- range/text/context/status
- created_at

comment_resolution_events
- id
- comment_id
- resolved_at
- resolved_commit
- resolved_head_ref
- resolved_merge_base
- resolved_file_path
- resolution evidence for rebase/squash equivalence
```

The exact columns for resolution equivalence are still undecided. We should
avoid relying primarily on full-file blob hashes.

## Query Semantics

When listing comments for a current review range:

1. Find comments that may apply to the current range.
2. Re-anchor applicable unresolved comments against current content.
3. Determine resolved state from resolution events that apply to the current
   range.
4. Return comments that are unresolved in the current context.
5. Include current-scope resolved comments only when explicitly requested.

`include_resolved=true` should not automatically dump all historical resolved
comments from unrelated ranges.

## Open Questions

- How do we define "comment may apply to this range" without leaking stale
  comments from unrelated branches?
- What is the canonical evidence for equivalent resolution after rebase?
- What is the canonical evidence for equivalent resolution after squash?
- Should unresolve create a separate reopen event rather than deleting a
  resolution event?
- Should check-comments report all unresolved repo comments, or only comments
  applicable to the selected range?
- How should comments on renamed or deleted files be handled?

## Near-Term Implementation Notes

- Do not solve this by widening `list_comments` to all unresolved rows without
  a stronger applicability model.
- Do not make `file_blob_sha` the primary resolution mechanism.
- Add shared Git range helpers before implementing comment resolution
  migration.
- Keep resolution derivation conservative: unresolved is preferable to
  incorrectly hiding an active comment.
