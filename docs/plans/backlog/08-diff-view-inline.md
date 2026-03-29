# Stage 8: Inline Diff View and Full-File Mode

## Goal

Implement the diff view pane showing a standard unified (inline) diff for
the selected file, with scrolling and color coding. Also implement the
full-file view mode (toggled with `f`) and the context-sensitive `s` key.

## Why

The diff view is where the actual review happens. The user needs to read
and understand the changes to each file. A clear, well-formatted inline
diff with proper coloring for additions, deletions, and context lines is
the minimum viable rendering for code review. The full-file mode provides
additional context when the diff alone isn't enough to understand the
change.

## Requirements

### Inline Diff Rendering

1. **Unified diff rendering**: display the diff in standard unified diff
   format:
   - Context lines in the default/neutral color.
   - Added lines prefixed with `+` in green.
   - Deleted lines prefixed with `-` in red.
   - Hunk headers (`@@ ... @@`) in a distinct style (e.g. cyan/dimmed).

2. **Line numbers**: display line numbers for both old and new files in
   gutters. Context lines show both. Added lines show only the new
   number. Deleted lines show only the old number.

3. **Scrolling** (when diff pane has focus):
   - `j` / `k` — one line down / up.
   - `Space` — one page down.
   - `Ctrl-d` / `Ctrl-u` — half page down / up.
   - `g` / `G` — top / bottom of diff.

4. **Reviewed file summary**: when the selected file has `Reviewed` status,
   show a summary line instead of the full diff:
   ```
   src/model.rs — reviewed at 2026-03-29T14:30:00+10:00
   ```
   Pressing `Enter` expands to show the full diff.

5. **Empty states**:
   - No file selected: empty pane or help message.
   - Binary file: show "Binary file changed".
   - File added: all lines as additions.
   - File deleted: all lines as deletions.

6. **Pane border**: visible border showing the file name. Focus state
   indicated visually (e.g. different border color).

7. **Long lines**: truncated with a visual indicator for v1.

### Full-File Mode

8. **`f` key**: toggles between diff mode and full-file mode. In full-file
   mode, the pane shows the complete file content (not just changed
   portions).

9. **Default version**: full-file mode initially shows the HEAD version
   of the file.

10. **`s` key (context-sensitive)**: in full-file mode, toggles between
    the HEAD version and the base version of the file. In diff mode,
    `s` is reserved for toggling inline/side-by-side (Stage 10).

11. **Full-file rendering**: in full-file mode, display line numbers and
    the file content. Lines that are part of the diff could optionally be
    highlighted to show what changed, but this is not required for v1.

12. **Scroll position preservation**: switching between diff mode and
    full-file mode should attempt to preserve the approximate scroll
    position (same region of the file visible).

## Acceptance Criteria

- [ ] Selecting a file renders its diff in the diff pane.
- [ ] Added, deleted, and context lines are color-coded.
- [ ] Hunk headers are displayed and visually differentiated.
- [ ] Line numbers are shown in gutters for both old and new files.
- [ ] All scrolling keybindings work.
- [ ] Reviewed files show the summary line; `Enter` expands.
- [ ] Binary files, added files, and deleted files display correctly.
- [ ] The pane border shows the current file name and focus state.
- [ ] `f` toggles between diff and full-file mode.
- [ ] In full-file mode, the complete file is displayed with line numbers.
- [ ] `s` in full-file mode switches between HEAD and base version.
- [ ] Long lines are handled without crashing or layout corruption.

## Open Questions

- In full-file mode, should diff lines be highlighted (e.g. green/red
  background) to indicate what changed? This would be useful but adds
  complexity.
- Should full-file mode support syntax highlighting (via `syntect`)?
  This would be a significant enhancement but adds a dependency.
- Should `f` be available when the file is in reviewed-summary mode,
  or only after expanding the diff?
