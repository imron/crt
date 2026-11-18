# Stage 13j: Check Comments CLI Command

## Status: Placeholder

## Order

Future follow-up after unresolved comment persistence is stable.

## Depends On

- 13b (comment creation)
- 13c (comment persistence and re-anchoring)
- 13e (comment lifecycle)

## Goal

Add a `crt check-comments [BASE]` command that reports whether unresolved
review comments exist for the current review scope.

## Why

Agents and scripts need a small, machine-friendly command that can fail a
workflow while unresolved review feedback remains.

## Command Contract

1. Support only this shape:

   ```text
   crt check-comments [BASE]
   ```

2. Do not support `crt <BASE> check-comments`.

3. When `BASE` is provided, resolve the review scope the same way review mode
   does:

   - discover the current git worktree,
   - resolve `HEAD`,
   - compute `merge_base(BASE, HEAD)`,
   - use `head.scope_key()` for the head scope.

4. When `BASE` is omitted, infer the scope only if it is unambiguous. The
   implementation should prefer an active review session for the current
   worktree. If no unambiguous scope exists, print a short message asking for
   `BASE` and exit with a usage/error status.

5. Exit status meanings:

   - `0`: no unresolved comments exist,
   - `1`: one or more unresolved comments exist,
   - `2`: usage, scope resolution, git, or database error.

6. Output must be brief:

   - no comments: `No unresolved comments.`
   - unresolved comments:
     `N unresolved comment(s) in M file(s): path/a.rs, path/b.rs`

7. File paths in the unresolved message should be unique, sorted, and relative
   to the worktree/review file paths already stored with comments.

8. If no comment database exists for the repo, treat that as no unresolved
   comments for a resolved scope.

## Implementation Notes

- Add a `CheckComments { base: Option<String> }` CLI subcommand.
- Keep the command independent from `apply-comments` / `clear-comments`.
- Reuse existing git scope resolution and database comment listing where
  possible.
- Add a small helper that returns the unresolved count and unique file list.
- Preserve `1` exclusively for the "unresolved comments exist" case so scripts
  can distinguish feedback from operational errors.
- Add CLI parsing tests to reject `crt <BASE> check-comments`.
- Add command behavior tests for:
  - no database,
  - zero unresolved comments,
  - unresolved comments across one file,
  - unresolved comments across multiple files,
  - resolved comments ignored,
  - omitted `BASE` with ambiguous scope.

## Acceptance Criteria

- [ ] `crt check-comments main` exits `0` and prints
      `No unresolved comments.` when no unresolved comments exist.
- [ ] `crt check-comments main` exits `1` and prints the count and involved
      files when unresolved comments exist.
- [ ] Resolved comments do not affect the exit code or count.
- [ ] Missing review database exits `0` for a resolved scope.
- [ ] `crt check-comments` without `BASE` works only when the current scope is
      unambiguous.
- [ ] `crt <BASE> check-comments` is rejected.
- [ ] Operational failures use exit status `2`, not `1`.

## Open Questions

- Should omitted `BASE` require a running active review session, or can it use
  the most recent database scope for the current head when exactly one exists?

## Progress

- Placeholder created.
