# Stage 26: Thin Client Hardening and Tests

## Status: Complete

## Order

7 of 7 (recommended implementation order)

## Depends On

- Stage 20 (`20-thin-client-api-contract.md`)
- Stage 21 (`21-protocol-type-decoupling.md`)
- Stage 22 (`22-extract-domain-usecases-from-tui.md`)
- Stage 23 (`23-tui-intent-only-input.md`)
- Stage 23k (`23k-thin-tui-module-splitting.md`)
- Stage 24 (`24-client-reconnect-failover-supervisor.md`)
- Stage 25 (`25-embedded-lifecycle-alignment.md`)

## Goal

Harden the new thin-client architecture with service-level tests, failover
integration tests, and regression coverage.

## Why

Extraction and failover changes alter core behavior paths. Without explicit
coverage, regressions in review workflow, reconnect logic, and multi-client
state are likely.

## Scope

- In scope: unit tests for extracted services.
- In scope: integration tests for reconnect/failover/takeover behavior.
- In scope: regression tests for core review UX invariants.
- Out of scope: performance benchmarking beyond sanity checks.

## Requirements

1. Add service-level tests for extracted domain behavior:
   - review status transitions,
   - sorting/selection advance,
   - effective diff base policy,
   - navigation line mapping and hunk jumps.

2. Add reconnect/failover integration tests:
   - two clients share one server,
   - kill active server and verify automatic recovery,
   - simultaneous recovery race with one bind winner.

3. Add lifecycle tests:
   - embedded host exit kills embedded server,
   - peer takeover continues session.

4. Add multi-repo integration scenario under shared server.

5. Add regression checks for notification continuity and state reload after
   reconnect.

6. Ensure CI-friendly determinism:
    - test-specific jitter overrides where needed,
    - robust waiting and timeout strategy.

7. Add contract tests for Stage 20 interaction model:
   - `InputEvent` handling paths for key/prompt/pointer flows,
   - prompt handshake correctness (`RequestPrompt` -> submit/cancel -> effects),
   - `AppModel` plus backend-owned hit map consistency for pointer target
     mapping.

8. Add reconnect-state tests for Stage 24 core effects/events:
   - `Reconnecting...` and `Reconnected` signaling,
   - snapshot resync before steady-state delta processing.

9. Add boundary regression tests or static checks for Stage 23h-23k:
   - TUI runtime does not own the raw client,
   - TUI runtime does not execute review/search/definition workflows directly,
   - TUI state remains limited to terminal/render/prompt/pointer concerns.

## Implementation Notes

- Reuse existing server test harness patterns in `tests/server_test.rs`.
- Keep failure diagnostics explicit (which client won bind, retries attempted,
  reconnect duration).
- Prefer black-box tests around public behavior over internal task timing.

## Deliverables

- New/expanded unit and integration tests.
- Test helper utilities for controlled failover scenarios.
- Documentation of manual verification steps for local smoke testing.

## Acceptance Criteria

- [x] All new thin-client services have focused behavioral unit tests.
- [x] End-to-end failover scenarios pass consistently.
- [x] Multi-client/multi-repo behavior is verified under shared server.
- [x] Input/prompt/pointer interaction contracts are covered by tests.
- [x] Reconnect status and snapshot-before-delta behavior are covered by tests.
- [x] No regressions in existing server protocol tests.
- [x] Test suite is stable across repeated runs.

## Resolved Decisions

- Long-running chaos-style failover tests run locally/manual (or optional
  nightly), not as blocking default CI checks.
