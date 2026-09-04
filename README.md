# Zacro

Zacro is an experimental native desktop client that rebuilds Macro's workspace
experience in Rust. It uses [Zed's GPUI](https://github.com/zed-industries/zed)
rendering framework, UI components, editor stack, layouts, icons, and semantic
color themes while connecting to Macro's service contracts.

The goal is a fast, keyboard-friendly application with Macro's connected inbox,
mail, channels, documents, tasks, agents, calls, CRM, files, calendar, settings,
and collaboration workflows presented through a Zed-style native interface.

> [!IMPORTANT]
> Zacro is under active parity development. It is not an official Macro or Zed
> product, and the presence of a screen does not by itself mean that the
> corresponding Macro workflow is complete or production-ready.

## Run Zacro locally

The current development target is macOS. Install the prerequisites from
[Zed's macOS build guide](./docs/src/development/macos.md), then run these
commands from the repository root:

```sh
git clone https://github.com/NickGardner0/Zacro.git
cd Zacro
cargo run -p macro_app
```

The first build compiles Zed, GPUI, and the Zacro crates and can take a while.
Later launches reuse Cargo's build cache.

For a fair UI responsiveness test, use the optimized build:

```sh
cargo run -p macro_app --release
```

Do not compare the first compilation or a fully unoptimized dependency graph
with an installed production application. Judge responsiveness after the window
has opened and data has loaded.

Production Macro services are selected by default. The application supports
passwordless sign-in and stores session credentials in the macOS keychain. To
target another configured environment, launch with one of the following:

```sh
MACRO_ENVIRONMENT=dev cargo run -p macro_app
MACRO_ENVIRONMENT=local cargo run -p macro_app
```

## What to evaluate

Zacro follows the operating-system appearance using Zed's One Light and One Dark
themes. When evaluating the current build, check:

- window chrome, sidebar, tabs, split views, menus, dialogs, icons, typography,
  spacing, focus states, and light/dark theme transitions;
- keyboard navigation, command palette behavior, text editing, scrolling, list
  virtualization, selection latency, and resize performance;
- loading, empty, error, offline, reconnecting, and authenticated states;
- real read and mutation paths for the Macro areas you use rather than the mere
  presence of navigation entries.

If the interface differs materially from Zed's visual language, feels sluggish
in a release build, or a Macro workflow is missing, that is remaining product
work rather than an inherent limitation of GPUI.

## Project layout

Zacro currently remains inside the Zed workspace because it directly reuses
internal Zed and GPUI crates:

- [`crates/macro-gpui`](./crates/macro-gpui) contains the Zacro product crates.
- [`crates/macro-gpui/README.md`](./crates/macro-gpui/README.md) records the
  implemented subsystems, verification commands, and current status boundary.
- [`crates/macro-gpui/PARITY_ROADMAP.md`](./crates/macro-gpui/PARITY_ROADMAP.md)
  is the implementation roadmap.
- [`crates/macro-gpui/PARITY_RELEASE_GATES.json`](./crates/macro-gpui/PARITY_RELEASE_GATES.json)
  is the machine-readable release-gate ledger.
- [`.github/workflows/macro_gpui.yml`](./.github/workflows/macro_gpui.yml) checks
  the native crates, visual fixtures, performance gates, and macOS packaging.

## Verification

Check the application graph:

```sh
cargo check -p macro_app
```

Run the native Zacro test matrix:

```sh
cargo test \
  -p macro_api \
  -p macro_app \
  -p macro_canvas \
  -p macro_editor \
  -p macro_entity \
  -p macro_layout \
  -p macro_list \
  -p macro_metrics \
  -p macro_models \
  -p macro_realtime \
  -p macro_store \
  -p macro_sync \
  -p macro_theme \
  -p macro_updater \
  -p macro_parity \
  -p macro_ui \
  -p livekit_client \
  --lib
```

Run the deterministic core-data performance soak:

```sh
cargo run -p macro_parity --bin macro_performance_soak
```

Run the release-mode GPUI interaction measurement:

```sh
python3 crates/macro-gpui/scripts/measure-gpui-interactions.py
```

These automated checks are useful evidence, but release parity still requires
matched testing against Macro's shipped client, live cross-client collaboration,
accessibility and multilingual-input review, distribution security review,
Apple signing/notarization, and license review. See the release-gate ledger for
the current source of truth.

## Upstream and attribution

Zacro is built from and modifies the
[Zed](https://github.com/zed-industries/zed) source tree. GPUI, Zed's UI and
editor crates, assets, and other upstream components remain attributable to
Zed Industries and their respective contributors. Zacro also interoperates
with Macro service contracts; Macro and its trademarks belong to their
respective owners.

The `upstream` Git remote should continue to point to Zed so compatible fixes
can be incorporated deliberately:

```sh
git remote add upstream https://github.com/zed-industries/zed.git
git fetch upstream
```

## Licensing

This repository contains Zed-derived code licensed primarily under
GPL-3.0-or-later, with Apache-2.0 components where marked. See
[`LICENSE-GPL`](./LICENSE-GPL), [`LICENSE-APACHE`](./LICENSE-APACHE), and the
notices within individual components. The Macro reference implementation has
separate AGPL-3.0 obligations that must be reviewed before distribution.

License information for third-party dependencies must remain complete for CI
and distribution. This summary is an engineering notice, not legal advice.
