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

This stage is broken into focused sub-plans. Implement in recommended
order (core panel-based flow first):

1. 13a-selection-anchor-capture.md
2. 13b-composer-create.md
3. 13c-reanchor.md (can be parallel with 13a)
4. 13d-gutter-display.md
5. 13e-panel-lifecycle.md
6. 13f-live-polish-tests.md

Later / optional sub-plans (inline comments + virtual rows):

- 13g-visual-line-abstraction.md
- 13h-inline-comment-blocks.md (and related)

The core comment functionality (selection/capture, composer, re-anchoring,
gutter markers, and comments panel) can be built and used without any
inline comment blocks or changes to line rendering. Inline comment display
and the associated visual line abstraction are deferred so the panel-first
UX can be evaluated first.

See the individual sub-plan files under docs/plans/backlog/ for detailed
requirements, acceptance criteria, and notes for each slice.

Keybinding decision (recorded here):
- Use `c` in the diff pane to comment on the current line. If the file list
  has focus, switch to the diff pane and comment on the current diff line.
  If a visual selection is active, `c` comments on that selection.
- Use plain `}` for next comment and `{` for previous comment in the
  current file's diff.
- Use `Shift-C` to toggle the comments panel.
- Use `Tab` to cycle focus between visible panes/panels; do not use `3`
  to focus the comments panel.
- Use `e` to edit an existing comment at the current cursor/comment focus.
  If there is no comment at the current cursor, `e` does nothing.
- In later lifecycle flows, `c` also switches to the comment editor for the
  current comment context; otherwise it creates a new comment at the cursor.

## Requirements

### Selection and Comment Creation

1. **`v` (line selection)**: in the diff pane, press `v` to enter line
   selection mode. The current line is selected. `j`/`k` extend the
   selection. Selected lines are visually highlighted. Shifted `V` is
   accepted as the same mode.

2. **Mouse selection**: dragging in the diff pane selects text and copies it
   to the clipboard. Pressing `Enter` with an active diff mouse selection
   opens the comment input for the selected lines.

3. **`Enter` to create comment**: after making a keyboard or mouse
   selection, press `Enter` to open the comment input. The selected lines
   remain visible while the input is active. Pressing `c` with an active
   selection does the same.

4. **`c` to comment current line**: in the diff pane, press `c` to open
   the comment input anchored to the current diff line. If the file list has
   focus, `c` switches to the diff pane and comments on the current diff line.

5. **Comment input (short)**: a multi-line text input area appears at the
   bottom of the screen or as an overlay. `Ctrl-Enter` (or designated key)
   submits. `Escape` cancels.

6. **Comment input (long)**: a keybinding within the comment input
   (e.g. `Ctrl-e`) opens `$EDITOR` for longer comments. The editor buffer
   shows selected lines and context above a separator. On editor exit, the
   unchanged generated prefix is stripped; if the prefix was changed, the
   full file content is used as the comment body.

7. **Cancel selection**: `Escape` in `V`/`v` mode before `Enter` cancels
   the selection and returns to normal mode.

8. **Anchor capture**: on creation, automatically capture and store
   anchor_text, context_before (3–5 lines), and context_after (3–5 lines).

9. **Server interaction**: creating a comment sends `create_comment` to
   the server. The server stores it and notifies other connected clients.

### Comment Display (core / panel-first)

10. **Gutter indicators**: lines with comments show a marker in the gutter.
   - `●` for unresolved comments.
   - `○` for resolved comments.
   Gutter markers are visible whenever comments exist for the file,
   independent of whether the comments panel is open.

11. **Comments panel**: a togglable panel (Shift-C) for viewing all comments
     for the current file or scope. The panel appears at the bottom of the
     diff view. Unresolved comments are shown fully; resolved comments are
     shown collapsed (header + preview). `Enter` on a collapsed resolved
     comment expands it. The panel supports:
     - Filtering by file.
     - Sorting by file/line/timestamp.
     - Navigating to a comment's location in the diff.

12. **Cursor-driven panel display**: when the cursor is on a line that has
     a comment marker in the gutter, the relevant comment(s) are shown
     (expanded) in the comments panel.

Later / optional (inline comments):

13. **Inline toggle key (deferred)**: a possible future key can toggle
     comments inline in the diff pane. Do not use `c`; it is reserved for
     comment creation/editing.

14. **Inline comment blocks** (deferred): unresolved comments displayed
     below the lines they're attached to when inline mode is active.

### Anchor Resolution

15. **Only unresolved comments are re-anchored.** Resolved comments skip
    anchoring entirely. A resolved comment whose anchor_text no longer
    exists is expected — the feedback was addressed.

16. **Re-anchoring**: the server performs anchor resolution when loading
    comments for a connection. For each unresolved comment:
    - **Step 1**: anchor_text at stored line number → anchored.
    - **Step 2**: anchor_text elsewhere in file → shifted, reattach.
    - **Step 3**: context_before/after match → approximate.
    - **Step 4**: no match → orphaned.

    On steps 1–3 a new row is appended to `anchor_versions` using the current
    `file_blob_sha` and fresh context strings. The `v_current_anchors` view
    then provides the latest attachment for display and future re-anchoring.

17. **Orphaned comments**: visible in the comments panel with their
    last-known context (from the most recent anchor version, or the
    creation version if none exists) and an orphaned indicator.
    Not silently lost.

### Comment Lifecycle

18. **Resolve/unresolve**: a keybinding toggles resolved status. Sends
    `resolve_comment` or `unresolve_comment` to the server. When
    resolved, the inline block disappears; the comment remains in the
    panel.

19. **Resolved + orphaned is success**: expected outcome when agent
    addresses feedback and code changes significantly.

20. **Edit**: a keybinding reopens the comment input with existing body.
    Sends `update_comment` to the server.

21. **Delete**: a keybinding deletes the comment with confirmation. Sends
    `delete_comment` to the server.

22. **Navigate between comments**: keybindings to jump to next/previous
    comment in the current file's diff.

23. **Live updates**: when another client (e.g. agent via MCP) resolves
    a comment, the server notifies the TUI. The comment's display
    updates immediately.

## Acceptance Criteria

- [ ] `v` enters line selection mode; `j`/`k` extend visually.
- [ ] `V` enters the same line selection mode.
- [ ] Diff mouse selection copies to the clipboard and Enter opens the
      comment input for the selected lines.
- [ ] `Escape` cancels selection without creating a comment.
- [ ] `Enter` after selection opens the comment input.
- [ ] Comment input supports multi-line text and `$EDITOR` escalation.
- [ ] `$EDITOR` buffers include selected lines and context, and strip the
      unchanged generated prefix on return.
- [ ] Comments are persisted via the server and survive restart.
- [ ] `c` (if implemented) toggles inline comment visibility (deferred).
- [ ] Unresolved comments appear as inline blocks when visible (deferred).
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

- Should comments have priorities or labels (e.g. "must fix", "nit",
  "question")?
- Should there be a way to reply to a comment (threaded comments)?
- How should the comment input handle very long comments — scrollable
  input area, or always escalate to `$EDITOR`?
- Should gutter indicators be visible when inline comments are toggled
  off with `c`? (Deferred question — panel-first flow uses gutter markers
  regardless of any inline toggle.)

## Progress

Sub-plans created. Implementation has not yet started.
