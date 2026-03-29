# Stage 12: Polish and Edge Cases

## Goal

Address remaining UX polish, edge cases, and quality-of-life improvements
that make the tool feel complete and reliable.

## Why

The previous stages build all the major features, but a good tool is
defined by how it handles the edges. This stage is about hardening the
tool and smoothing out rough edges.

## Requirements

### Pane Management

1. **`1` toggles file list pane**: hide/show. At least one pane must
   always be visible.

2. **`2` toggles diff pane**: same behavior.

3. **Single-pane focus**: when only one pane is visible, it has focus
   automatically. All relevant keybindings work.

4. **Tab behavior**: switches focus between visible panes. Does nothing
   when only one is visible.

### Diff Search

5. **`/` search**: opens a search prompt at the bottom of the diff pane.
   All matches highlighted, view scrolls to first match.

6. **`n` / `N`**: next / previous match. Wrap around.

7. **Search clearing**: `Escape` or new search clears highlights.

### Edge Cases

8. **Empty diff**: no changes between base and HEAD. Show a message and
   exit gracefully or show empty state.

9. **Very large diffs**: only render visible lines. No freezing.

10. **Very large file lists**: virtualize rendering.

11. **Terminal resize**: layout adapts correctly.

12. **Narrow terminal**: side-by-side degrades gracefully.

13. **Missing base ref**: clear error on startup.

14. **Unicode and wide characters**: no layout corruption.

15. **Detached HEAD**: handled with fallback per Stage 1 decisions.

### Status Bar

16. **Status bar**: persistent bar at the bottom showing:
    - Base ref and HEAD (abbreviated hashes).
    - Review progress (e.g. `7/12 reviewed`).
    - Current view mode (inline / side-by-side / full-file HEAD / base).
    - Current search term (if active).
    - Pane focus indicator.

### Help

17. **`?` or `:help`**: keybinding reference overlay.

## Acceptance Criteria

- [ ] Pane toggling with `1`/`2` works; at least one pane always visible.
- [ ] `Tab` switches focus correctly.
- [ ] `/` opens search, matches highlighted, `n`/`N` navigate matches.
- [ ] Empty diff handled gracefully.
- [ ] Large diffs and file lists don't cause freezing.
- [ ] Terminal resize works without panics.
- [ ] Unicode renders correctly.
- [ ] Status bar shows relevant info.
- [ ] `?` or `:help` shows keybinding reference.

## Open Questions

- Should we support mouse input (click to select file, scroll wheel)?
- What's the minimum supported terminal size?
- Should the status bar be configurable (show/hide certain elements)?
- Should there be a "reload" keybinding to re-read from git without
  restarting?
