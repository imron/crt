# Stage 13f: Live Updates, Polish, and Full Verification for Comments

## Status: Not Started

## Order

6 of 6

## Depends On

- All prior 13a-13e slices

## Goal

Wire CommentChanged notifications so that comment changes from other clients (including MCP agents) are reflected immediately in the TUI. Complete any remaining polish (help text, status messages, edge cases), and verify that the full acceptance criteria from the stage 13 spec are satisfied.

## Why

The spec requires live updates when another client resolves a comment etc. This slice closes the loop and ensures the whole feature is solid and matches the original requirements.

## Requirements

1. Receipt of a CommentChanged notification causes the affected comment (or the full list) to be refreshed and the UI updated without requiring a full file snapshot reload.

2. Comments loaded at review start and kept up to date across file changes and notifications.

3. All acceptance criteria listed in the parent stage 13 plan are demonstrably passing:
   - Visual selection and creation
   - Input with editor support
   - Persistence and restart survival
   - c toggle, inline blocks, gutter markers
   - Panel listing + collapsed resolved + expand
   - Re-anchoring after changes
   - Orphaned unresolved comments are visible
   - Resolved comments are not re-anchored
   - Resolve/unresolve/edit/delete
   - Next/prev navigation with { / }
   - Live updates from other clients

4. Help text and key documentation are updated.

5. Edge cases (binary files, empty selection, large contexts, unicode, no EDITOR in environment, comments on pure context lines, etc.) are handled gracefully.

6. Tests (unit + interaction + any integration) cover the critical paths.

## Acceptance Criteria

- [ ] Live update from another client immediately affects the current TUI view.
- [ ] Full original stage 13 checklist passes.
- [ ] Help and documentation reflect the new keys and workflows.
- [ ] No regressions in review, cursor, search, or other core flows.

## Implementation Notes

- Use the existing notification plumbing.
- This is the integration and hardening slice; keep changes minimal and focused on completeness.

## Depends On / Enables

- Final slice for stage 13.
- Unblocks any later work that relies on comments (apply markers etc.).

## Progress

- (To be filled)
