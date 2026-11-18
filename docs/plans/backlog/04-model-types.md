# Stage 4: Model Types

## Goal

Define the core data types used across the application. These types form
the shared vocabulary between the git module, database module, server,
client, and UI.

## Why

Having well-defined types prevents the modules from being coupled to each
other's internals. The git module produces `DiffContent` and `FileChange`
values. The database module works with `StoredReview` values. The server
combines them into `FileEntry` values and sends them to clients. Clear type
boundaries make each module independently understandable and testable.

## Requirements

1. **FileChange**: represents a file that changed between base and HEAD.
   - File path.
   - Change kind: added, deleted, modified, renamed.
   - Old path (for renames).

2. **DiffHunk**: a single hunk within a diff.
   - Old file start line and line count.
   - New file start line and line count.
   - The list of diff lines in this hunk.

3. **DiffLine**: a single line within a diff hunk.
   - Line type: context, addition, deletion.
   - Content (the text of the line).
   - Old file line number (if applicable).
   - New file line number (if applicable).

4. **DiffContent**: the complete diff for a single file.
   - The list of hunks.
   - Whether the file is binary.
   - The diff hash (SHA-256).

5. **ReviewStatus**: an enum representing a file's review state:
   - `Unreviewed` — never reviewed.
   - `Reviewed { at: DateTime }` — reviewed and diff unchanged since.
   - `Changed { at: DateTime }` — was reviewed, but diff has changed.

6. **FileEntry**: the complete state of a single file in the review.
   - The file change information.
   - The review status.
   - The diff content (may be loaded lazily later, but eagerly for now).

7. **Comment**: a review comment.
   - All fields from the database schema.
   - Anchor status: anchored, shifted, approximate, orphaned.

8. **Pane focus**: which pane currently has focus.

9. **Content mode**: diff mode vs. full-file mode.

10. **Render variant**: inline vs. side-by-side (diff mode), or HEAD vs.
    base (full-file mode).

11. **Connection context**: the resolved state from the `init` handshake.
    - Repo root path.
    - Worktree path.
    - Base ref.
    - Head ref.

12. **Server message types**: request and response types for the JSON-RPC
    API. These should be serializable/deserializable with serde.

## Acceptance Criteria

- [ ] All types are defined and compile.
- [ ] Types use appropriate Rust idioms (enums for variants, structs for
      records, `Option` where values may be absent).
- [ ] The git module can produce `FileChange`, `DiffContent`, `DiffHunk`,
      and `DiffLine` values without importing UI or DB types.
- [ ] The database module can work with file paths, diff hashes, and
      timestamps without importing git or UI types.
- [ ] `ReviewStatus` correctly models all three states.
- [ ] `FileEntry` combines git and review data into a single type.
- [ ] Server message types serialize to/from JSON correctly.
- [ ] `ConnectionContext` holds worktree, repo root, merge_base, and
      head_ref.

## Open Questions

- Should server message types be auto-generated from a schema, or
  hand-written? Hand-written is simpler for now.
- Should `DiffContent` include the raw diff string as well as the parsed
  structure, or only the parsed structure?
- Do we need a separate `FileContent` type for full-file mode, or is a
  `Vec<String>` (lines) sufficient?
