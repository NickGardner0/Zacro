# Architecture and design

[Project overview](../../../README.md) · [Development](development.md) · [Status](status.md)

## Product goal

Zacro explores a native desktop interface for connected work: a conversation
leads to a task, a task references a document, and an agent works with the same
context. The client brings these workflows into a shared window, navigation
model, and editing stack.

The project reimplements Macro's desktop experience in Rust/GPUI while using
Macro's service contracts. Its scope is the desktop client. Macro continues to
provide authentication, storage, search, email, agent execution, and other
backend capabilities.

## System boundaries

```text
Zacro desktop application
  ├── GPUI views + Zed UI, editor, themes, and platform components
  ├── Layout, list interaction, and shared entity state
  ├── Typed HTTP clients + realtime event reconciliation
  └── Collaborative documents/canvas + local recovery
                │
                └── Macro services: auth, storage, mail, agents, sync, ...
```

[`macro-app`](../macro-app/src/macro_app.rs) composes these layers and contains
most feature views and orchestration. The supporting crates isolate several
important models, transports, and editing operations, but the application layer
is still large. This is the current structure, rather than the more granular
feature-crate architecture proposed in the original roadmap.

## Engineering decisions

### Reuse a native framework and editor

GPUI and Zed's components supply rendering, input handling, text editing, themes,
and platform behavior. Reusing that stack makes text selection, clipboard,
keyboard focus, and IME part of the same application model.

**Tradeoff:** Zacro depends on internal Zed crates and remains in the Zed
workspace. Extracting a small standalone workspace requires auditing transitive
dependencies and preserving upstream compatibility. The retained upstream tree
is dependency infrastructure; Zacro's own product work is under `crates/macro-gpui`.

### Integrate with existing service contracts

[`macro-api`](../macro-api/src/macro_api.rs) centralizes transport and request /
response models. UI workflows call those services instead of reproducing server
business logic. Credentials use the platform keychain.

**Tradeoff:** the service boundary is a moving dependency. Changes to provider
types, response shapes, events, and permissions can break a client even when its
local tests pass. The pinned reference and fixture corpus need regular updates.

### Share state across workflows

[`macro-store`](../macro-store/src/macro_store.rs) provides reusable collection
and normalized-state primitives. [`macro-realtime`](../macro-realtime/src/macro_realtime.rs)
classifies events for targeted reconciliation. Tabs and views share application
state so related entities can update without rebuilding the whole workspace.

**Tradeoff:** optimistic edits, permissions, account switching, and delayed
responses need explicit ownership and invalidation. Correctness depends on
exercising combinations of those transitions, not only individual request paths.

### Preserve collaborative document structure

[`macro-sync`](../macro-sync/src/macro_sync.rs) implements the Loro/Bebop transport,
awareness, snapshots, and persistence. [`macro-editor`](../macro-editor/src/macro_editor.rs)
connects the native editing representation to Macro's document structure, with
handling for unknown nodes and concurrent changes.

**Tradeoff:** a document that looks correct locally can still lose structure or
behave differently in another client. Snapshot fixtures and concurrent-edit
tests provide local evidence; service-backed native/web sessions remain a
separate acceptance test.

### Keep validation inspectable

The repository includes contract fixtures, deterministic GPUI captures with
accessibility metadata, performance runners, and packaging checks. Each answers
a different question: schema compatibility, rendering, interaction behavior,
latency, or distribution integrity.

**Tradeoff:** checklist and source-marker validation can catch missing plumbing,
but cannot prove that a user can finish a workflow. The [status page](status.md)
keeps these distinctions explicit.

## Next structural work

1. Refresh the consumed Macro contracts and feature inventory against a new
   reference revision before expanding parity claims.
2. Extract feature controllers from `macro-app` as complete workflows gain tests,
   keeping service concerns out of reusable UI components.
3. Build matched native/web acceptance scenarios for mail, document editing,
   messaging, and account switching, including disconnect and permission changes.
4. Evaluate workspace extraction after the native dependency boundary is documented.
