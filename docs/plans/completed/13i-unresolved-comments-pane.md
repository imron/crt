# Stage 13i: Unresolved Comments Pane

## Status: Complete

## Order

Future follow-up after the core Stage 13 comment flow is stable.

## Depends On

- 13d (comments visible in model + basic display)
- 13e (comments panel and lifecycle)

## Goal

Add a compact pane for unresolved comments, likely underneath or inside the
Files panel.

## Why

Reviewers need a fast way to scan outstanding feedback without opening the
full comments panel or moving through every file.

## Scope

1. Place unresolved comments as a sub-section inside the Files panel.

2. Show unresolved comments across the current review scope.

3. Selecting an unresolved comment should jump to the owning file and anchored
   line in the diff.

4. Keep this pane focused on unresolved comments. Resolved and historical
   comments remain the responsibility of the main comments panel.

## Acceptance Criteria

- [x] Placement is decided: inside the Files panel.
- [x] Unresolved comments can be scanned without opening the main comments
      panel.
- [x] Selecting an entry navigates to the comment anchor.
- [x] The pane coexists with existing file-list navigation and `Tab` focus
      cycling.
- [x] Comments from files outside the current diff are included.
- [x] `{` and `}` cycle through unresolved comments across files.
- [x] A status message is shown when no unresolved comments exist.

## Implementation Notes

- The unresolved comments section is part of the file panel model and render
  path, not a separate TUI-owned workflow.
- Navigation behavior lives in the App update/navigation layer. The TUI only
  renders the rows and forwards input through core interaction contracts.
- The section uses the shared visible-comment client helper so current-scope
  unresolved comments and previous-base unresolved comments are shown
  consistently with MCP.

## Progress

- Added an `Unresolved Comments` section to the file panel.
- Added unresolved comment counts to file rows.
- Selecting a comment row jumps to its file and anchored line.
- Added section navigation and exact-line jumps when the section is active.
- Added `{` and `}` navigation across unresolved comments, including multiple
  comments with the same start line and comments in later files.
- Added empty-list status feedback.
- Completed and moved to `docs/plans/completed/`.
