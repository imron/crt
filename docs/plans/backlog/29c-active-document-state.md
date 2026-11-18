# 29c: Active Document State

## Status

Backlog.

## Overview

This slice is part of the Stage 29 diff document model. Read
`29-diff-document-model.md` first for the full architecture, naming, layering,
and migration strategy.

## Goal

Move the expensive active diff document projection into `AppState` as
`ActiveDocument`, keyed by `DocumentKey`, so cursor movement and ordinary
rendering no longer rebuild structural diff rows.

## Scope

Add active document state:

```rust
pub struct AppState {
    // existing raw state...
    pub active_document: Option<ActiveDocument>,
}
```

Add app-state document lifecycle methods:

```rust
impl AppState {
    pub fn rebuild_active_document(&mut self);
    pub fn ensure_active_document(&mut self);
    pub fn refresh_document_overlays(&mut self);
    pub fn current_document_key(&self) -> Option<DocumentKey>;
}
```

The exact method names can change, but responsibilities should remain clear:

- key construction decides whether structural rebuild is needed;
- structural rebuild creates `ActiveDocument`;
- overlay refresh updates cursor/search/selection/comment state;
- callers do not manually patch document internals.

## Invalidation Rules

Rebuild `DiffDocument` when `DocumentKey` changes:

- selected file changes,
- diff hash changes,
- content mode changes,
- render variant changes,
- diff algorithm changes,
- whitespace mode changes,
- base/head content identity changes.

Do not rebuild structural rows for:

- cursor movement,
- scroll movement,
- selected/current comment changes,
- comment body/resolved changes,
- search query changes,
- visual selection changes,
- `show_blame` toggles.

When an incremental overlay update is unclear, rebuilding from raw app state is
allowed. The raw `AppState` fields remain authoritative.

## AppModel Changes

`App::model()` should become a cheap snapshot over the active document:

```rust
pub struct DiffPanel {
    pub document: Option<DiffDocument>,
    pub scroll: usize,
    pub cursor: DocumentCursor,
    pub show_blame: bool,
}
```

The implementation may use clone, reference, or `Arc` based on Rust lifetime
constraints. The architectural requirement is that `AppModel` does not rebuild
the full document on every render.

## Update Flow

Update paths should call app-state document lifecycle methods instead of
assuming rendering will rebuild projection state.

Examples:

- file selection calls `rebuild_active_document`;
- view-mode changes call `rebuild_active_document`;
- cursor movement updates `DocumentCursor` overlay;
- comment reload refreshes comment overlays;
- search updates refresh search overlays;
- selection updates refresh selection overlays.

## Tests

Add tests for:

- active document is built after initial file load.
- selecting a different file changes `DocumentKey` and rebuilds.
- switching render mode changes `DocumentKey` and rebuilds.
- cursor movement does not change `DocumentKey`.
- scroll movement does not change `DocumentKey`.
- search changes do not change `DocumentKey`.
- comment resolve/unresolve does not change `DocumentKey`.
- `show_blame` does not change `DocumentKey`.
- `App::model()` returns the existing active document projection.

## Acceptance Criteria

- [ ] `AppState` owns `active_document`.
- [ ] `DocumentKey` controls structural rebuilds.
- [ ] Cursor movement does not rebuild structural document rows.
- [ ] `AppModel` exposes the active document instead of rebuilding it.
- [ ] Raw app state remains authoritative.
- [ ] Existing user-visible behavior remains unchanged.
