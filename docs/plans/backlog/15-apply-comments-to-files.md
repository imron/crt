# Stage 15: Apply Comments to Files

## Goal

Write review comments into source files in the worktree as inline markers
using the `<<<<<<< REVIEW` / `=======` / `>>>>>>> REVIEW` format, and
provide the inverse operation to remove them. This produces files that any
agent can read to see review feedback, regardless of MCP support.

## Why

Not all agents have MCP access. Some agents work by simply reading source
code files. By writing review comments directly into the worktree files
using the native comment syntax of each language, any agent — regardless
of tooling capabilities — can see the feedback by reading the code.

This is a complementary channel to the MCP adapter. The MCP adapter is
the structured, preferred interface; apply-to-files is the universal
fallback.

## Requirements

### Comment Marker Format

1. **Marker structure**: comments are written into source files wrapping
   the original code being commented on. Each marker includes the
   comment's database ID (`#<id>`) on both the opening and closing lines,
   enabling agents to reference specific comments (e.g. to call
   `resolve_comment` via MCP) and enabling idempotent operations:
   ```
   {prefix} <<<<<<< REVIEW #<id>
   <the original code lines the comment is attached to>
   {prefix} =======
   {prefix} <the review comment body, line by line>
   {prefix} >>>>>>> REVIEW #<id>
   ```
   Where `{prefix}` is the file's native comment syntax and `<id>` is
   the comment's integer ID from the database.

2. **Language-aware comment syntax**: markers must use the correct comment
   syntax for the file's language so the file remains syntactically valid.
   Detection is based on file extension. At minimum:

   | Style        | Languages                                             |
   | ------------ | ----------------------------------------------------- |
   | `//`         | Rust, JavaScript, TypeScript, C, C++, Java, Go, Swift, Kotlin, Scala, Dart, PHP |
   | `#`          | Python, Shell, Bash, Ruby, Perl, TOML, YAML, Dockerfile, Makefile |
   | `--`         | SQL, Lua, Haskell                                     |
   | `<!-- -->`   | HTML, XML, Markdown, SVG, Vue                         |
   | `/* */`      | CSS, SCSS, LESS                                       |
   | `"`          | Vim script                                            |
   | `;`          | Lisp, Clojure, Scheme                                 |
   | `#` fallback | Any unrecognized file extension                       |

3. **Block comment languages** (HTML, CSS): the marker format must adapt
   to use block comment syntax correctly so delimiters balance properly.

4. **Multi-line comment bodies**: each line prefixed with the comment
   syntax individually.

5. **Indentation**: markers match the indentation level of the code they
   wrap.

### Apply Operation

6. **`crt apply-comments <base>`**: CLI subcommand that connects to the
   server, retrieves all unresolved comments, and writes markers into
   the worktree files. Only unresolved comments are applied.

7. **TUI command**: `:apply-comments` does the same from within the TUI.

8. **Server interaction**: the apply operation sends `apply_comments` to
   the server. The server retrieves comments, performs anchor resolution,
   and returns the comments with their resolved positions. The client
   (or server) writes the markers to the worktree files.

9. **Idempotency**: applying twice must not produce duplicate markers.
   Check for existing markers with matching comment IDs before inserting.

10. **File writing**: read each affected worktree file, insert markers
    at correct positions (bottom-up to preserve line numbers), write back.

### Clear Operation

11. **`crt clear-comments <base>`**: CLI subcommand that removes all
    `<<<<<<< REVIEW` marker blocks from worktree files.

12. **TUI command**: `:clear-comments` does the same from within the TUI.

13. **Marker parsing**: correctly identify and remove complete marker
    blocks regardless of comment syntax style. Partial/corrupted markers
    reported as warnings.

14. **Restoration**: after clearing, files should be identical to their
    pre-apply state.

### Integration

15. **Applied status tracking**: the server records which comments have
    been applied (supports idempotency and TUI display).

16. **Warning on review launch**: if markers are detected in the worktree,
    warn the user. They will appear in the diff and could be confusing.

17. **Worktree awareness**: markers are always written to the worktree
    path from the connection context, not the main repo working tree.

## Acceptance Criteria

- [ ] `crt apply-comments main` writes markers into worktree files for
      all unresolved comments.
- [ ] Markers use the correct comment syntax per language (tested with
      at least `//`, `#`, `--`, and `<!-- -->`).
- [ ] A Rust file with markers still compiles (`cargo check` passes).
- [ ] A Python file with markers still parses.
- [ ] `crt clear-comments main` removes all marker blocks.
- [ ] Files are byte-identical to pre-apply state after clearing.
- [ ] Applying twice does not produce duplicate markers.
- [ ] Marker format includes `#<id>` on open and close lines.
- [ ] An agent can read the `#<id>` and call `resolve_comment(<id>)`.
- [ ] Indentation matches the wrapped code.
- [ ] Multi-line bodies are correctly prefixed.
- [ ] Orphaned comments are skipped with a warning.
- [ ] `:apply-comments` and `:clear-comments` work from the TUI.
- [ ] Markers are written to the worktree, not the main repo working tree.
- [ ] `crt <base>` warns if markers are detected.

## Open Questions

- Should `clear-comments` also resolve the comments in the DB (assuming
  the agent has addressed them), or just remove the markers?
- Should there be a `crt apply-comments --file <path>` to apply markers
  to a single file?
- How should the apply operation handle files that have been modified
  in the worktree but not committed? Should it warn or refuse?
- Should the marker format include a timestamp or just the ID?
- For block-comment languages (HTML, CSS), should the entire marker
  block be wrapped in one block comment, or should each line use
  separate comments?
