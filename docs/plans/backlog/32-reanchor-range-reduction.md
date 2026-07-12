# Stage 32: Reanchor Range Reduction

## Status

Backlog.

## Goal

When reanchoring a comment, make the new anchor range match the current source
shape instead of preserving a stale multiline range.

For example, if a reviewer leaves a multiline comment such as "delete these
lines" and the selected lines are later deleted, reanchoring should collapse
the comment marker to the deletion boundary. It should not keep drawing a large
range over unrelated current source.

## Why

The compound-anchor work decides whether each segment can still be located, but
the current range can still be too large after source edits. Context matching
can prove that the original selected text used to live between two surviving
context regions, but if the selected text is now gone, the useful current
location is the boundary where it disappeared, not the old line count.

This affects both unresolved and resolved comments:

- unresolved comments should remain unresolved unless explicitly resolved;
- resolved comments should remain resolved;
- both should display at the smallest current source range that represents the
  reanchored location.

## Core Rule

Reanchoring should choose the smallest faithful current range for each segment.

Priority order:

1. If the original selected text still matches exactly, use the exact selected
   text range.
2. If selected text is partially preserved, reduce the range to the preserved
   current source span.
3. If selected text was replaced, use the replacement/interior span between
   matched context boundaries.
4. If selected text was deleted and no replacement/interior remains, collapse
   to a single boundary line.
5. If there is not enough reliable evidence to identify the current location,
   mark the segment orphaned rather than anchoring to unrelated source.

The comment lifecycle state is not inferred from the body text. A comment body
that says "delete these lines" is not automatically resolved when the lines are
deleted. Resolution remains an explicit user or check-comments decision.

## Range Shape

Add an internal enum to describe why the current range has its size:

```rust
pub enum ReanchoredRangeShape {
    ExactSelection,
    ReducedSelection,
    ContextInterior,
    CollapsedBoundary,
}
```

This is primarily for tests and diagnostics. It does not need to be exposed in
the TUI immediately, but the server-side reanchor implementation should keep
the decision explicit.

If persisted, store it as a string mapped through enum conversion helpers. Do
not pass raw string literals through server, app, TUI, or MCP logic.

## Boundary Collapse Rules

When selected text is completely deleted:

- If both before and after context match and are now adjacent, collapse to the
  first line of the after context.
- If deletion happened at end of file and only before context remains,
  collapse to the last line of the before context.
- If deletion happened at start of file and only after context remains,
  collapse to the first line of the after context.
- If neither boundary is reliable, orphan the segment.

Collapsed ranges are still represented as normal one-line ranges because the
document model and gutter markers need a concrete row to render.

## Scenario Matrix

### 1. Whole Multiline Selection Deleted

Original:

```text
before
delete a
delete b
delete c
after
```

Comment segment covers `delete a` through `delete c`.

Current:

```text
before
after
```

Expected:

- segment anchors by context;
- range collapses to the current `after` line;
- `ReanchoredRangeShape::CollapsedBoundary`;
- comment resolved/unresolved state is unchanged;
- marker renders as a single-line marker.

### 2. Whole Selection Deleted At End Of File

Original:

```text
before
delete a
delete b
```

Current:

```text
before
```

Expected:

- segment anchors by context or boundary evidence;
- range collapses to the current `before` line;
- `CollapsedBoundary`;
- no phantom rows are created.

### 3. Whole Selection Deleted At Start Of File

Original:

```text
delete a
delete b
after
```

Current:

```text
after
```

Expected:

- segment anchors by context or boundary evidence;
- range collapses to the current `after` line;
- `CollapsedBoundary`.

### 4. Multiline Selection Replaced By One Line

Original:

```text
before
old a
old b
old c
after
```

Current:

```text
before
replacement
after
```

Expected:

- selected text no longer matches;
- before and after context identify a non-empty interior;
- range becomes the single `replacement` line;
- `ReanchoredRangeShape::ContextInterior`;
- marker renders as one line, not the old three-line range.

### 5. Multiline Selection Replaced By Fewer Lines

Original selection has five lines. Current source has two replacement lines
between the same reliable context boundaries.

Expected:

- range becomes the two current replacement lines;
- `ContextInterior`;
- marker length is two lines.

### 6. Multiline Selection Replaced By More Lines

Original selection has two lines. Current source has five replacement lines
between the same reliable context boundaries.

Expected:

- range becomes the five current replacement lines;
- `ContextInterior`;
- this is allowed because the current source shape grew.

### 7. Selection Partially Deleted

Original:

