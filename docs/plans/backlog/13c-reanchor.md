# Stage 13c: Server Re-Anchoring for Unresolved Comments

## Status: Not Started

## Order

1 of 6 (can run in parallel with 13a)

## Depends On

- Existing comment DB schema, StoredComment, list/get handlers, review_types AnchorStatus and Comment

## Goal

Implement the re-anchoring logic on the server so that when unresolved comments are loaded they are matched against the current file content and given the correct AnchorStatus (Anchored, Shifted, Approximate, or Orphaned) per the four-step process defined in the stage 13 spec. Resolved comments must never be re-anchored.

## Why

Comments must survive rebases and other changes to the file. The spec requires only unresolved comments to be re-anchored; resolved ones are intentionally left pointing at their historical location. This is a core requirement for both TUI display and MCP consumers.

## Requirements

1. When serving list_comments or get_comment (and similar), for every unresolved comment the server performs anchor resolution against the current version of the file.

2. The four steps are applied in order:
   - Exact match of anchor_text at the stored line number.
   - Anchor text found elsewhere in the file (shifted).
   - Context before/after provides an approximate location.
   - No usable match (orphaned).

3. The returned Comment objects carry the appropriate AnchorStatus and (where possible) updated line/character ranges.

4. Resolved comments bypass re-anchoring entirely and retain their stored location and "resolved" status.

5. Re-anchoring must not silently lose comments; orphaned unresolved comments must still be returned with Orphaned status and their original context.

6. Add or extend tests that exercise the matcher with exact, shifted, approximate, and orphaned cases.

7. The logic lives on the server (as specified) and is exercised for TUI and MCP paths alike.

## Acceptance Criteria

- [ ] Unresolved comments receive correct non-Always-Anchored status after changes to the file.
- [ ] Resolved comments are never re-anchored and produce no warnings.
- [ ] Orphaned unresolved comments are still returned (visible in panel later).
- [ ] list_comments and get_comment both apply the logic.
- [ ] Existing comment CRUD and notification behavior is unchanged.

## Implementation Notes

- The spec describes the steps; the implementing agent decides the exact matching algorithm (substring, normalized, context windows, etc.) as long as the four cases are covered.
- File content for the current HEAD/worktree is already accessible via existing git helpers.

## Depends On / Enables

- Enables 13d (display needs real anchor status for gutter and blocks).
- Independent of UI slices.

## Progress

- (To be filled)
