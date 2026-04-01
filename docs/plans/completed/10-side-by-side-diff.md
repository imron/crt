# Stage 10: Side-by-Side Diff View

## Goal

Add a side-by-side diff rendering mode as an alternative to the inline
(unified) view, toggled with `s` when in diff mode.

## Why

Side-by-side diffs are often easier to read for complex changes, especially
when lines are modified rather than purely added or deleted. Having both
modes available lets the user pick whichever is clearer for the file
they're currently reviewing.

## Requirements

1. **Side-by-side layout**: split the diff pane into two equal columns:
   - Left column: old file content (base version).
   - Right column: new file content (HEAD version).
   - A vertical divider between the two columns.

2. **Line alignment**: corresponding lines aligned:
   - Context lines appear on both sides at the same vertical position.
   - Deleted lines on the left with a filler on the right.
   - Added lines on the right with a filler on the left.
   - Modified lines (deletion followed by addition) paired side by side.

3. **Color coding**: same semantics as inline view:
   - Additions in green (right column).
   - Deletions in red (left column).
   - Context in default color (both columns).

4. **Inline change highlighting**: within a modified line pair, highlight
   the specific characters that changed. This makes it much easier to
   spot small changes in long lines.

5. **Line numbers**: each column has its own line number gutter.

6. **Toggle**: `s` in diff mode switches between inline and side-by-side.
   - Preserves approximate scroll position.
   - Works from either pane (global keybinding).
   - Indicated in the UI (mode indicator in border or status bar).

7. **Scrolling**: all scroll keybindings work in side-by-side mode. Both
   columns scroll together (synchronized).

8. **Narrow terminals**: when the terminal is too narrow for a readable
   side-by-side view, degrade gracefully (fall back to inline or
   truncate with indicator).

## Acceptance Criteria

- [ ] `s` in diff mode toggles between inline and side-by-side views.
- [ ] Side-by-side shows old on left, new on right, with divider.
- [ ] Lines are aligned correctly across the two columns.
- [ ] Modified line pairs show character-level change highlighting.
- [ ] Line numbers in both columns.
- [ ] Scrolling works identically to inline mode, both columns synced.
- [ ] Toggle preserves approximate scroll position.
- [ ] Current view mode is indicated in the UI.
- [ ] Narrow terminals do not cause panics or garbled rendering.

## Open Questions

- What's the minimum terminal width for side-by-side? 80 columns? 120?
- Should the column width split be configurable (e.g. 50/50 vs. 40/60)?
- Should word-wrap be supported in side-by-side, or only truncation?
