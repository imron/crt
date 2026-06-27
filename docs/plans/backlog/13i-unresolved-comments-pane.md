# Stage 13i: Unresolved Comments Pane

## Status: Placeholder

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

## Placeholder Scope

1. Decide whether unresolved comments should be a sub-section inside the
   Files panel or a separate pane beneath it.

2. Show unresolved comments across the current review scope.

3. Selecting an unresolved comment should jump to the owning file and anchored
   line in the diff.

4. Keep this pane focused on unresolved comments. Resolved and historical
   comments remain the responsibility of the main comments panel.

## Acceptance Criteria

- [ ] Placement is decided: inside Files panel or underneath it.
- [ ] Unresolved comments can be scanned without opening the main comments
      panel.
- [ ] Selecting an entry navigates to the comment anchor.
- [ ] The pane coexists with existing file-list navigation and `Tab` focus
      cycling.

## Implementation Notes

- This is intentionally a placeholder. Do not implement it as part of 13b.
- Reuse the app model and server comment list contracts from the core comment
  plans.

## Progress

- Placeholder created.
