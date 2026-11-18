# Stage 2: Git Module

## Goal

Implement the git operations layer: listing all files changed between the
base ref and HEAD, computing unified diffs per file, hashing diff content
for change detection, and resolving worktree/repo paths.

## Why

The git module is the data source for the entire application. The file list
pane needs to know which files changed. The diff view pane needs the actual
diff content. The review state system needs diff hashes to determine whether
a file's diff has changed since it was last reviewed. All of this comes from
git, and we need a clean API that the rest of the application (and the
server) can call without worrying about git2 internals.

## Requirements

1. **List changed files**: given a base commit OID and HEAD, produce a list
   of all files that differ between the two. Each entry should include:
   - File path (relative to repo root).
   - Change type: added, deleted, modified, renamed.
   - For renames: both the old and new path.

2. **Compute diff per file**: given a base commit OID, HEAD, and a file
   path, produce the unified diff for that file. The diff should be
   returned as structured data (not a raw string), with enough information
   to render both inline and side-by-side views later. At minimum:
   - List of diff hunks.
   - Each hunk contains: old start line, old line count, new start line,
     new line count, and a list of diff lines.
   - Each diff line has: a type (context, addition, deletion), the line
     content, and the line numbers in the old and new files.

3. **Read full file content**: given a commit (or working tree) and a file
   path, return the full file content. This supports the full-file view
   mode (`f` key). Must work for both the HEAD version and the base
   version.

4. **Hash diff content**: for a given file's diff, produce a stable
   SHA-256 hash of the diff content. This hash must be deterministic —
   the same diff content must always produce the same hash, regardless of
   when or how many times it's computed.

5. **Worktree resolution**: given a path (which may be a worktree or the
   main working tree), resolve:
   - The main repository root (via `commondir`).
   - The worktree path.
   - The branch name at HEAD of the worktree.
   This is used by the server during the `init` handshake.

6. **Handle edge cases**:
   - Binary files: detect and represent as "binary file changed" rather
     than attempting to diff.
   - Empty files / files with no trailing newline.
   - Files that were added (no old version) or deleted (no new version).
   - Very large diffs: the API should not panic or hang.

7. **API design**: the git module should expose a clean public API that
   hides git2 types from the rest of the application. Callers should not
   need to import anything from `git2` directly.

## Acceptance Criteria

- [ ] Given a repo with known changes between two commits, the module
      correctly lists all changed files with their change types.
- [ ] Given a specific changed file, the module produces a structured diff
      that includes hunk headers, line types, line content, and line
      numbers.
- [ ] Full file content can be read for both the HEAD and base versions.
- [ ] The diff hash for a given file is deterministic across multiple
      calls.
- [ ] Renamed files are detected and both paths are available.
- [ ] Binary files are identified without producing garbled diff output.
- [ ] Added and deleted files produce valid diffs (one side empty).
- [ ] The public API does not expose any `git2` types.
- [ ] Worktree resolution correctly identifies the main repo root and
      current branch from both a worktree and the main working tree.

## Open Questions

- Should we diff against the working tree (uncommitted changes) or only
  against committed HEAD? For code review, committed-only seems correct,
  but there may be cases where seeing uncommitted changes is useful.
- How should submodules be handled? Ignore them, show as a single changed
  entry, or recurse into them?
- Should we support a maximum diff size to prevent memory issues with very
  large files? If so, what's the threshold?
