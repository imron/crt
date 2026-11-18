# Stage 11: Navigation and Search

## Goal

Implement go-to-definition (`Ctrl-]` / `Ctrl-t`), codebase search (`:gr`),
and the command mode infrastructure. All search operations go through the
server API.

## Why

Code review requires understanding context. When reviewing a diff, you
frequently encounter symbols whose definitions you need to see. Codebase
search provides a broader way to find related code. These features
transform the tool from a diff viewer into a review environment.

## Requirements

### Go-to-Definition

1. **`Ctrl-]`**: identify the word under the cursor in the diff view and
   send `find_definition(symbol, context_file)` to the server.

2. **Server-side search**: the server uses the `ignore` and `regex` crates
   to search the worktree for definition patterns:
   - `fn <symbol>`, `struct <symbol>`, `enum <symbol>`, `trait <symbol>`,
     `type <symbol>`, `impl <symbol>`, `const <symbol>` (Rust)
   - `function <symbol>`, `class <symbol>`, `const <symbol>`,
     `interface <symbol>` (TypeScript/JavaScript)
   - `def <symbol>`, `class <symbol>` (Python)
   - `func <symbol>`, `type <symbol>` (Go)
   - Additional patterns for other common languages.

3. **Trait-based design**: the definition-finding logic is behind a trait
   (`DefinitionFinder`) so the backend can be replaced with tree-sitter
   or LSP in the future.

4. **Result handling**: single result → navigate directly. Multiple
   results → show a picker/popup.

5. **Navigation to result**: display the file in the diff pane. If the
   file is in the diff, show the diff scrolled to the relevant line. If
   not in the diff, show the file content read-only (via `get_file_content`
   from the server).

6. **`Ctrl-t`**: jump stack. Each `Ctrl-]` pushes the current location
   (file + scroll position). `Ctrl-t` pops and returns. Arbitrary depth.

### Codebase Search

7. **Command mode**: `:` opens a command input line at the bottom of the
   screen. `Enter` executes. `Escape` cancels.

8. **`:gr <regex>`**: send `search_codebase(pattern, scope: "all")` to
   the server. Results shown in an overlay/popup: file path, line number,
   matching line with match highlighted.

9. **`:grd <regex>`**: same but `scope: "diff"` — restricted to files in
   the current diff.

10. **Result navigation**: `j`/`k` navigate results, `Enter` jumps to
    result, `Escape`/`q` closes results.

11. **`:q`**: quit the application.

### Shared Concerns

12. **Gitignore respect**: searches respect `.gitignore` (the `ignore`
    crate handles this).

13. **Performance**: searches should be fast enough for interactive use.
    Blocking is acceptable for v1 if it completes within a reasonable
    time.

## Acceptance Criteria

- [x] `Ctrl-]` on a symbol finds and navigates to its definition.
- [x] Multiple definition matches show a picker.
- [x] `Ctrl-t` returns to the previous location. Multiple levels work.
- [x] Navigating to a file not in the diff shows a status message (read-only file view deferred).
- [x] `:` opens a command input line.
- [x] `:gr <regex>` searches the codebase and shows results.
- [x] `:grd <regex>` searches only files in the diff.
- [x] Search results can be navigated and jumped to.
- [x] `:q` exits the application.
- [x] Searches respect `.gitignore`.
- [x] The definition finder is behind a trait.

## Open Questions

- How should we detect file type for choosing definition patterns? By file
  extension? What about extensionless files?
  → **Decision (v1)**: all patterns are tried regardless of extension.
- Should search results persist (stay open as a panel) or close after
  jumping to a result?
  → **Decision**: results close after jumping (Enter). Can reopen with another search.
- Should `:gr` results show context lines (1-2 lines above/below the
  match)?
  → **Deferred**: v1 shows single matching line only.
- Should command mode support history (up/down to recall previous
  commands)?
  → **Deferred**: not in v1.
