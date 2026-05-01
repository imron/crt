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

### Horizontal Cursor

8. **Column cursor in diff pane**: extend the current line cursor with a
   column position, giving a full row+column cursor in the diff view.

   **State**: `diff_col_cursor: usize` — character offset within the
   content portion of the current line (after gutter and prefix).

   **Navigation**:
   - `h` / `l` (or Left / Right): move cursor left/right by one character.
   - `0`: move to start of line content.
   - `$`: move to end of line content.
   - `w` / `b`: word-forward / word-backward (skip by identifier boundaries).
   - Horizontal scroll is not needed — lines are already rendered within the
     viewport width (wrapped or truncated).

   **Rendering**: highlight the character cell at (line_cursor, col_cursor)
   with an inverted or distinct style (e.g. block cursor). The line cursor
   highlight remains on the full line; the column cursor is a single cell
   overlay on top of it.

   **Integration points**:
   - `Ctrl-]` go-to-definition: extract the word at (line, col) instead of
     just the first word on the line.
   - Visual selection (`v`): start char-wise selection from cursor position.
   - Comment anchoring (Stage 13): use cursor position to specify inline
     comment attachment point.
   - `/` diff search: place cursor on the match position, not just the line.

   **Persistence**: column cursor resets to 0 when the line cursor moves
   (same as vim's default). Optional: `$` makes the cursor "stick" to EOL
   until an explicit horizontal movement.

   **Gutter handling**: the cursor must not enter the line-number gutter or
   the diff prefix column (`+`/`-`/` `). `diff_gutter_cols` provides the
   left boundary. The `0` position maps to the first content character after
   the prefix.

### Edge Cases

9. **Empty diff**: no changes between base and HEAD. Show a message and
   exit gracefully or show empty state.

10. **Very large diffs**: only render visible lines. No freezing.

11. **Very large file lists**: virtualize rendering.

12. **Terminal resize**: layout adapts correctly.

13. **Narrow terminal**: side-by-side degrades gracefully.

14. **Missing base ref**: clear error on startup.

15. **Unicode and wide characters**: no layout corruption.

16. **Detached HEAD**: handled with fallback per Stage 1 decisions.

### Status Bar

17. **Status bar**: persistent bar at the bottom showing:
    - Base ref and HEAD (abbreviated hashes).
    - Review progress (e.g. `7/12 reviewed`).
    - Current view mode (inline / side-by-side / full-file HEAD / base).
    - Current search term (if active).
    - Pane focus indicator.

### Help

18. **`?` or `:help`**: keybinding reference overlay.

## Acceptance Criteria

- [ ] Pane toggling with `1`/`2` works; at least one pane always visible.
- [ ] `Tab` switches focus correctly.
- [ ] `/` opens search, matches highlighted, `n`/`N` navigate matches.
- [ ] Horizontal cursor: `h`/`l` move within a line, `0`/`$` jump to start/end.
- [ ] `Ctrl-]` uses word at (line, col) cursor for go-to-definition.
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
