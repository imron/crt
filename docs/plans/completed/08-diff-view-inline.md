# Stage 8: Inline Diff View and Full-File Mode

## Status: Complete

## Goal

Implement the diff view pane showing a standard unified (inline) diff for
the selected file, with scrolling and color coding. Also implement the
full-file view mode and context-sensitive mode cycling.

## What Was Implemented

### Inline Diff Rendering

1. **Unified diff rendering**: diff displayed in standard unified format:
   - Context lines in neutral color.
   - Added lines prefixed with `+` in green with faint background (`#23303A`).
   - Deleted lines prefixed with `-` in red with faint background (`#34232C`).
   - Hunk boundaries delineated by background color transitions (no
     explicit `@@ ... @@` header lines — deferred to backlog).

2. **Word-level diff highlighting**: paired deletion/addition lines within
   a hunk get word-level diff analysis (token-based LCS). Changed tokens
   are rendered with a brighter emphasis background to make intra-line
   changes immediately visible.

3. **Line numbers**: dual gutters for old and new file numbers. Context
   lines show both. Added lines show only the new number. Deleted lines
   show only the old number.

4. **Scrolling** (when diff pane has focus):
   - `j` / `k` — one line down / up.
   - `Space` / `Ctrl-b` — page down / up.
   - `Ctrl-d` / `Ctrl-u` — half page down / up.
   - `g` / `G` — top / bottom of diff.
   - Mouse scroll wheel.

5. **Hunk navigation**: `Ctrl-i` / `Ctrl-o` jumps to next / previous
   hunk. Works from both the file list and diff pane.

6. **Reviewed file summary**: reviewed files show a summary line with
   timestamp. Pressing `Enter` expands to show the full diff.

7. **Empty states**: no file selected, binary file, file added, file
   deleted — all handled with appropriate messages.

8. **Pane border**: shows file name, mode label `(diff:patience)`,
   whitespace flag `-w`, and hunk position `2/5`. Focus state indicated
   by border color.

9. **Background extends edge-to-edge**: addition/deletion line
   backgrounds are padded with spaces to fill the entire row width.

### Full-File Mode

10. **`s` key**: cycles through Diff → HEAD → Base → Diff. In full-file
    modes, the complete file is displayed with line numbers. Changed lines
    (additions in HEAD view, deletions in base view) are highlighted with
    the same background colors as diff mode.

11. **Scroll position preservation**: switching modes attempts to preserve
    the approximate scroll position.

### Diff Algorithm Selection

12. **`d` key**: cycles through diff algorithms: myers → patience →
    minimal → histogram. The current algorithm is shown in the title bar
    as `(diff:algorithm)`.

13. **Algorithm resolution**: on startup, the algorithm is resolved from:
    crt config (`~/.crt/config.toml`) → git config (`diff.algorithm`) →
    default (patience). The selected algorithm is persisted to crt config.

14. **Histogram support**: histogram is not available in libgit2, so it
    shells out to `git diff --diff-algorithm=histogram` and parses the
    output back via `git2::Diff::from_buffer()`.

### Ignore Whitespace

15. **`w` key**: toggles ignore-whitespace mode (equivalent to `diff -w`).
    The diff pane title shows `-w` when active. The current file's diff
    is reloaded with `ignore_whitespace(true)`.

### Blame Annotations

16. **`b` key**: toggles blame annotations. When visible, a 20-character
    blame column (7-char commit hash + author name) appears to the left
    of line numbers.

17. **Blame for both versions**: in diff mode, deletion lines show blame
    from the base version, addition/context lines show blame from HEAD.
    In full-file modes, blame matches the displayed version.

18. **Blame color**: separate configurable `blame_fg` color
    (`#8C8CA0` default) for readability.

### Configuration

19. **All colors configurable**: every color in the diff view is driven
    by `StyleConfig` loaded from `~/.crt/config.toml`. No hardcoded
    colors in rendering code.

20. **Config color format**: supports `"#RRGGBB"`, named colors
    (`"red"`, `"dark_gray"`), and 256-color indices (`"238"`).

### Mouse Support

21. **Text selection**: click-and-drag selects text. Selection
    automatically skips line-number gutters (and blame columns when
    visible). Selected text is copied to clipboard via OSC 52.

22. **Double-click word selection**: double-clicking selects the word
    under the cursor (reads directly from terminal buffer for accuracy).
    The selected word is copied to clipboard.

23. **Mouse scroll**: scroll wheel scrolls the diff pane.

## Deferred

- Explicit `@@ ... @@` hunk header lines (background transitions
  suffice for v1).
- Long line truncation with `…` visual indicator (ratatui clips
  silently).
- Syntax highlighting (would require `syntect` dependency).
- `f` key was an intentional removal from the original plan.

## Acceptance Criteria

- [x] Selecting a file renders its diff in the diff pane.
- [x] Added, deleted, and context lines are color-coded.
- [x] Hunk boundaries are visually differentiated (via background).
- [x] Word-level changes within lines are highlighted.
- [x] Line numbers are shown in gutters for both old and new files.
- [x] All scrolling keybindings work.
- [x] Hunk navigation (Ctrl-i/o) works from both panes.
- [x] Reviewed files show the summary line; `Enter` expands.
- [x] Binary files, added files, and deleted files display correctly.
- [x] The pane border shows file name, mode, algorithm, and focus state.
- [x] `s` cycles through diff / HEAD / base modes.
- [x] In full-file mode, the complete file is displayed with line numbers.
- [x] `d` cycles diff algorithms (myers/patience/minimal/histogram).
- [x] `w` toggles ignore-whitespace.
- [x] `b` toggles blame annotations with per-line commit/author info.
- [x] Double-click selects words, drag selects text, both auto-copy.
- [x] All colors are configurable via `~/.crt/config.toml`.
