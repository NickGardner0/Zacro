# Contributing to Zacro

Zacro is an independent native desktop client for Macro, built in Rust with
GPUI. Product work lives in `crates/macro-gpui`; the surrounding workspace
contains the Zed components it uses.

Start with the [development guide](crates/macro-gpui/docs/development.md),
[architecture](crates/macro-gpui/docs/architecture.md), and
[current status](crates/macro-gpui/docs/status.md).

## Report a problem or propose a change

Use [Zacro's issue tracker](https://github.com/NickGardner0/Zacro/issues).
Describe the workflow, the result you expected, and the result you observed.
Include the commit, macOS version, and reproduction steps. For compatibility
issues, include the Macro revision or behavior used as the reference.

Remove account data, message contents, email addresses, and credentials from
logs and screenshots before posting. Use the feature-request form to explain
the user problem and the smallest useful improvement.

## Make a focused change

1. Create a branch from the repository's default branch.
2. Keep product changes in the Zacro crates where possible. Changes to shared
   Zed crates should explain why the application layer cannot handle the issue.
3. Use existing GPUI components and semantic theme tokens. Check keyboard
   navigation, focus, and both light and dark appearances for UI changes.
4. Ground API changes in Macro's actual schemas or generated SDK. Include the
   relevant reference revision and cover failure and reconnect behavior.
5. Run checks appropriate to the change and describe any checks you could not run.

## Validation

For Rust changes, format the packages you touched and check the app graph:

```sh
cargo fmt --check -p macro_app
cargo check -p macro_app
```

Replace or extend the package list when working in other crates. The
[development guide](crates/macro-gpui/docs/development.md#verification) lists the
full test matrix, collaboration fixtures, and rendering/performance commands.
For workflow changes, run `actionlint` when available.

Tests should exercise behavior through the relevant public interface. A source
marker or a passing checklist entry is not evidence of live-service parity.
Keep compatibility claims tied to a revision, scenario, and recorded result.

## Pull requests

Explain the user-visible result, the implementation decision, and how it was
validated. Include actual screenshots for visual changes and identify any
fixture data. State unverified behavior directly. Complete the review checklist
in the PR template; review reminders belong in the PR rather than the README.

Preserve upstream attribution, copyright headers, and component licenses.
Follow the [code of conduct](CODE_OF_CONDUCT.md).
