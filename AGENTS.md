# AGENTS.md

This repository uses an agent-driven workflow. Follow these instructions when
making changes.

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
