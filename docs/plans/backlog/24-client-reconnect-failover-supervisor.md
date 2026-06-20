# Stage 24: Client Reconnect and Failover Supervisor

## Status: Backlog

## Order

5 of 7 (recommended implementation order)

## Depends On

- Stage 19 (`19-shared-server-failover-lifecycle.md`)
- Stage 20 (`20-thin-client-api-contract.md`)
- Stage 21 (`21-protocol-type-decoupling.md`)
- Stage 23 (`23-tui-intent-only-input.md`)
- Stage 23g (`23g-remove-app-update.md`)

## Goal

Implement robust runtime reconnect and bind-race failover for clients when the
active server disappears.

## Why

Startup connect-or-start exists, but runtime loss recovery is not yet fully
enforced. Thin clients need resilient transport behavior without embedding
recovery logic in UI code.

## Scope

- In scope: reconnecting RPC wrapper/supervisor used by TUI and MCP.
- In scope: race-safe re-election using socket bind semantics.
- In scope: user-visible transient connection status messaging hooks.
- Out of scope: daemon mode and non-socket transports.

## Requirements

1. Add a transport supervisor that wraps request flow and classifies:
   - transport loss (EOF, reset, broken pipe),
   - server method errors,
   - serialization/protocol errors.

2. On transport loss:
   1. wait random jitter (50-200ms),
   2. attempt reconnect,
   3. on refusal, attempt embedded startup + bind,
   4. if bind race lost (`EADDRINUSE`), retry reconnect loop.

3. After successful reconnect, re-run `init` with original context and resume.

4. After reconnect and re-init, trigger Stage 20 resync semantics:
   - request/produce a fresh snapshot,
   - resume steady-state delta updates only after resync completes.

5. Retries remain unbounded while process is alive (per Stage 19 decisions).

6. Expose connection-state events through core interaction outputs/effects
   (not UI-framework-specific callbacks) so clients can show:
   - `Reconnecting...`,
   - `Reconnected`.

7. Ensure in-flight requests fail fast with transient errors while disconnected.

8. Define behavior for pending prompt/interaction state during disconnect:
   - stale prompt submissions are rejected safely,
   - client prompt UX is reset/revalidated after reconnect snapshot.

## Implementation Notes

- Keep supervisor separate from TUI event loop so GUI can reuse it unchanged.
- Ensure notification stream resumes after reconnect.
- Prefer idempotent reload-on-reconnect semantics for state consistency.
- Keep retry jitter configurable in tests for deterministic failover coverage;
  use fixed production defaults.

## Deliverables

- Reconnecting client wrapper module.
- Integration wiring for `crt <base>` and `crt mcp-server`.
- Connection-state signaling path to UI status layer.

## Acceptance Criteria

- [ ] Clients recover from active server death without restart.
- [ ] Exactly one recovering client wins bind in race scenarios.
- [ ] Other clients reconnect to winner automatically.
- [ ] Reconnect path performs snapshot resync before steady-state deltas resume.
- [ ] UI shows transient reconnect status and returns to normal.
- [ ] Connection-state notifications are exposed via core effects/events.
- [ ] Prompt/interaction state is safe across disconnect/reconnect boundaries.
- [ ] No regression in normal connected operation.

## Resolved Decisions

- Retry jitter is configurable for tests and fixed by default in production.
