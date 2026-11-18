# Stage 13k Tests: Compound Comment Anchors

## Status: Planned

## Goal

Define the test matrix for Stage 13k compound comment anchors. The suite should
cover anchor shape separately from Git history shape, and each behavior should
be tested at the layer that owns it.

These tests should be added immediately after the protocol structs and database
schema exist. They are expected to fail at first. Keeping the suite red before
implementing behavior is intentional; later implementation slices should turn
the tests green without weakening assertions or deleting coverage.

## Layered Testing Strategy

Use the narrowest layer that owns the behavior. Broader integration tests should
prove wiring across layers, not exhaustively repeat lower-layer cases.

### Protocol / Type Tests

Owns:

- enum serialization and deserialization,
- compound anchor request/response shape,
- rejection of impossible in-memory states.

Cover:

- `CommentAnchorSide`, placement status, match method, aggregate status,
- `CommentAnchor` with base only, head only, and paired segments,
- no string literals leaking beyond enum conversion tests.

### Database Tests

Owns:

- schema reset that deletes existing development comment data,
- enum-to-`TEXT` persistence mapping,
- anchor version and segment insert/list/read behavior,
- resolution event evidence storage.

Cover:

- storing base-only, head-only, and paired anchors,
- append-only anchor versions,
- aggregate status persistence,
- resolution patch-id and anchor snapshot persistence,
- old comment/anchor/resolution rows removed by migration.

### Re-Anchor / Server Tests

Owns:

- per-segment matching,
- aggregate status derivation,
- applicability and resolution evidence,
- API shape returned by `create_comment`, `list_comments`, and `get_comment`.

Cover:

- exact-at-line, exact-elsewhere, context, and not-found match methods,
- anchored, partial, and orphaned aggregate outcomes,
- base and head content lookup for current session, head checks, and range
  checks,
- commit ancestry, stable patch-id, and resolution-time anchor evidence.

### App Behavior Tests

Owns:

- deriving compound anchors from cursor/selection state,
- deciding which segment to navigate to,
- status messages when a comment is hidden in the current render mode.

Cover:

- unified diff selections,
- side-by-side selections,
- full-file base/head selections,
- hidden-comment navigation without render-mode changes.

### Document / Render-Model Tests

Owns:

- marker spans and visible rows from an already-built document model.

Stage 29 owns the generic document model and should test these behaviors at
the document layer first. Stage 13k should keep only the compound-anchor data
and app/server behavior tests that are unique to anchor semantics.

Cover:

- one marker spanning base/head segments in unified view,
- one marker spanning both columns in side-by-side view,
- full-file base/head filtering,
- same-start-line and nested compound anchors.

### MCP / CLI Tests

Owns:

- agent-facing and shell-facing check summaries,
- parameter mode handling,
- count/file output derived from shared check helpers.

Cover:

- `check_comments` head and range modes,
- `crt check`, `crt check HEAD`, `crt check --range BASE HEAD`,
- unresolved counts include partial/orphaned unresolved comments,
- operational failures remain distinct from unresolved feedback.

### End-To-End Tests

Owns:

- confidence that protocol, DB, server, App, TUI render model, and MCP/CLI
  wiring work together.

Cover a small representative subset:

- deleted-line comment survives rebase,
- replacement comment becomes partial,
- hidden comment navigation flashes status,
- resolved comment is ignored by `check_comments`.

## Anchor Shape Matrix

Cover these populated side-slot shapes:

1. `base` only:

   - deleted-line comment,
   - missing head segment is expected,
   - aggregate is based only on the base segment.

2. `head` only:

   - added-line comment,
   - missing base segment is expected,
   - aggregate is based only on the head segment.

3. paired `base` + `head`, same visual row:

   - unchanged context comment,
   - both sides resolve to the same row in unified diff,
   - marker span is one row.

4. paired `base` + `head`, base before head:

   - normal replacement in unified diff where deletions render before
     additions,
   - marker spans from first base row through last head row.

5. paired `base` + `head`, head before base:

   - lower-level marker/layout test if the renderer cannot produce this from a
     real diff,
   - marker span logic is order-independent.

