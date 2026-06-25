# Stage 14: MCP Review Comment Tools

## Status: Backlog

## Goal

Expose review comments and review-state actions through the rmcp-based MCP
adapter.

## Why

LLM agents are primary consumers of review comments. They need a structured
way to discover outstanding comments, inspect full context, and mark comments
resolved after making code changes.

The MCP protocol adapter itself is now implemented with `rmcp`, and session
selection is handled through `list_review_sessions` /
`select_review_session`. This plan keeps only the missing comment and review
tool surface.

## Current State

- `crt mcp-server` starts and completes MCP handshake through `rmcp`.
- The MCP adapter can discover and select active review sessions.
- MCP currently exposes:
  - `list_review_sessions`,
  - `select_review_session`,
  - `list_changed_files`,
  - `get_file_diff`,
  - `search_codebase`,
  - `find_definition`,
  - `list_review_summary`.
- Review comment database operations and protocol types exist.
- Server dispatch recognizes comment methods, but currently returns
  `not implemented` for them.

## Scope

- In scope: server RPC handlers for existing comment operations.
- In scope: typed client wrappers for comment operations.
- In scope: MCP tools for comment discovery/detail/resolve workflows.
- In scope: MCP tools for marking files reviewed/unreviewed.
- In scope: summary output that includes comment counts.
- Out of scope: replacing `rmcp` or reintroducing manual MCP JSON-RPC
  handling.
- Out of scope: cwd/base-bound MCP startup as the primary flow; tools operate
  on the selected review session.
- Out of scope: creating, updating, or deleting comments through MCP unless a
  separate product decision explicitly enables agent-authored comments.

## Requirements

1. Implement server handlers for existing comment RPC methods:
   - `create_comment`,
   - `list_comments`,
   - `get_comment`,
   - `update_comment`,
   - `resolve_comment`,
   - `unresolve_comment`,
   - `delete_comment`.

2. Replace comment client wrappers that return `serde_json::Value` with typed
   results:
   - `ListCommentsResult`,
   - `CommentResult`,
   - `DeleteCommentResult`.

3. Add MCP comment tools:
   - `list_review_comments`
     - `file_path` optional,
     - `include_resolved` optional, default false,
     - returns comment id, file path, line/character range, anchor text,
       body, resolved status, timestamps, and anchor status.
   - `get_comment_detail`
     - `id` required,
     - returns the full comment including context before/after and anchor
       status.
   - `resolve_comment`
     - `id` required,
     - marks the comment resolved and returns the updated comment.
   - `unresolve_comment`
     - `id` required,
     - marks the comment unresolved and returns the updated comment.

4. Add MCP review-state tools:
   - `mark_file_reviewed`
     - `file_path` required,
     - marks the selected file reviewed and returns the new review status.
   - `unmark_file_reviewed`
     - `file_path` required,
     - clears the selected file's reviewed state and returns the new review
       status.

5. Update `list_review_summary` to include comment counts:
   - total comments,
   - unresolved comments,
   - resolved comments,
   - per-file comment counts where practical.

6. Keep tool descriptions self-sufficient for LLM agents:
   - identify when `select_review_session` is required,
   - describe parameters and defaults,
   - describe the shape of returned data.

7. Preserve rmcp-owned protocol behavior:
   - capability negotiation,
   - tool listing,
   - argument schemas,
   - malformed request handling.

## Implementation Notes

- Keep `src/mcp.rs` as a thin adapter over the shared `Client`.
- Keep behavior semantics in server/client/core layers, not in the MCP tool
  methods.
- Use existing database comment CRUD and anchor-status conversion logic.
- Prefer typed result structs from `review_types.rs` instead of raw JSON in
  client code.
- MCP tools should require a selected session, except discovery tools.

## Deliverables

- Server comment RPC handlers.
- Typed client comment methods.
- New MCP comment/review tools.
- Summary output with comment counts.
- Focused server and MCP tests.

## Acceptance Criteria

- [ ] `list_comments` returns scoped comments from the server.
- [ ] `list_comments` respects `file_path`.
- [ ] `list_comments` respects `include_resolved`.
- [ ] `get_comment` returns full context and anchor status.
- [ ] `resolve_comment` marks a comment resolved.
- [ ] `unresolve_comment` marks a comment unresolved.
- [ ] Typed client comment methods parse server responses correctly.
- [ ] MCP `list_review_comments` returns correct data.
- [ ] MCP `get_comment_detail` returns full comment context.
- [ ] MCP `resolve_comment` and `unresolve_comment` update state.
- [ ] MCP `mark_file_reviewed` and `unmark_file_reviewed` update file review
      state.
- [ ] `list_review_summary` includes accurate comment counts.
- [ ] Tool descriptions are detailed enough for an LLM agent to use without
      extra documentation.

## Resolved Decisions

- MCP remains a key feature of the app.
- The adapter uses `rmcp`; manual MCP JSON-RPC handling is superseded.
- MCP startup can be unscoped; agents choose an active review session through
  MCP tools.
- Agent-created comments remain out of MCP scope for now.
