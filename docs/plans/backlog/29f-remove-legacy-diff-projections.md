# 29f: Remove Legacy Diff Projections

## Status

Backlog.

## Overview

This slice is part of the Stage 29 diff document model. Read
`29-diff-document-model.md` first for the full architecture, naming, layering,
and migration strategy.

## Goal

Remove obsolete diff row, comment projection, and TUI semantic reconstruction
paths after the document model owns rendering and interactions.

This is the cleanup slice. It should happen only after 29a-29e are green.

## Scope

Remove or shrink legacy structures that are no longer part of the active model:

- old active `DiffPanel` row fields that duplicate `DiffDocument`;
- TUI code that builds semantic rows from hunks;
- TUI-owned hunk row projections;
- TUI-owned rendered text as an app interaction dependency;
- transitional comment projection APIs used by active rendering;
- `AppViewport` methods that expose TUI-rendered semantic state to app update
  code.

Some low-level row-building helpers may remain if document builders use them
internally. The cleanup target is duplicate ownership, not necessarily every
helper function.

## Design Requirements

- `AppState` raw fields remain authoritative.
- `ActiveDocument` is the app-owned projection for the active diff view.
- `AppModel` exposes document state cheaply.
- TUI renders `RenderLine`.
- App update code uses document APIs for document interactions.
- TUI does not expose rendered text or hunk rows back to app semantics.

## Tests

Add regression checks for removed dependency paths:

- app update tests use a document-backed viewport or no viewport where
  possible.
- TUI tests assert rendering uses `RenderLine`.
- no active update path calls TUI-rendered text for search.
- no active update path calls TUI hunk rows for hunk navigation.
- no active renderer path calls old comment projection marker lookup.
- side-by-side still renders base document left and head document right.
- all 13k, 13l, 13m, and 29-series behavior remains green.

## Acceptance Criteria

- [ ] Obsolete active diff row fields are removed from `DiffPanel`.
- [ ] App update code no longer depends on TUI-rendered text.
- [ ] App update code no longer depends on TUI hunk row projections.
- [ ] Active rendering no longer uses transitional comment projection APIs.
- [ ] TUI diff renderer is a thin document renderer.
- [ ] All tests pass.