6. paired `base` + `head`, different line numbers but same display span:

   - side-by-side replacement where the left and right line numbers differ,
   - marker represents one logical comment across both columns.

Every shape test should assert:

- stored segments and absent side slots,
- segment line coordinate spaces,
- aggregate status,
- marker span in unified and side-by-side views,
- navigation behavior when one side is hidden by the current render mode.

## History Scenario Matrix

For each anchor shape above, cover these history transitions where practical.
Each scenario must assert returned segments, match methods, aggregate status,
visibility, and 13j check behavior.

### 1. Same Branch Scope Change

All same-branch scenarios share this setup:

- CRT `merge_base` / `head` changes,
- branch lineage remains the original review lineage,
- later commits are descendants of the original review head.

#### 1.1 Same Branch, Code Unchanged

Setup:

- selected base/head content and context are unchanged.

Expected:

- populated segments re-anchor as `anchored`,
- match method is `exact_at_line` or `exact_elsewhere` depending on line
  movement,
- aggregate remains `anchored`,
- unresolved comment remains visible and counts in check results,
- direct commit ancestry is sufficient for resolution evidence.

#### 1.2 Same Branch, Start Context Changes

Setup:

- start context changes in later commits,
- selected text and end context remain usable.

Expected:

- selected text still anchors by `exact_at_line` or `exact_elsewhere`,
- aggregate remains `anchored`,
- changed start context is captured in the new anchor version,
- unresolved comment remains visible and counts in check results.

#### 1.3 Same Branch, End Context Changes

Setup:

- end context changes in later commits,
- selected text and start context remain usable.

Expected:

- selected text still anchors by `exact_at_line` or `exact_elsewhere`,
- aggregate remains `anchored`,
- changed end context is captured in the new anchor version,
- unresolved comment remains visible and counts in check results.

#### 1.4 Same Branch, Both Context Sides Change

Setup:

- start and end context both change in later commits,
- selected text remains usable.

Expected:

- selected text still anchors by `exact_at_line` or `exact_elsewhere`,
- aggregate remains `anchored`,
- both updated context sides are captured in the new anchor version,
- unresolved comment remains visible and counts in check results.

#### 1.5 Same Branch, Selected Base Text Changes

Setup:

- selected base text changes in later commits.

Expected:

- base segment anchors by `context` when surrounding context is sufficient,
- base segment becomes `orphaned`, `not_found` when context is insufficient,
- paired anchors become `partial` if the head segment remains anchored,
- unresolved partial/orphaned comments remain visible and count in checks.

#### 1.6 Same Branch, Selected Head Text Changes

Setup:

- selected head text changes in later commits.

Expected:

- head segment anchors by `context` when surrounding context is sufficient,
- head segment becomes `orphaned`, `not_found` when context is insufficient,
- paired anchors become `partial` if the base segment remains anchored,
- unresolved partial/orphaned comments remain visible and count in checks.

#### 1.7 Same Branch, Selected Base And Head Text Change

Setup:

- selected base and head text both change in later commits.

Expected:

- each segment evaluates independently,
- both segments anchoring by `context` yields aggregate `anchored`,
- one anchored and one orphaned yields aggregate `partial`,
- both orphaned yields aggregate `orphaned`,
- unresolved partial/orphaned comments remain visible and count in checks.

### 2. Rebase Onto New Base

#### 2.1 Rebase, Code Unchanged

Setup:

- `merge_base` and head commit both change,
- selected base/head content remains textually identical.

Expected:

- head segments re-anchor against rebased head,
- base segments re-anchor against the new applicable base/range content,
- unchanged text at moved lines uses `exact_elsewhere`,
- aggregate remains `anchored`,
- comment remains applicable to the rebased head.

#### 2.2 Rebase, Base Start Context Changes

Setup:

- base-side start context changes,
- selected base text remains usable.

Expected:

- base segment anchors by `exact_at_line` or `exact_elsewhere`,
- new start context is captured,
- paired anchor remains `anchored` if head segment anchors.

#### 2.3 Rebase, Base End Context Changes

Setup:

