# Stage 13: Comments

## Goal

Add the ability to create, view, edit, and resolve review comments attached
to specific lines or character ranges within diffs. Comments are stored via
the server in SQLite with content-based anchoring for rebase resilience.

## Why

The primary consumers of review comments are LLM coding agents. When
reviewing code an agent has written, the reviewer needs a way to provide
structured feedback tied to specific lines of code. Comments must survive
rebases and must be queryable so the agent knows what to fix.

This stage builds the data model, the TUI interaction for creating and
viewing comments, and the anchor resolution system. The next stages add
the agent-facing interfaces (MCP adapter and file markers).

## Implementation Plans

This stage is broken into focused sub-plans. Implement in recommended order:

1. 13a-selection-anchor-capture.md
2. 13b-composer-create.md
3. 13c-reanchor.md (can be parallel with 13a)
4. 13d-inline-gutter-display.md
5. 13e-panel-lifecycle.md
6. 13f-live-polish-tests.md

See the individual sub-plan files under docs/plans/backlog/ for detailed
requirements, acceptance criteria, and notes for each slice.

Keybinding decision (recorded here):
- Use plain `}` for next comment and `{` for previous comment in the
  current file's diff.

## Requirements

### Selection and Comment Creation

1. **`V` (line selection)**: in the diff pane, press `V` to enter line
   selection mode. The current line is selected. `j`/`k` extend the
   selection. Selected lines are visually highlighted.

2. **`v` (character selection)**: in the diff pane, press `v` to enter
   character selection mode. `h`/`l`/`j`/`k` move the selection endpoint.
   Highlighted at character level.

3. **`Enter` to create comment**: after making a selection with `V` or
   `v`, press `Enter` to open the comment input.

4. **Comment input (short)**: a multi-line text input area appears at the
   bottom of the screen or as an overlay. `Ctrl-Enter` (or designated key)
   submits. `Escape` cancels.

5. **Comment input (long)**: a keybinding within the comment input
   (e.g. `Ctrl-e`) opens `$EDITOR` for longer comments. On editor exit,
   the text is read back.

6. **Cancel selection**: `Escape` in `V`/`v` mode before `Enter` cancels
   the selection and returns to normal mode.

7. **Anchor capture**: on creation, automatically capture and store
   anchor_text, context_before (3–5 lines), and context_after (3–5 lines).

8. **Server interaction**: creating a comment sends `create_comment` to
   the server. The server stores it and notifies other connected clients.

### Comment Display

9. **`c` toggles inline comment visibility**: pressing `c` (global key)
   toggles whether comments are shown inline in the diff pane. When off,
   comments are hidden from the diff but still visible in the comments
   panel.

10. **Inline comment blocks** (when visible): unresolved comments
    displayed below the lines they're attached to, visually distinct
    (different background, bordered):

    ```
      42 │  fn process(input: &str) -> Result<Output> {
      43 │+     let parsed = parse(input);
         │  ┌─ comment ──────────────────────────────────
         │  │ Does parse() handle empty input? If input is
         │  │ "", this will silently produce a default value.
         │  └────────────────────────────────────────────
      44 │+     transform(parsed)
    ```

11. **Gutter indicators**: lines with comments show a marker in the
    gutter, visible even when scrolled past the comment block:
    - `●` for unresolved comments.
    - `○` for resolved comments.

12. **Comments panel**: a togglable panel for viewing all comments for the
    current file or all files. Unresolved comments shown fully. Resolved
    comments shown **collapsed** (header line only: file, line range,
    body preview). `Enter` on a collapsed resolved comment expands it.
    The panel should support:
    - Filtering by file.
    - Sorting by file/line/timestamp.
    - Navigating to a comment's location in the diff.

### Anchor Resolution

13. **Only unresolved comments are re-anchored.** Resolved comments skip
    anchoring entirely. A resolved comment whose anchor_text no longer
    exists is expected — the feedback was addressed.

14. **Re-anchoring**: the server performs anchor resolution when loading
    comments for a connection. For each unresolved comment:
    - **Step 1**: anchor_text at stored line number → anchored.
    - **Step 2**: anchor_text elsewhere in file → shifted, reattach.
    - **Step 3**: context_before/after match → approximate.
    - **Step 4**: no match → orphaned.

15. **Orphaned comments**: visible in the comments panel with original
    context and orphaned indicator. Not silently lost.

### Comment Lifecycle

16. **Resolve/unresolve**: a keybinding toggles resolved status. Sends
    `resolve_comment` or `unresolve_comment` to the server. When
    resolved, the inline block disappears; the comment remains in the
    panel.

17. **Resolved + orphaned is success**: expected outcome when agent
    addresses feedback and code changes significantly.

18. **Edit**: a keybinding reopens the comment input with existing body.
    Sends `update_comment` to the server.

19. **Delete**: a keybinding deletes the comment with confirmation. Sends
    `delete_comment` to the server.

20. **Navigate between comments**: keybindings to jump to next/previous
    comment in the current file's diff.

21. **Live updates**: when another client (e.g. agent via MCP) resolves
    a comment, the server notifies the TUI. The comment's display
    updates immediately.

## Acceptance Criteria

- [ ] `V` enters line selection mode; `j`/`k` extend visually.
- [ ] `v` enters character selection mode with character-level highlight.
- [ ] `Escape` cancels selection without creating a comment.
- [ ] `Enter` after selection opens the comment input.
- [ ] Comment input supports multi-line text and `$EDITOR` escalation.
- [ ] Comments are persisted via the server and survive restart.
- [ ] `c` toggles inline comment visibility in the diff.
- [ ] Unresolved comments appear as inline blocks when visible.
- [ ] Gutter indicators show `●` (unresolved) and `○` (resolved).
- [ ] Comments panel lists all comments; resolved are collapsed.
- [ ] `Enter` on collapsed resolved comment expands it.
- [ ] Resolved comments can be unresolved from the panel.
- [ ] After rebase, unresolved comments re-anchor correctly.
- [ ] Orphaned unresolved comments appear in the panel.
- [ ] Resolved comments are not re-anchored and produce no warnings.
- [ ] Comments can be resolved, unresolved, edited, and deleted.
- [ ] Next/previous comment navigation works.
- [ ] Live updates from other clients are reflected immediately.

## Open Questions

- What keybinding opens the comments panel? `3` (matching `1`/`2` for
  pane toggling)? A command like `:comments`?
- Should comments have priorities or labels (e.g. "must fix", "nit",
  "question")?
- Should there be a way to reply to a comment (threaded comments)?
- How should the comment input handle very long comments — scrollable
  input area, or always escalate to `$EDITOR`?
- Should gutter indicators be visible when inline comments are toggled
  off with `c`?

## Progress

Sub-plans created. Implementation has not yet started.
