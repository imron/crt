# crt

`crt` is a terminal code review tool for reviewing code that agents wrote.

You read the diff, select the lines that are wrong, and type what is wrong
with them. The agent picks those comments up over MCP — with the file, the
line range, and the exact code attached — fixes them, and marks them
resolved. Your view updates as it goes.

No pull request. No browser tab. No pasting snippets into a chat window,
and no describing a location in prose and hoping the agent finds it.

## The problem

Agents write code far faster than you can read it, so review is now the
bottleneck. The usual ways of reviewing make that worse:

- **Feedback lands in the wrong place.** You read code in one window and
  describe the problem in another. "In the worktree resolution function,
  the second branch" is a poor way to point at line 214.
- **Nothing is machine-readable.** The agent has to reconstruct from your
  prose what you were looking at, and it regularly fixes the wrong thing.
- **You have to push before you can review.** Opening a pull request to
  review your own branch costs a push, a CI wait, and a context switch.
- **You lose your place.** Agents amend, squash, and rebase constantly.
  Anything that tracks "files I have already read" by commit or by line
  number discards that on the first rebase and makes you start over.

## The review loop

Start a review of everything on your branch:

```sh
crt main
```

You get a two-pane TUI: changed files on the left, the diff on the right.
Work through it:

- Read a file, then press `a` to mark it reviewed.
- Press `c` on a line to comment on it, or press `V` to select a range of
  lines first. Type the comment and it is attached to that code.
- Press `?` for the full keybinding list.

Now point your agent at the review. Over MCP it calls:

1. `list_review_sessions`, then `select_review_session` to attach to the
   review you are running.
2. `list_review_comments` to see what you flagged, and
   `get_comment_detail` for the exact lines and surrounding context.
3. `get_file_diff`, `search_codebase`, and `find_definition` to work out
   what the fix should be.
4. `resolve_comment` once it has made the change.

Your TUI updates as the agent resolves comments — the server pushes the
change to every connected client. Files whose diff actually changed
reappear as needing another look. Everything else stays reviewed.

## Comments survive rebasing

This is what keeps the loop usable in practice.

Comments are anchored to the **content** they were attached to, not to a
line number. Each comment stores the exact code it covers plus the lines
around it. When the file changes, `crt` looks for that content again:

- Found where it was: the comment stays put.
- Found elsewhere: the comment moves with the code.
- Found approximately, via the surrounding context: the comment follows,
  flagged as approximate.
- Not found at all: the comment is kept and marked orphaned, with its
  last known context, rather than silently disappearing.

Reviewed state works the same way. `crt` records a hash of each file's
diff when you mark it reviewed. After a rebase, files whose diff is
unchanged stay reviewed; only files that genuinely changed come back.

The practical effect: your agent can rewrite history under you and you do
not lose the review.

## Working while the agent works

Review state is keyed on the merge base and the branch, not on a
directory. So the agent can work in a `git worktree` while you review from
the main working tree, and you are both looking at the same review.

Comments, resolutions, and reviewed state are shared live. `crt main` and
`crt <that commit hash>` resolve to the same review, so you and the agent
do not have to agree on how to spell the base ref.

## Agents reviewing agents

Reviewing agents get the same tools you do. An agent can call
`create_review_comment` to attach a comment to a line range, and
`mark_file_reviewed` as it works through the diff.

That makes the reviewer replaceable: one agent reviews the branch and
leaves comments, another reads them, fixes the code, and resolves them —
and you can open the same review in the TUI at any point to see both
sides of that conversation.

## Review markers in source files

Not every agent speaks MCP. For those, `crt` can write your comments
directly into the source files in your worktree, using each file's own
comment syntax:

```rust
// <<<<<<< REVIEW #42
fn process(input: &str) -> Result<Output> {
    let parsed = parse(input);
}
// =======
// Does parse() handle empty input? If input is "", this
// will silently produce a default value.
// >>>>>>> REVIEW #42
```

The file still compiles, the feedback is right next to the code, and the
comment id lets an agent resolve it later. Applying and clearing markers
is idempotent, so you can round-trip freely.

*Not implemented yet — see Status below.*

## Getting started

Build it and put it on your `PATH`:

```sh
cargo build --release
cp target/release/crt ~/bin/crt
```

Then review a branch:

```sh
crt main          # review main..HEAD
crt --root        # review everything back to the first commit
crt main --reset  # throw away review state for this branch
```

To let an agent join the review, add `crt` as an MCP server. For example:

```json
{
  "mcpServers": {
    "crt": {
      "command": "/path/to/crt",
      "args": ["mcp-server"],
      "env": {
        "CRT_SERVER_HOST": "127.0.0.1",
        "CRT_SERVER_PORT": "25175"
      }
    }
  }
}
```

`crt mcp-server` connects to a running `crt` server rather than starting
one, so have a TUI session open — or run `crt server` for a persistent one
— before the agent connects.

Review state lives in `.crt/reviews.db` in your repository root. It is
local and personal, so add `.crt/` to your `.gitignore`.

## Status

Alpha. It is used daily, but the interface and the stored data format can
still change without a migration path.

Planned:

- A native GUI alongside the TUI.
- Better comment span behaviour: spans do not always expand or collapse
  intuitively when lines are added or deleted inside them.
- Review markers in source files, for agents without MCP.

## Learn more

`docs/OVERVIEW.md` covers the architecture, the full keybinding and
command reference, the server API, and the design decisions behind the
storage and anchoring models.