- base-side end context changes,
- selected base text remains usable.

Expected:

- base segment anchors by `exact_at_line` or `exact_elsewhere`,
- new end context is captured,
- paired anchor remains `anchored` if head segment anchors.

#### 2.4 Rebase, Both Base Context Sides Change

Setup:

- base-side start and end context both change,
- selected base text remains usable.

Expected:

- base segment anchors by `exact_at_line` or `exact_elsewhere`,
- both new context sides are captured,
- paired anchor remains `anchored` if head segment anchors.

#### 2.5 Rebase, Selected Base Text Changes

Setup:

- selected base text changes.

Expected:

- base segment anchors by `context` when context is sufficient,
- base segment becomes `orphaned`, `not_found` when context is insufficient,
- paired anchor becomes `partial` if the head segment remains anchored.

#### 2.6 Rebase, Head Start Context Changes

Setup:

- head-side start context changes,
- selected head text remains usable.

Expected:

- head segment anchors by `exact_at_line` or `exact_elsewhere`,
- new start context is captured,
- paired anchor remains `anchored` if base segment anchors.

#### 2.7 Rebase, Head End Context Changes

Setup:

- head-side end context changes,
- selected head text remains usable.

Expected:

- head segment anchors by `exact_at_line` or `exact_elsewhere`,
- new end context is captured,
- paired anchor remains `anchored` if base segment anchors.

#### 2.8 Rebase, Both Head Context Sides Change

Setup:

- head-side start and end context both change,
- selected head text remains usable.

Expected:

- head segment anchors by `exact_at_line` or `exact_elsewhere`,
- both new context sides are captured,
- paired anchor remains `anchored` if base segment anchors.

#### 2.9 Rebase, Selected Head Text Changes

Setup:

- selected head text changes.

Expected:

- head segment anchors by `context` when context is sufficient,
- head segment becomes `orphaned`, `not_found` when context is insufficient,
- paired anchor becomes `partial` if the base segment remains anchored.

#### 2.10 Rebase, Base And Head Start Context Change

Setup:

- start context changes on both sides,
- selected text remains usable on both sides.

Expected:

- both segments anchor by `exact_at_line` or `exact_elsewhere`,
- new start context is captured for both segments,
- aggregate remains `anchored`.

#### 2.11 Rebase, Base And Head End Context Change

Setup:

- end context changes on both sides,
- selected text remains usable on both sides.

Expected:

- both segments anchor by `exact_at_line` or `exact_elsewhere`,
- new end context is captured for both segments,
- aggregate remains `anchored`.

#### 2.12 Rebase, Base And Head Context Both Change

Setup:

- start and end context change on both sides,
- selected text remains usable on both sides.

Expected:

- both segments anchor by `exact_at_line` or `exact_elsewhere`,
- new context is captured for both segments,
- aggregate remains `anchored`.

#### 2.13 Rebase, Selected Base And Head Text Change

Setup:

- selected text changes on both sides.

Expected:

- each segment is evaluated independently,
- both segments anchoring by `context` yields aggregate `anchored`,
- one anchored and one orphaned yields aggregate `partial`,
- both orphaned yields aggregate `orphaned`.

#### 2.14 Rebase, Base Start Context Removed

Setup:

- base-side start context is removed,
- selected base text remains usable.

Expected:

- base segment anchors by exact text,
- new context snapshot records the missing start context,
- paired anchor remains `anchored` if head segment anchors.

#### 2.15 Rebase, Base End Context Removed

Setup:

- base-side end context is removed,
- selected base text remains usable.

Expected:

- base segment anchors by exact text,
- new context snapshot records the missing end context,
- paired anchor remains `anchored` if head segment anchors.

#### 2.16 Rebase, Base Context Removed

Setup:

- base-side start and end context are removed,
- selected base text remains usable.

Expected:

- base segment anchors by exact text,
- new context snapshot records missing context,
- paired anchor remains `anchored` if head segment anchors.

#### 2.17 Rebase, Selected Base Text Removed

Setup:

- selected base text is removed.

Expected:

