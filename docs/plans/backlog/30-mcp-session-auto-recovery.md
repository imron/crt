# Stage 30: MCP Session Auto-Recovery

## Status

Backlog.

## Goal

Make MCP scoped tools recover automatically when the adapter loses selected
session initialization or the underlying shared server drops and reconnects.

## Why

Agent workflows call tools such as `list_review_comments`, `get_file_diff`,
and `resolve_comment` after selecting a review session. If the MCP adapter or
shared server loses connection state, those scoped tools can fail with errors
such as:

```text
Connection not initialized. Send 'init' first.
```

The agent can manually recover by listing sessions and selecting the active
session again, but that is avoidable friction. The MCP adapter already owns the
selected session metadata, so it should use that metadata to re-initialize or
re-select automatically before returning a hard failure.

## Scope

- In scope: MCP adapter behavior for scoped tools.
- In scope: automatic retry after a "not initialized" or lost-session failure.
- In scope: preserving the selected review session across reconnects when the
  same session is still available.
- In scope: clear errors when the selected session no longer exists.
- Out of scope: changing server-side review scope semantics.
- Out of scope: changing the core client reconnect supervisor, unless a small
  helper is needed by the MCP adapter.

## Design

The MCP adapter should wrap scoped tool execution in a small recovery helper:

1. Run the scoped operation normally.
2. If it fails because the selected session is not initialized, stale, or lost:
   - refresh active review sessions,
   - find the previously selected session by stable identity,
   - re-run the equivalent `select_review_session` initialization,
   - retry the original operation once.
3. If the selected session cannot be found, return a clear MCP tool error that
   tells the caller to list and select a session.
4. If the retry fails for the same reason, return the retry error without
   looping indefinitely.

The stable identity should prefer:

- worktree path,
- merge base,
- base ref,
- head ref when available.

The helper should live in the MCP adapter layer, not in individual tool bodies.
Individual scoped tools should call the helper so recovery behavior cannot
drift between `list_review_comments`, `resolve_comment`, `get_file_diff`, and
future scoped tools.

## Tests

- A scoped MCP tool succeeds normally after session selection.
- A scoped MCP tool that receives a "not initialized" response re-selects the
  previous session and retries once.
- The retry path works for at least one read tool and one mutation tool.
- If the previous session no longer exists, the tool returns a clear error.
- If the retried operation still fails with "not initialized", the adapter does
  not loop.
- Existing manual `list_review_sessions` and `select_review_session` behavior
  remains unchanged.

## Acceptance Criteria

- [ ] Scoped MCP tools automatically recover from stale initialization.
- [ ] Recovery is implemented once and reused by scoped tools.
- [ ] Recovery retries the original operation at most once.
- [ ] Missing-session failures return a clear actionable error.
- [ ] Tests cover read, mutation, missing-session, and no-infinite-loop cases.
- [ ] Tool behavior remains unchanged when no recovery is needed.
