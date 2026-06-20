# Stage 21: Protocol Type Decoupling

## Status: Complete

## Order

2 of 7 (recommended implementation order)

## Depends On

- Stage 20 (`20-thin-client-api-contract.md`)

## Goal

Remove layer leaks between client and server modules by introducing neutral
shared protocol types.

## Why

Today `src/client.rs` depends on server internals for notification typing.
That coupling blocks clean layering and makes future interfaces harder to add.

## Scope

- In scope: notification/request/response shared type placement and imports.
- In scope: client/server compile-time separation of module ownership.
- In scope: clear separation between transport protocol types and core
  interaction/render types from Stage 20.
- Out of scope: behavior changes in API semantics.

## Requirements

1. Move shared protocol types to the neutral `protocol` module consumed by
   both client and server.

2. Remove `client -> server::*` imports for type aliases and message models.

3. Ensure server dispatch and client decoding both use the same shared types.

4. Backward compatibility of the wire format is not required in this stage;
   coordinated protocol changes are allowed while in development.

5. Add compile-time boundaries in module docs to enforce:
   - client may depend on protocol/core,
   - server may depend on protocol/core,
   - client may not depend on server module internals.

## Implementation Notes

- Prefer clear, simplified field/type naming even if that requires coordinated
  renames across client and server.
- If renames are applied, update both client and server in the same stage so
  the project remains buildable/runnable end-to-end.
- Update comments to reflect ownership (shared vs server-local).
- Keep transport protocol models separate from core `InputEvent` and
  AppModel-backed interaction contracts.

### Naming Guidance

- Favor concise, unambiguous protocol names over preserving legacy names.
- Avoid leaking server-internal terminology into shared protocol types.
- Keep JSON field names stable only when clarity is not harmed; otherwise
  prefer clarity and update both ends together.

## Deliverables

- New/updated shared protocol module with notifications and RPC payload models.
- Updated imports in `src/client.rs`, `src/server/mod.rs`, `src/server/api.rs`.
- Brief architecture note documenting allowed dependencies.

## Acceptance Criteria

- [x] `src/client.rs` has no dependency on `src/server/*` for shared types.
- [x] Client and server compile and run together with the updated protocol.
- [x] Existing tests are updated as needed and pass.
- [x] Notification handling works identically from client perspective.
- [x] Module-level docs clearly define dependency direction.

## Resolved Decisions

- Protocol types move to a dedicated `protocol` module (not `model`) to keep
  transport concerns separate from core/domain and UI interaction contracts.
- Backward wire compatibility is explicitly non-goal during this dev stage.
