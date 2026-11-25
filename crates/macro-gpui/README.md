# Zacro product crates

This directory contains the native Macro client implemented in Rust and GPUI.
The root [project overview](../../README.md) introduces Zacro; the guides below
cover development and the current implementation.

| Guide | Contents |
| --- | --- |
| [Architecture](docs/architecture.md) | Product goals, integration boundaries, and engineering tradeoffs |
| [Development](docs/development.md) | Build, run, test, capture fixtures, and package the app |
| [Status](docs/status.md) | Current coverage, compatibility gaps, and validation limits |
| [Implementation notes](IMPLEMENTATION_NOTES.md) | Detailed subsystem notes and historical verification records |
| [Original roadmap](PARITY_ROADMAP.md) | Milestones against the pinned Macro reference |

## Source map

- `macro-app`: application lifecycle, authentication, navigation, feature views,
  and native media integration.
- `macro-api`, `macro-models`: service transport, wire contracts, and domain types.
- `macro-store`, `macro-realtime`: shared state, cached collections, optimistic
  changes, and event reconciliation.
- `macro-editor`, `macro-canvas`, `macro-sync`: editing, drawing, collaboration,
  snapshots, and local recovery.
- `macro-layout`, `macro-list`, `macro-ui`, `macro-theme`: pane state, list
  interaction, reusable components, and Zed theme integration.
- `macro-parity`, `macro-metrics`, `macro-updater`: validation artifacts,
  measurements, and application updates.

## Reference artifacts

The [baseline](PARITY_BASELINE.json), [source inventory](SOURCE_INVENTORY.json),
[fixture manifest](PARITY_FIXTURE_MANIFEST.json), and
[release gates](PARITY_RELEASE_GATES.json) describe the original compatibility
program. Their implementation labels and recorded local results should be read
alongside the [current status](docs/status.md), especially when comparing Zacro
with a newer Macro revision.