- base segment anchors by `context` when context is sufficient,
- base segment becomes `orphaned`, `not_found` when context is insufficient,
- paired anchor becomes `partial` if head segment anchors.

#### 2.18 Rebase, Head Start Context Removed

Setup:

- head-side start context is removed,
- selected head text remains usable.

Expected:

- head segment anchors by exact text,
- new context snapshot records the missing start context,
- paired anchor remains `anchored` if base segment anchors.

#### 2.19 Rebase, Head End Context Removed

Setup:

- head-side end context is removed,
- selected head text remains usable.

Expected:

- head segment anchors by exact text,
- new context snapshot records the missing end context,
- paired anchor remains `anchored` if base segment anchors.

#### 2.20 Rebase, Head Context Removed

Setup:

- head-side start and end context are removed,
- selected head text remains usable.

Expected:

- head segment anchors by exact text,
- new context snapshot records missing context,
- paired anchor remains `anchored` if base segment anchors.

#### 2.21 Rebase, Selected Head Text Removed

Setup:

- selected head text is removed.

Expected:

- head segment anchors by `context` when context is sufficient,
- head segment becomes `orphaned`, `not_found` when context is insufficient,
- paired anchor becomes `partial` if base segment anchors.

### 3. Merge Equivalents

Setup:

- repeat rebase scenarios 2.1 through 2.21 through merge history.

Expected:

- direct resolution commit ancestry resolves comments,
- unresolved applicable comments re-anchor using the same segment rules,
- merge topology does not change anchor placement semantics.

### 4. Squash Equivalents

Setup:

- repeat rebase scenarios 2.1 through 2.21 through squash history.

Expected:

- direct commit ancestry may be unavailable,
- stable patch-id equivalence resolves comments when the resolving patch is
  present,
- resolution-time anchor evidence resolves only when it positively proves the
  resolved content state,
- unresolved comments remain unresolved when equivalence cannot be proven.

## Expected Re-Anchor Outcomes

For each content mutation:

- unchanged selected text at expected line -> `anchored`, `exact_at_line`,
- unchanged selected text at a new line -> `anchored`, `exact_elsewhere`,
- changed selected text with matching context -> `anchored`, `context`,
- removed selected text and insufficient context -> `orphaned`, `not_found`.

For compound anchors:

- all populated segments anchored -> aggregate `anchored`,
- some populated segments anchored and some orphaned -> aggregate `partial`,
- all populated segments orphaned -> aggregate `orphaned`.

Orphaned and partial unresolved comments must remain visible in comment lists
and must count as unresolved in 13j checks.

## Edge Cases

Each edge case should state the owning layer before implementation.

1. Duplicate selected text appears multiple times in the same file.

   Expected: exact-at-line wins when available; otherwise deterministic tie
   breaking chooses one location and records the match method.

2. Start context matches but end context does not.

   Expected: context match can still anchor only if the matcher's confidence
   threshold is met; otherwise the segment is orphaned.

3. End context matches but start context does not.

   Expected: same as start-only context mismatch.

4. Selected range crosses hunk boundaries.

   Expected: App creates side slots from selected base/head lines; renderer
   shows one marker spanning the resulting visual range.

5. File is renamed without content changes.

   Expected: rename-aware lookup resolves the current path before anchoring.
   The segment anchors against the renamed file path.

6. File is deleted on one side.

   Expected: segment for the deleted side is orphaned; paired anchors become
   `partial` if the other side anchors.

7. File is recreated with unrelated content.

   Expected: no false anchor; segment is orphaned unless context positively
   identifies the intended content.

8. Multiple comments share the same segment start line.

   Expected: all comments remain addressable; current-comment override drives
   marker highlight without dropping other marker spans.

9. A paired anchor has one side visible in the current render mode and the
   other side hidden.

   Expected: navigation uses the visible side; marker span includes only rows
   present in the current render model.

10. A hidden comment navigation attempt occurs.

    Expected: App keeps the current render mode and flashes a status message.
    TUI does not decide or switch views.

## Progress

- Initial test matrix split out from `13k-base-head-comment-anchors.md`.
- Test suite should be added as an intentionally failing red slice after
  protocol and database structure are in place.
