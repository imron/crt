# Stage 13k: Base and Head Comment Anchors

## Status: Placeholder

## Order

Future follow-up after the current HEAD-only comment flow is stable.

## Depends On

- 13b (comment creation)
- 13c (comment persistence and re-anchoring)
- 13d (gutter display)
- 13e (comment lifecycle)

## Goal

Support review comments on deleted lines, added lines, and unchanged context
lines by modelling whether each comment anchor belongs to the base side, the
head side, or both sides of a diff.

## Why

The current comment model treats anchors as HEAD lines. That works for
comments on added or unchanged new-file lines, but it cannot express feedback
on deleted lines such as "why was this removed?". Side-by-side and inline diff
views both need a way to attach comments to the base side without pretending
the line exists in HEAD.

## Planning Questions

1. What exact anchor side values do we need?

   - `head`: added lines and new-file context.
   - `base`: deleted lines and old-file context.
   - `both`: unchanged context where old and new line numbers map cleanly.

2. Should a context-line comment default to `head`, `base`, or `both`?

   The likely default is `both`, because context comments are about code that
   exists on both sides. The UI can still choose a display side based on the
   current render mode.

3. How should range anchors behave when a selection crosses additions and
   deletions?

   Mixed-side ranges probably need to be rejected at first, or split into
   separate anchors, because one `line_start..line_end` cannot represent a
   coherent range across unrelated base and head line spaces.

4. How does re-anchoring work for base-side comments?

   Base-side comments should re-anchor against base content while they remain
   tied to the current review range. Once the deleted code no longer exists in
   a rebased base, the comment should become orphaned rather than being
   incorrectly attached to a HEAD line.

## Candidate Data Model

Do not implement this without a schema review first. The likely shape is:

- Add an `anchor_side` field to the current anchor version records.
- Store `line_start` and `line_end` in the coordinate space named by
  `anchor_side`.
- For `both`, store enough information to reconstruct both base and head
  line ranges, or require the server to map the range at read time from the
  diff hunk.
- Keep the stable comment record side-agnostic; side belongs to each anchor
  version.

## UI Contract

1. Inline diff:

   - deletion rows can receive base-side comments,
   - addition rows receive head-side comments,
   - context rows default to both-side comments.

2. Side-by-side diff:

   - left column can receive base-side comments,
   - right column can receive head-side comments,
   - context rows can show the same both-side comment on both columns or use a
     single shared marker, depending on the final visual design.

3. Full-file base mode:

   - base-side comments are visible,
   - head-only comments are hidden.

4. Full-file head mode:

   - head-side comments are visible,
   - base-only comments are hidden.

## Acceptance Criteria

- A user can create a comment on a deleted line.
- `list_comments` returns enough side information for UIs and MCP clients to
  explain where the comment is attached.
- Re-anchoring does not move base-side comments onto unrelated HEAD lines.
- Gutter markers render on the appropriate side in inline, side-by-side,
  full-file base, and full-file head modes.
- Mixed base/head selections have explicit behaviour and tests.
- Existing HEAD-only comments migrate deterministically to `head` or `both`
  according to the selected compatibility rule.
