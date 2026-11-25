# Zacro

**A native desktop workspace, built with Rust and GPUI.**

Zacro brings mail, messages, documents, tasks, and agents into a keyboard-driven
desktop application. It is an independent reimplementation of
[Macro](https://github.com/macro-inc/macro)'s desktop experience, using
[GPUI](https://www.gpui.rs/) and Zed's editor and UI components to connect to
Macro's existing services.

[Architecture](crates/macro-gpui/docs/architecture.md) ·
[Development](crates/macro-gpui/docs/development.md) ·
[Project status](crates/macro-gpui/docs/status.md) ·
[Contributing](CONTRIBUTING.md)

## The project

Work moves between conversations, documents, and decisions. Zacro explores how
that connected workflow can feel in a native desktop client: persistent tabs and
splits, fast keyboard navigation, and an editor that shares the same input and
rendering stack as the rest of the application.

The work spans the interface and the systems behind it:

- **A native workspace.** Resizable panes, persistent tabs, command navigation,
  virtualized lists, and system-aware light and dark themes.
- **Connected workflows.** Service-backed mail, channel messages, tasks, CRM,
  files, agent sessions, and calls, with shared entity links and permissions.
- **Collaborative editing.** A Rust document and canvas implementation, with
  Loro/Bebop synchronization, local persistence, history, and reconnect handling.
- **A validation harness.** Contract fixtures, deterministic GPUI renders,
  accessibility-tree checks, interaction measurements, and macOS packaging.

## Run locally

The development target is **macOS**. Install Rust through rustup, full Xcode with
its macOS components, and CMake. The repository pins its Rust toolchain.

```sh
git clone https://github.com/NickGardner0/Zacro.git
cd Zacro
cargo run --release
```

The default workspace target is Zacro. The explicit command is
`cargo run -p macro_app --release`. The first build compiles the native dependency
graph; subsequent launches reuse Cargo's cache.

Sign in with a Macro account to use connected workflows. Production Macro
services are selected by default, and session credentials use the macOS
keychain. See the [development guide](crates/macro-gpui/docs/development.md) for
debug builds, environment selection, fixture captures, and application bundles.

## Inside the repository

Zacro's product code lives in [`crates/macro-gpui`](crates/macro-gpui). The
surrounding Zed workspace supplies the native framework, editor, UI components,
and platform integrations it depends on.

| Layer | Source |
| --- | --- |
| Application, navigation, and feature views | [`macro-app`](crates/macro-gpui/macro-app) |
| Service contracts and domain models | [`macro-api`](crates/macro-gpui/macro-api), [`macro-models`](crates/macro-gpui/macro-models) |
| State, caching, and realtime events | [`macro-store`](crates/macro-gpui/macro-store), [`macro-realtime`](crates/macro-gpui/macro-realtime) |
| Documents, canvas, and collaboration | [`macro-editor`](crates/macro-gpui/macro-editor), [`macro-canvas`](crates/macro-gpui/macro-canvas), [`macro-sync`](crates/macro-gpui/macro-sync) |
| Layout and presentation | [`macro-layout`](crates/macro-gpui/macro-layout), [`macro-list`](crates/macro-gpui/macro-list), [`macro-ui`](crates/macro-gpui/macro-ui), [`macro-theme`](crates/macro-gpui/macro-theme) |
| Validation and distribution | [`macro-parity`](crates/macro-gpui/macro-parity), [`macro-metrics`](crates/macro-gpui/macro-metrics), [`macro-updater`](crates/macro-gpui/macro-updater) |

The [architecture notes](crates/macro-gpui/docs/architecture.md) explain the
integration boundaries, engineering tradeoffs, and remaining structural work.

## Status

**Active development / alpha.** Zacro has substantial native implementations of
the core workspace workflows. Full compatibility with current Macro and
production release readiness are still in progress. The implementation baseline
is Macro's August 26, 2026 revision; newer product areas, including databases,
spreadsheets, WhatsApp, and booking workflows, are outside the current native
coverage.

The [status page](crates/macro-gpui/docs/status.md) distinguishes implemented code,
recorded local checks, and outstanding live-service and release validation.

## Attribution and licensing

Created and maintained by [Nick Gardner](https://github.com/NickGardner0).
Zacro is an independent project built on the
[Zed source tree](https://github.com/zed-industries/zed) and interoperates with
[Macro](https://github.com/macro-inc/macro). It is not an official Macro or Zed
product.

Upstream code, assets, and notices retain their attribution and licenses. See
[`LICENSE-GPL`](LICENSE-GPL), [`LICENSE-APACHE`](LICENSE-APACHE), asset notices,
and individual crate manifests for component terms, including the Zacro crates
marked AGPL-3.0-only.
