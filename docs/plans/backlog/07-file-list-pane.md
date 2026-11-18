# Stage 7: File List Pane

## Goal

Implement the file list widget with the split unreviewed/reviewed layout,
navigation, and file selection.

## Why

The file list is the primary navigation interface. It's how the user
understands the scope of the review, tracks progress, and selects files
to examine. The split layout (unreviewed on top, reviewed on bottom) is a
key UX decision — it makes it immediately visible how much work remains
and provides a natural workflow: work through the top section, watch files
move to the bottom as they're reviewed.

## Requirements

1. **Split layout**: the file list is divided into two sections with
   visible headers:
   - **Unreviewed (N)**: files with status `Unreviewed` or `Changed`.
     Count N is the number of files in this section.
   - **Reviewed (N)**: files with status `Reviewed`. Count N is the
     number of files in this section.

2. **Status markers**: each file displays a status icon:
   - `✗` for `Unreviewed`.
   - `~` for `Changed` (previously reviewed, diff changed).
   - `✓` for `Reviewed`.

3. **File path display**: show the file path relative to the repo root.
   For renamed files, show both old and new paths.

4. **Sorting**: files within each section are sorted alphabetically.

5. **Cursor navigation** (when file list pane has focus):
   - `j` moves the cursor down.
   - `k` moves the cursor up.
   - Both sections feel like one continuous list.
   - `g` jumps to the first file.
   - `G` jumps to the last file.

6. **Selection**: the currently selected file is visually highlighted.
   When the selection changes, the diff pane updates to show the diff
   for the newly selected file (requested from the server).

7. **Scrolling**: if the file list is longer than the pane height, scroll
   to keep the cursor visible.

8. **`Ctrl-n` / `Ctrl-p`**: navigate to the next/previous file. These
   work globally (regardless of pane focus). Cycle through unreviewed
   files first, then reviewed files.

9. **`Enter`**: select the file and move focus to the diff pane.

10. **Initial selection**: on startup, the first unreviewed file is
    selected. If all files are reviewed, select the first file.

## Acceptance Criteria

- [ ] The file list renders with two separated sections, each with a
      header showing the section name and file count.
- [ ] Files show the correct status marker based on their review state.
- [ ] `j`/`k` navigation works, scrolling when necessary.
- [ ] `g`/`G` jump to the first/last file.
- [ ] Selecting a different file updates the diff pane.
- [ ] `Ctrl-n`/`Ctrl-p` navigate files regardless of pane focus, cycling
      unreviewed first.
- [ ] `Enter` moves focus to the diff pane.
- [ ] On startup, the first unreviewed file is selected.
- [ ] The currently selected file is visually distinct (highlighted).
- [ ] Renamed files show both old and new paths.

## Open Questions

- Should files be grouped by directory with collapsible sections, or kept
  as a flat alphabetical list?
- Should the file list show file size or diff size (e.g. "+42 -10")?
- When there are many files (100+), should we support filtering/searching
  within the file list?
