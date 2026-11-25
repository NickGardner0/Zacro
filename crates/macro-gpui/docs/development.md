# Development

[Project overview](../../../README.md) · [Architecture](architecture.md) · [Status](status.md)

## Prerequisites

The current development target is macOS. You need:

- Rust installed through rustup; `rust-toolchain.toml` selects the pinned version.
- Full Xcode with macOS components and an accepted Xcode license. Command Line
  Tools alone do not provide the complete native build environment.
- CMake, available through `brew install cmake`.
- Node.js and npm for the browser/native collaboration fixture.

Check that `xcode-select -p` points inside the intended Xcode application. The
retained [Zed macOS build guide](../../../docs/src/development/macos.md) has
additional toolchain troubleshooting. Use the Zacro commands below in this repo.

## Build and run

From the repository root:

```sh
cargo run
```

The default member is `macro_app`, whose default binary is `zacro`. To select it
explicitly, use `cargo run -p macro_app`. For responsiveness evaluation:

```sh
cargo run -p macro_app --release
```

Evaluate after the first compilation and initial data loading. A build duration
is not an input-to-paint measurement.

## Accounts and environments

Connected workflows require a Macro account. Production services are the
default; mutations operate on that account's data. Use a dedicated test account
for demonstrations and service-backed testing.

```sh
MACRO_ENVIRONMENT=dev cargo run -p macro_app
MACRO_ENVIRONMENT=local cargo run -p macro_app
```

`dev` selects the configured development service hosts. `local` selects localhost
services; it does not start a backend stack. Use Macro's own local-stack setup
and the matching reference revision when testing contracts.

Session credentials use the macOS keychain. The packaged application has its own
bundle identity, `com.nickgardner.zacro`, and registers `zacro:` links.

## Verification

Check the application graph:

```sh
cargo check -p macro_app
```

Run the native library matrix:

```sh
cargo test \
  -p macro_api -p macro_app -p macro_canvas -p macro_editor \
  -p macro_entity -p macro_layout -p macro_list -p macro_metrics \
  -p macro_models -p macro_realtime -p macro_store -p macro_sync \
  -p macro_theme -p macro_updater -p macro_parity -p macro_ui \
  -p livekit_client --lib
```

Run the browser/native document fixture:

```sh
cd crates/macro-gpui/macro-sync/fixtures
npm ci --ignore-scripts --no-audit --no-fund
node verify_markdown_two_client.mjs
```

From the repository root, capture deterministic GPUI states and measure performance:

```sh
python3 crates/macro-gpui/scripts/capture-gpui-fixtures.py
cargo run -p macro_parity --bin macro_performance_soak
python3 crates/macro-gpui/scripts/measure-gpui-interactions.py
```

The captures use synthetic fixture data and the application's render paths.
PNG files and accessibility metadata are written under
`target/macro-gpui/visual-current/`. Label these images as fixture renders when
using them in a demo. They are candidate captures, not a completed comparison
against the web client.

The performance scripts measure their declared workloads. Keep the commit,
hardware, build mode, and generated attestation with any published result.

To run the separate service-backed contract test with a prepared Macro checkout:

```sh
crates/macro-gpui/scripts/test-local-stack.sh /path/to/pinned/macro
```

See the [release gates](../PARITY_RELEASE_GATES.json) for environment requirements.

## Build an application bundle

```sh
crates/macro-gpui/scripts/bundle-macos.sh release
open target/zacro/release/Zacro.app
```

Local bundles use ad-hoc signing. Developer ID signing, notarization, and a
production update origin are separate release requirements. The protected
manual release job in [native CI](../../../.github/workflows/macro_gpui.yml)
expects the corresponding credentials.

## CI and upstream maintenance

[Zacro native CI](../../../.github/workflows/macro_gpui.yml) is configured for
pull requests and relevant pushes to `codex/macro-gpui-rewrite` or `main`.
It covers formatting, library tests, the application graph, fixture generation,
performance checks, and macOS packaging. Consult the actual run and artifacts
for results; a workflow definition alone does not establish a passing build.

The repository retains Zed's source and dependency workspace, but removes Zed's
release, community, Slack, and organization-specific GitHub automation. To track
upstream code deliberately:

```sh
git remote add upstream https://github.com/zed-industries/zed.git
git fetch upstream
```

If that remote already exists, inspect it with `git remote -v` before changing it.
Keep upstream attribution and license notices when incorporating changes.
