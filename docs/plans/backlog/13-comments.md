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

### Comment Display (core / panel-first)

9. **Gutter indicators**: lines with comments show a marker in the gutter.
   - `●` for unresolved comments.
   - `○` for resolved comments.
   Gutter markers are visible whenever comments exist for the file,
   independent of whether the comments panel is open.

10. **Comments panel**: a togglable panel (Shift-C) for viewing all comments
     for the current file or scope. The panel appears at the bottom of the
     diff view. Unresolved comments are shown fully; resolved comments are
     shown collapsed (header + preview). `Enter` on a collapsed resolved
     comment expands it. The panel supports:
     - Filtering by file.
     - Sorting by file/line/timestamp.
     - Navigating to a comment's location in the diff.

11. **Cursor-driven panel display**: when the cursor is on a line that has
     a comment marker in the gutter, the relevant comment(s) are shown
     (expanded) in the comments panel.

Later / optional (inline comments):

12. **`c` (deferred)**: possible future toggle for showing comments inline
     in the diff pane.

13. **Inline comment blocks** (deferred): unresolved comments displayed
     below the lines they're attached to when inline mode is active.

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

    On steps 1–3 a new row is appended to `anchor_versions` using the current
    `file_blob_sha` and fresh context strings. The `v_current_anchors` view
    then provides the latest attachment for display and future re-anchoring.

15. **Orphaned comments**: visible in the comments panel with their
    last-known context (from the most recent anchor version, or the
    creation version if none exists) and an orphaned indicator.
    Not silently lost.

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

- What keybinding opens the comments panel? `3` (matching `1`/`2` for
  pane toggling)? A command like `:comments`?
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
