# Stage 14: MCP Adapter

## Goal

Build the MCP adapter that bridges the MCP stdio protocol to the crt
server's JSON-RPC API, exposing review comments and state to LLM agents.

## Why

LLM agents are the primary consumers of review comments. The MCP adapter
provides a structured, queryable interface: the agent can ask "what
comments need addressing?", read the full context of each comment, and
mark comments as resolved after fixing the code.

The adapter is a thin translation layer — it speaks MCP on stdio (for the
agent's MCP client) and forwards requests to the crt server over the Unix
socket. This keeps the adapter simple and ensures all logic lives in the
server.

## Requirements

### Adapter Infrastructure

1. **`crt mcp-server` subcommand**: launches the MCP adapter. Reads
   MCP JSON-RPC from stdin, writes to stdout. Connects to a running crt
   server via Unix socket, or starts an embedded server if none is
   running.

2. **Connection to server**: on startup, the adapter connects to the crt
   server and sends `init` with the worktree path (cwd) and base ref
   (from a `--base` argument or auto-detected).

3. **MCP protocol compliance**: correct implementation of:
   - Capability negotiation.
   - Tool listing (`tools/list`).
   - Tool invocation (`tools/call`).
   - Proper JSON-RPC 2.0 error responses.

### Tools

4. **`list_review_comments`**: forwards to server's `list_comments`.
   Parameters:
   - `file_path` (optional) — filter to one file.
   - `include_resolved` (optional, default false).
   Returns each comment with: ID, file path, line range, anchor text,
   body, resolved status, timestamps, anchor status.

5. **`get_comment_detail`**: forwards to server's `get_comment`. Returns
   full context including context_before, context_after, anchor status.

6. **`resolve_comment`**: forwards to server's `resolve_comment`. Returns
   confirmation.

7. **`unresolve_comment`**: forwards to server's `unresolve_comment`.
   Returns confirmation.

8. **`list_review_summary`**: assembles from server's
   `list_changed_files` and `list_comments`. Returns:
   - Base ref and head ref.
   - Total files changed, reviewed count, unreviewed count.
   - Total comments (unresolved and resolved counts).
   - Per-file breakdown.

9. **`get_file_diff`**: forwards to server's `get_file_diff`. Returns
   the diff content for a file.

### Documentation

10. **Tool descriptions**: every tool has a clear, detailed description
    and parameter documentation in the MCP tool schema. These must be
    self-sufficient — an agent should be able to use the tools correctly
    based solely on the descriptions.

### Error Handling

11. **Server not running**: if no server is available and embedded server
    fails to start, return a clear error via MCP.

12. **Invalid requests**: malformed tool calls return proper MCP error
    responses.

13. **Server disconnection**: if the server disconnects mid-session,
    return errors for subsequent requests.

## Acceptance Criteria

- [ ] `crt mcp-server` starts and completes MCP handshake over stdio.
- [ ] `tools/list` returns all tools with descriptions and parameter
      schemas.
- [ ] `list_review_comments` returns correct data.
- [ ] `list_review_comments` with `file_path` filter works.
- [ ] `list_review_comments` with `include_resolved: true` works.
- [ ] `get_comment_detail` returns full context and anchor status.
- [ ] `resolve_comment` marks a comment as resolved and the change is
      visible in the TUI.
- [ ] `unresolve_comment` toggles back to unresolved.
- [ ] `list_review_summary` returns accurate counts.
- [ ] `get_file_diff` returns diff content.
- [ ] Malformed requests receive proper MCP error responses.
- [ ] Tool descriptions are detailed enough for an LLM agent to use
      without additional documentation.

## Open Questions

- How should the adapter determine the `base_ref` for the `init` call?
  A `--base` flag? Auto-detect from the most recent review in the DB?
  Environment variable?
- Should the adapter stay running indefinitely (like a typical MCP
  server), or should it have an idle timeout?
- Should the adapter support creating comments (allowing agents to leave
  their own notes), or only read + resolve?
- Should there be a tool for the agent to mark files as reviewed?
