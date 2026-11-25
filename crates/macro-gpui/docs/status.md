# Project status

[Project overview](../../../README.md) · [Architecture](architecture.md) · [Development](development.md)

**Stage: alpha / active development.** Source coverage reviewed November 25, 2026.
This is a source assessment; it is not a new live-service or release attestation.

## Reference scope

The original implementation targets Macro revision
[`7324c6a`](https://github.com/macro-inc/macro/commit/7324c6adaee90070e64bcabf3c0d77600c96db71)
from August 26, 2026, with Zed revision `f66ed399` as its framework baseline.
The [baseline ledger](../PARITY_BASELINE.json) records those pins.

A source review against Macro
[`0386681`](https://github.com/macro-inc/macro/commit/03866815645c62a3fe2cf63bae5b6c4e12d49ef4)
identified significant newer functionality and contract changes. The original
ledger is not a complete inventory of today's Macro.

## Coverage

| Area | Current boundary |
| --- | --- |
| Native shell | Themes, tabs, splits, keyboard navigation, authentication, and persistent layout are implemented. |
| Mail and messaging | Service-backed read/write paths, composition, drafts, threads, reactions, and attachments exist. Current provider and workflow compatibility needs validation. |
| Documents and canvas | Native editing, Loro/Bebop integration, snapshots, history, and recovery exist. Live native/web convergence remains unverified by the release program. |
| Tasks, CRM, and files | Core reads, mutations, properties, sharing, and organization workflows exist. These labels do not establish full current feature parity. |
| Agents and calls | Agent lifecycle/transcript and native media implementations exist. Complete production behavior needs live validation. |
| Calendar | Event listing and mutations exist; a full scheduling calendar, availability, and booking workflows are not implemented. |
| Databases, spreadsheets, WhatsApp | No native product implementation identified in the current Zacro application. |
| Distribution | macOS bundle and updater code exist. A signed, notarized production release remains an open gate. |

One concrete contract gap: Zacro's `EmailProvider` enum supports Gmail only,
while the reviewed Macro SDK also exposes Outlook. Refreshing service contracts
is necessary before claiming current multi-provider mail compatibility.

## How to read the evidence

The baseline labels 34 of 39 broad categories as implemented. That count measures
its own checklist coverage, not the percentage of user workflows completed.

The [release ledger](../PARITY_RELEASE_GATES.json) records six local gates as
passed: library tests, a browser/native document fixture, GPUI captures, a data
soak, interaction latency, and an ad-hoc bundle. These are recorded development
results and must be rerun for the candidate revision being evaluated.

The fixture manifest defines 117 states in two appearances, producing 234
candidate image/accessibility pairs. Matched web-client captures remain open.
`macro_parity` validates artifact consistency and selected source markers; it
does not itself execute every workflow named in the ledger.

## Open release gates

- Matched SolidJS reference captures and comparison against the native fixtures.
- Contract tests against Macro's seeded local stack.
- Live native/web collaboration, awareness, disconnect, and reconnect scenarios.
- VoiceOver and representative multilingual input-method review.
- Distribution security review.
- Developer ID signing, notarization, and production update configuration.
- Distribution licensing and source/notice delivery review.

## Next acceptance milestone

Establish a reproducible native/web comparison against a refreshed Macro pin.
Start with a small set of complete journeys: authenticate, triage mail, reply in
a channel, edit a shared document, create a linked task, and recover after a
network interruption. Record the build, fixture account, expected behavior,
actual result, and unresolved failures for each journey.

That evidence should drive feature claims, screenshots, and release decisions.
The detailed [implementation notes](../IMPLEMENTATION_NOTES.md) and
[original roadmap](../PARITY_ROADMAP.md) remain available as development context.