```text
before
keep a
delete b
keep c
after
```

Current:

```text
before
keep a
keep c
after
```

Expected:

- preserved selected text still identifies the current location;
- range reduces to `keep a` through `keep c`;
- `ReanchoredRangeShape::ReducedSelection`;
- marker spans only the surviving selected lines.

### 8. Selection Partially Rewritten

Original:

```text
before
keep a
old b
keep c
after
```

Current:

```text
before
keep a
new b
keep c
after
```

Expected:

- range covers `keep a` through `keep c`;
- `ReducedSelection` if preserved selected lines are the primary evidence;
- `ContextInterior` if context boundaries are the primary evidence;
- tests should assert the range, and may assert the chosen shape once the
  matcher decision is stable.

### 9. Selected Text Moved Elsewhere

Original selected text still exists unchanged but moved to another location in
the same file.

Expected:

- exact text matching wins over boundary collapse;
- range moves to the exact selected text;
- match method remains exact/shifted rather than context-collapse;
- no reduction happens merely because the old context no longer brackets it.

### 10. Duplicate Context With Deleted Selection

Original selected text is deleted, but the before/after context pair appears in
multiple places.

Expected:

- do not collapse to an arbitrary duplicate;
- use nearest reliable candidate only if existing reanchor confidence rules can
  choose one deterministically;
- otherwise mark the segment orphaned.

### 11. One Boundary Context Removed

Original selected text is deleted and only one side of context still exists.

Expected:

- if the remaining boundary is at file start or file end, collapse to that
  boundary as in scenarios 2 and 3;
- otherwise require enough additional evidence to avoid false anchoring;
- if evidence is insufficient, orphan the segment.

### 12. Compound Base/Head Segment With One Side Deleted

A replacement comment has both base and head segments. On reanchor, the base
segment's selected text is deleted, while the head segment still anchors
exactly.

Expected:

- base segment collapses or reduces according to these rules;
- head segment anchors normally;
- aggregate status remains `anchored` if both segments have current ranges;
- aggregate status becomes `partial` only if one segment is orphaned.

### 13. Both Sides Deleted

Both base and head segment selected texts are deleted and both sides have
reliable context boundaries.

Expected:

- each segment collapses independently;
- aggregate status remains `anchored`;
- the document model renders a compact marker for each visible side.

### 14. Resolved Comment Whose Selected Text Was Deleted

A resolved comment selected a multiline range. Later reanchor proves the
selected text was deleted.

Expected:

- lifecycle state remains resolved;
- anchor range collapses to the boundary;
- resolved comment display, if visible, uses the collapsed marker;
- no stale multiline range is preserved just because the comment is resolved.

### 15. Unresolved Comment Whose Selected Text Was Deleted

An unresolved comment selected a multiline range. Later reanchor proves the
selected text was deleted.

Expected:

- lifecycle state remains unresolved;
- anchor range collapses to the boundary;
- comment remains visible and counts as unresolved;
- `check_comments` semantics decide separately whether this should be treated
  as addressed.

## Implementation Notes

- Implement this in the server reanchor layer, not in TUI rendering.
- The app/document model should receive already-reduced segment ranges.
- Do not make the TUI infer collapsed ranges from context.
- Preserve append-only anchor version behavior: a reduction creates a new
  anchor version with reduced segment ranges.
- Store updated `anchor_text`, `context_before`, and `context_after` from the
  current source range when a segment anchors.
- For `CollapsedBoundary`, the new `anchor_text` should be the boundary line
  text, while diagnostics should make it clear that the original selected text
  disappeared.

## Tests

Add tests at the lowest layer that owns the behavior:

- pure range-decision tests for exact, reduced, interior, collapsed, and
  orphaned outcomes;
- server reanchor tests for every scenario above;
- DB anchor-version tests only if `ReanchoredRangeShape` is persisted;
- app/document tests only for rendering already-collapsed segment ranges;
- no TUI tests unless visual styling changes.

## Acceptance Criteria

- [ ] Fully deleted selected text collapses to a boundary row when context is
      reliable.
- [ ] Replaced selected text uses the current replacement/interior range.
- [ ] Partially preserved selected text reduces to the current surviving span.
- [ ] Exact moved selected text still wins over context collapse.
- [ ] Ambiguous deleted selections orphan instead of false anchoring.
- [ ] Compound base/head anchors reduce each segment independently.
- [ ] Resolved and unresolved lifecycle states are not changed by range
      reduction.
- [ ] Comment markers no longer preserve stale multiline ranges after the
      selected source has been deleted or shortened.
