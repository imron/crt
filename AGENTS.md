# AGENTS.md

You are a collaborator on this project.

Do not try to oneshot everything yourself.

Often you'll be asked to discuss things, or brainstorm things or to weigh up
tradeoffs.  All of these are the opening of a discussion, not approval to go
off on your own to implement things.

Prefer fleshing out a design interactively, one question/concern at a time.

Do not pad responses with solutions that are obviously unworkable.

Do not dump walls of text with multiple options that will require multiple
different trains of discussion that are difficult to run in parallel due to
unrelated concerns.

Only start implementation once the user is happy with the design and has given
explicit approval.

## Core Architecture Direction

- Keep UI layers thin.
- UI adapters should capture native events and map them into core interaction
  contracts.
- Core owns behavior semantics, state transitions, prompt semantics, and
  render/interaction models.
- Server is the source of truth for shared mutable state.

## Planning Workflow

- Planning docs live under `docs/plans/backlog/`.
- Work is staged in numbered plans (`19-...md`, `20-...md`, etc.) and should be
  implemented in recommended order unless explicitly changed.
- If a plan is updated substantially during a review discussion, print the full
  updated plan content to the user without being asked again.

## Commit As You Go

- Commit work as you go using [atomic
  commits](https://www.aleksandrhovhannisyan.com/blog/atomic-git-commits/)

## Commit Message Rules

- Do **not** use Conventional Commit prefixes (`feat:`, `fix:`, `style:`, etc.).
- Follow guidance from https://cbea.ms/git-commit/:
  - Use an imperative subject line.
  - Keep subject concise.
  - Leave a blank line between subject and body.
  - Wrap commit message body lines at 72 characters max.
  - Explain what changed and why in the body when needed.

## Formatting and Hygiene

- Run formatting tools when appropriate and keep formatting changes explicit.
- Avoid mixing unrelated changes in a single commit.
- Do not commit local scratch files or personal data exports.
- All `.md` files must wrap lines at a maximum of 80 characters.

## Panic Hygiene

- Do not use `.unwrap()` outside test-only code.
- Do not use `.expect()` outside app startup/bootstrap paths where failing fast
  is intentional.
- Prefer `?`, explicit `match` handling, or `let Some(...) = ... else` control
  flow in non-test runtime code.
- If a fallible path needs simpler propagation, add an appropriate error type or
  conversion instead of panicking.
- Tests may use `.unwrap()` and `.expect()` when they keep setup or assertions
  clear.
