# Stage 13j: Check Comments Commands

## Status: Planned

## Order

Future follow-up after 13k comment provenance and base/head anchors are
implemented.

## Depends On

- 13b (comment creation)
- 13c (comment persistence and re-anchoring)
- 13e (comment lifecycle)
- 13k (comment provenance plus base/head/both anchor sides)

## Goal

Add commands that report whether unresolved review comments still apply to a
target head commit:

- `crt check [HEAD]` for shell/CI workflows,
- `crt check --range BASE HEAD` for exact review ranges,
- an MCP `check_comments` tool for agent workflows.

## Why

Agents and scripts need a small, machine-friendly command that can fail a
workflow while unresolved review feedback remains. The common case is asking
whether the current result is clean, not whether one specific historical
base/head pair is clean. MCP agents need the same check without shelling out
or parsing human text.

## Command Contract

1. Support only these shapes:

   ```text
   crt check
   crt check HEAD
   crt check --range BASE HEAD
   ```

2. Do not support these ambiguous or legacy shapes:

   ```text
   crt BASE check
   crt check BASE HEAD
   ```

   A single positional argument to `crt check` is always interpreted as a head
   ref, not a base ref.

3. `crt check` is shorthand for `crt check HEAD`.

4. `crt check HEAD` checks unresolved comments that may still apply to the
   target head commit:

   - discover the current git worktree,
   - resolve the target `HEAD` argument to an exact commit,
   - find unresolved comments created on review head commits that are
     ancestors of the target commit,
   - re-anchor those comments against the target head content before
     reporting,
   - count orphaned unresolved comments as unresolved,
   - ignore resolved comments.

5. `crt check --range BASE HEAD` checks unresolved comments for a specific
   review range:

   - discover the current git worktree,
   - resolve `BASE` and `HEAD`,
   - compute `merge_base(BASE, HEAD)`,
   - include exact range comments,
   - include unresolved carry-over comments that can apply to the target
     head,
   - re-anchor comments in the `BASE`/`HEAD` context before reporting,
   - count orphaned unresolved comments as unresolved,
   - ignore resolved comments.

6. Exit status meanings:

   - `0`: no unresolved comments exist,
   - `1`: one or more unresolved comments exist,
   - `2`: usage, scope resolution, git, or database error.

7. Output must be brief:

   - no comments: `No unresolved comments.`
   - unresolved comments:

     ```text
     N unresolved comment(s) in M file(s):
       path/a.rs
       path/b.rs
     ```

8. File paths in the unresolved message should be unique, sorted, and relative
   to the worktree/review file paths already stored with comments.

9. If no comment database exists for the repo, treat that as no unresolved
   comments for a resolved scope.

10. The command must be conservative. If an unresolved comment cannot be
    proven resolved or irrelevant, it should be reported.

## MCP Tool Contract

1. Add a `check_comments` MCP tool.

2. The tool uses the selected review session by default. Agents must call
   `list_review_sessions` and `select_review_session` before using the default
   mode, matching the other scoped MCP tools.

3. Parameters:

   ```json
   {
     "mode": {
       "kind": "head",
       "head_ref": "HEAD"
     }
   }
   ```

   ```json
   {
     "mode": {
       "kind": "range",
       "base_ref": "main",
       "head_ref": "HEAD"
     }
   }
   ```

   `mode` is optional. If omitted, use the selected session's head ref.
   `mode.kind` is an internally tagged enum. The `head` variant accepts an
   optional `head_ref`; the `range` variant requires both `base_ref` and
   `head_ref`.

4. The MCP tool returns structured data instead of an exit status:

   ```json
   {
     "has_unresolved": true,
     "unresolved_count": 2,
     "file_count": 1,
     "files": ["src/app.rs"],
     "mode": {
       "kind": "head",
       "head_ref": "HEAD",
       "resolved_head_commit": "abc123"
     }
   }
   ```

   Range responses use the same tagged shape:

   ```json
   {
     "mode": {
       "kind": "range",
       "base_ref": "main",
       "head_ref": "HEAD",
       "resolved_base_commit": "def456",
       "resolved_head_commit": "abc123",
       "merge_base": "789abc"
     }
   }
   ```

