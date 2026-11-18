# Stage 16: Hunk-Level Review

## Status: Backlog

## Goal

Allow individual hunks within a file to be reviewed independently. If a
hunk's diff hash hasn't changed since it was approved, it doesn't need
re-review — even if other hunks in the same file have changed.

## Why

Large files with many changes are tedious to re-review after a rebase or
minor edit. If a reviewer has already approved 9 out of 10 hunks, they
should only need to review the one that changed, not re-read the entire
file.

## Key Design Questions

- How are hunk reviews stored? Per-hunk diff hash in the database?
- How are hunks identified across rebases? By content hash, or by
  line range + context?
- Should approved hunks be hidden from the diff view, collapsed, or
  shown with a visual indicator (e.g. dimmed)?
- `A` approves all hunks in the current file at once. A file stays
  `Unreviewed` until all hunks are approved.
- What marker should show a file with some approved hunks and some
  unapproved hunks?
- Plain `a` is reserved for hunk-by-hunk approval when this stage is
  implemented.
- How does this interact with the `Changed` status? If one hunk changes,
  the file is `Changed`, but only that hunk needs re-review.

## Requirements

TBD — needs design discussion before implementation.

## Acceptance Criteria

TBD
