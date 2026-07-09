# Stage 13: Comments

## Status

Partially complete. Core creation, persistence, re-anchoring, gutter markers,
comments panel lifecycle, and unresolved file-panel scanning are implemented.
Final live-update polish, agent checks, and base/head anchor follow-ups remain.

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

Later / optional sub-plans:

- 13i-unresolved-comments-pane.md
- 13k-base-head-comment-anchors.md

The core comment functionality (selection/capture, composer, re-anchoring,
gutter markers, and comments panel) can be built and used without any
inline comment blocks or changes to line rendering. Inline comment display
and the associated visual line abstraction have been abandoned for now because
the comments panel and unresolved comments file-panel section cover the needed
review workflows.

Completed sub-plans:

- 13a-selection-anchor-capture.md
- 13b-composer-create.md
- 13c-reanchor.md
- 13d-gutter-display.md
- 13e-panel-lifecycle.md
- 13i-unresolved-comments-pane.md
- 13l-comment-projection.md

Remaining sub-plans:

- 13f-live-polish-tests.md
- 13j-check-comments-command.md
- 13k-base-head-comment-anchors.md

Superseded sub-plans:

- 13m-comment-projection-contract.md, replaced by the Stage 29 diff document
  model.

Abandoned sub-plans:

- 13g-visual-line-abstraction.md
- 13h-inline-comment-blocks.md and related inline-row work

See the individual sub-plan files under `docs/plans/backlog/` and
`docs/plans/completed/` for detailed requirements, acceptance criteria, and
notes for each slice.

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
- Use the file panel's `Unresolved Comments` section to scan outstanding
  feedback without opening the full comments panel.

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

13. **Unresolved comments section**: the file panel includes an
    `Unresolved Comments` section. It lists unresolved visible comments across
    files, including unresolved comments carried over from previous bases.
    Activating a row jumps to the owning file and anchored line.

### Anchor Resolution

14. **Only unresolved comments are re-anchored.** Resolved comments skip
    anchoring entirely. A resolved comment whose anchor_text no longer
    exists is expected — the feedback was addressed.

15. **Re-anchoring**: the server performs anchor resolution when loading
    comments for a connection. For each unresolved comment:
    - **Step 1**: anchor_text at stored line number → anchored.
    - **Step 2**: anchor_text elsewhere in file → shifted, reattach.
    - **Step 3**: context_before/after match → approximate.
    - **Step 4**: no match → orphaned.

    On steps 1–3 a new row is appended to `anchor_versions` using the current
    `file_blob_sha` and fresh context strings. The `v_current_anchors` view
    then provides the latest attachment for display and future re-anchoring.

16. **Orphaned comments**: visible in the comments panel with their
    last-known context (from the most recent anchor version, or the
    creation version if none exists) and an orphaned indicator.
    Not silently lost.

### Comment Lifecycle

17. **Resolve/unresolve**: a keybinding toggles resolved status. Sends
    `resolve_comment` or `unresolve_comment` to the server. When
    resolved, the comment remains available through the panel and history.

18. **Resolved + orphaned is success**: expected outcome when agent
    addresses feedback and code changes significantly.

19. **Edit**: a keybinding reopens the comment input with existing body.
    Sends `update_comment` to the server.

20. **Delete**: a keybinding deletes the comment with confirmation. Sends
    `delete_comment` to the server.

21. **Navigate between comments**: keybindings to jump to next/previous
    comment in the current file's diff.

22. **Live updates**: when another client (e.g. agent via MCP) resolves
    a comment, the server notifies the TUI. The comment's display
    updates immediately.

## Acceptance Criteria

- [x] `v` enters line selection mode; `j`/`k` extend visually.
- [x] `V` enters the same line selection mode.
- [ ] Diff mouse selection copies to the clipboard and Enter opens the
      comment input for the selected lines.
- [x] `Escape` cancels selection without creating a comment.
- [x] `Enter` after selection opens the comment input.
- [x] Comment input supports multi-line text and `$EDITOR` escalation.
- [x] `$EDITOR` buffers include selected lines and context, and strip the
      unchanged generated prefix on return.
- [x] Comments are persisted via the server and survive restart.
- [x] Gutter indicators show `●` (unresolved) and `○` (resolved).
- [x] Comments panel lists all comments; resolved are collapsed.
- [x] `Enter` on collapsed resolved comment expands it.
- [x] Resolved comments can be unresolved from the panel.
- [x] After rebase, unresolved comments re-anchor correctly.
- [x] Orphaned unresolved comments appear in the panel.
- [x] Resolved comments are not re-anchored and produce no warnings.
- [x] Comments can be resolved, unresolved, edited, and deleted.
- [x] Next/previous comment navigation works.
- [x] Unresolved comments can be scanned from the file panel.
- [ ] Live updates from other clients are reflected immediately.

## Open Questions

- Should comments have priorities or labels (e.g. "must fix", "nit",
  "question")?
- Should there be a way to reply to a comment (threaded comments)?
- How should the comment input handle very long comments — scrollable
  input area, or always escalate to `$EDITOR`?

## Progress

- 13a selection and anchor capture completed.
- 13b composer and creation completed.
- 13c server re-anchoring completed.
- 13d gutter markers completed.
- 13e comments panel and lifecycle completed.
- 13i unresolved comments pane completed as a file-panel section.
- 13g visual line abstraction and related inline-row work abandoned because
  comment workflows no longer need inline comment rows.
- 13f remains as the final live-update polish and full verification pass.