5. `has_unresolved` is the MCP equivalent of CLI exit status `1`. Operational
   errors are returned as MCP tool errors or error strings consistent with the
   existing adapter.

6. The tool must use the same shared check helper as the CLI so the CLI and
   MCP adapter cannot drift on scope, re-anchor, or orphan handling.

## Implementation Notes

- Add a `Check { head: Option<String>, range: Option<CheckRange> }` CLI
  subcommand shape, or equivalent clap representation.
- Add a `CheckCommentsParams` MCP parameter type and
  `CheckCommentsSummary` result type.
- Model MCP mode as an internally tagged enum, equivalent to:

  ```rust
  #[serde(tag = "kind", rename_all = "snake_case")]
  enum CheckCommentsMode {
      Head { head_ref: Option<String> },
      Range { base_ref: String, head_ref: String },
  }
  ```

- Keep the command independent from `apply-comments` / `clear-comments`.
- Reuse existing git scope resolution, ancestry checks, and database comment
  listing where possible.
- Add a shared helper that returns unresolved count and unique file list after
  re-anchoring for a target context.
- Preserve `1` exclusively for the "unresolved comments exist" case so scripts
  can distinguish feedback from operational errors.
- Add CLI parsing tests to reject `crt BASE check` and `crt check BASE HEAD`.
- Add MCP tests for default selected-session mode, explicit head mode, explicit
  range mode, and malformed partial range parameters.
- Add command behavior tests for:
  - no database,
  - zero unresolved comments,
  - unresolved comments across one file,
  - unresolved comments across multiple files,
  - resolved comments ignored,
  - explicit head ref,
  - default current `HEAD`,
  - range-specific `BASE HEAD`,
  - orphaned unresolved comments still counted,
  - unresolved comments on non-ancestor review heads ignored.

## 13k Requirements

This command depends on 13k because the current schema does not record enough
information to answer `crt check HEAD` precisely after a branch moves. 13k
should provide:

- exact head commit provenance for the commit being reviewed when a comment is
  created,
- side-aware anchors (`base`, `head`, `both`),
- re-anchor helpers that can evaluate comments against an arbitrary target
  head or explicit `BASE`/`HEAD` range,
- conservative applicability rules for unresolved comments from previous
  review ranges.

## Acceptance Criteria

- [ ] `crt check` exits `0` and prints
      `No unresolved comments.` when no unresolved comments exist.
- [ ] `crt check HEAD` exits `1` and prints the count and involved
      files when unresolved comments exist.
- [ ] `crt check --range main HEAD` checks only comments applicable to that
      explicit range.
- [ ] MCP `check_comments` returns the same unresolved count and file
      list as the CLI check helper.
- [ ] MCP `check_comments` supports selected-session default, explicit
      head, and explicit range modes.
- [ ] Resolved comments do not affect the exit code or count.
- [ ] Missing review database exits `0` for a resolved scope.
- [ ] Orphaned unresolved comments still cause exit status `1`.
- [ ] `crt BASE check` is rejected.
- [ ] `crt check BASE HEAD` is rejected.
- [ ] MCP partial range parameters are rejected.
- [ ] Operational failures use exit status `2`, not `1`.

## Open Questions

- Should output include comment ids in a machine-readable mode later?
- Should the command grow `--json`, or should plain text remain the only
  contract for now?
- Should the MCP check include comment ids directly, or require agents to call
  `list_review_comments` when `has_unresolved` is true?

## Progress

- Planned command contract changed from `[BASE]` scope checks to default
  `[HEAD]` ancestry checks plus explicit `--range BASE HEAD`.
- Added MCP `check_comments` as the agent-facing equivalent of the CLI check.
