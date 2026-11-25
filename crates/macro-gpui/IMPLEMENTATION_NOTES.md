# Implementation notes

These notes describe the initial implementation against the pinned Macro baseline.
The checks and measurements below are historical development records. See
[project status](docs/status.md) for current coverage and validation limits.

Native GPUI client workbench for rebuilding Macro's desktop frontend with a
Zed-inspired interaction model.

## Implemented subsystems

- `cargo run -p macro_app` launches a native GPUI application.
- Sidebar clicks, editable shortcuts, and the searchable keyboard command
  palette navigate between workspace areas. `Cmd-B` toggles the sidebar,
  `Cmd-K` opens the palette, `Cmd-W` closes a tab, and `Cmd-Shift-T` reopens it.
- Unified search is authored in Zed's native single-line editor rather than a
  workspace-level character accumulator, so cursor movement, selection,
  clipboard operations, undo/redo, IME composition, and AccessKit value edits
  use the same input stack as Zed. Arrow keys move into a named, virtualized
  results list; Enter opens fullscreen, Shift-Enter opens in the split, Escape
  returns focus to the query, and every result exposes its position and shortcut
  contract to assistive technology.
- Passwordless email/code authentication is connected to the production auth
  service by default. Access, refresh, and Macro API tokens are persisted in the
  platform keychain, restored at launch, and refreshed when needed.
- Signed-in sessions load `/items/soup`, `/comms/channels`, paginated channel
  messages, active calls, connected inboxes, and agent-tool integrations. Mail,
  documents, tasks, agents, CRM, files, calendar, activity, and reminders are
  honest filtered views over authenticated data rather than mock screens.
- Soup and channel selections open persistent, selectable entity tabs. Channel
  tabs load real message timelines; email, document, CRM, reminder, and agent tabs
  fetch complete type-specific records instead of dumping raw JSON.
- Channel message deep links resolve reply IDs through Macro's canonical
  endpoint, load the owning thread, and use persistent GPUI virtual-list
  handles to reveal both the timeline root and exact reply in cold and
  already-cached paths. The deterministic oversized fixture proves both
  selected rows are painted outside their initial viewports as named
  accessibility list items in One Light and One Dark; the seeded local-stack
  gate also verifies Macro's reply-resolution and thread-list contracts when
  the service stack is available.
- Native collaborative Markdown editing uses Macro's sync-service Bebop/Loro
  transport, WAL replay, snapshots, awareness, comments, history/diff/fork,
  autosave, and explicit remote-conflict handling. Unknown Lexical blocks are
  preserved losslessly instead of being silently rewritten. The editing surface
  uses Zed's native editor input/selection/IME stack and offers a live Zed-themed
  rich block view by default; each rendered block has a native Edit action that
  opens a block-scoped Zed editor in place while writing through the authoritative
  document buffer. The rich view is a tab stop with an accessible document/list structure, exposes selected
  block state, and supports Arrow or `j`/`k`, Home/End, and Enter/`e` keyboard
  operation. Clicking rendered text edits that block at the corresponding
  character, rendered blocks can be reordered directly from their native drag
  handles, and rendered checklist controls mutate the real
  document in place. Its selection stays synchronized with the native block rail. Its native
  block rail reuses Macro Lexical node IDs,
  identifies block types, selects whole blocks through the real editor
  selection, and supports pointer move, indent, outdent, and drag reorder
  operations without introducing a second document state model. Document
  stats, outline, and block analysis are cached at text/CRDT mutation boundaries
  so cursor-only paints do not reparse the entire document. The rich block
  surface is a variable-height GPUI virtual list: only visible/overdraw blocks
  are laid out, changed blocks are remeasured in place, and stable Lexical IDs
  preserve the selected block and scroll anchor across remote reconciliation.
  Click or Enter opens a block-scoped Zed editor in place; its edits
  write through the authoritative document buffer and therefore use the same
  undo, autosave, projection, and collaboration path as source-mode edits.
  Each inline session starts with block-local undo history, and multi-stage IME
  composition is relayed with UTF-8-safe current-buffer ranges so composed text
  is undone atomically without reaching into a previously edited block.
  Formatting and block commands apply to every native Zed selection in one
  simultaneous edit, preserving useful selections after earlier replacements
  shift later cursors.
  The inline editor grows from two to twelve display lines before scrolling,
  and the Zed-native Style, Block, Insert, and More menus keep the common
  formatting toolbar compact without hiding keyboard access.
  Session-stable fallback IDs prevent unsynchronized Markdown blocks from
  borrowing a neighbor's selection after insertion or deletion. Non-overlapping
  remote changes merge while inline editing remains active; overlapping changes
  freeze the exact local snapshot and exit inline mode before resolution.
  Top-level blocks come from the same CommonMark/GFM parser used for projection,
  so adjacent headings and paragraphs stay distinct while loose lists, tables,
  quotes, and fenced code containing blank lines remain one lossless block.
- Email compose/reply/reply-all/forward, drafts, scheduling, signatures,
  attachments, task/document/folder/file mutations, channel threads/reactions,
  managed agents, CRM, calendar, settings, integrations, and notifications use
  typed Macro service contracts rather than mock screens.
  Email bodies are authored in the native Zed editor and sent as Macro Markdown,
  derived plain text, and Macro's base64url HTML wire format. Meaningful drafts
  autosave after 500 ms through serialized server writes, survive recipientless
  early composition, delete when emptied, and select a connected From inbox
  without requiring the user to discover an internal link ID. Macro contacts
  appear as native recipient typeahead suggestions for To/Cc/Bcc. Typed user
  mentions retain Macro's internal mention tag, render as safe inert HTML, and
  add the mentioned address to Cc without duplicates. Reply All excludes the
  active inbox while preserving sender priority. Per-composer signature
  dismissal follows the backend's omission-vs-false contract, quoted replies
  remain outside editable `body_macro`, and forwards synchronize original
  attachments through Macro's forwarded-attachment endpoints before send.
  Email threads use Macro's canonical `THREAD` property target so native tags
  and properties share the web client's contract. The entity inspector exposes
  backlinks and view-only channel sharing; email link sharing and elevated
  channel access remain disabled by design. “Create linked task” creates the
  task first and persists a typed task-to-email mention, while reporting a link
  failure without encouraging a duplicate task retry.
  Send persists the exact draft first so the global Undo action can unschedule
  and restore the composer.
  Server and agent-created drafts are identified as drafts, never selected as
  reply targets, and reopen with recipients, rich body, schedule, account, and
  attachments intact for an explicit human review before sending.
- CRM list mode mirrors Macro's persisted Stage, Owner, and Revenue column
  preferences instead of collapsing pipeline fields into a generic details
  cell. Stage, owner, and revenue are mutable from company rows through the
  canonical system-property contracts, and company details distinguish first
  and last interaction timestamps while reporting whether directory enrichment
  metadata is available. List and board views retain saved personal/team views,
  owner and stage filters, bulk mutations, virtualized rows, and horizontally
  scrollable board columns.
- File uploads hash and stream bounded 256 KiB chunks from disk to Macro's
  presigned storage URL, remain cancellable during both passes, and wait for
  `/location_v3` backend finalization without buffering a permitted 2 GB file
  in memory. Zip-folder imports use `/projects/upload_extract` and reconcile
  the correlated `bulk_upload` realtime status. Downloads use native save
  dialogs and validate binary/data URL payloads before writing sanitized
  filenames.
- Selecting a project folder loads its backend capabilities before enabling
  mutations. Editors can open Macro's native project-sharing dialog, while
  create, rename, move, and delete controls remain disabled for viewers instead
  of presenting actions that the service will reject.
- Task details load associated GitHub pull requests through the generated
  storage contract, enrich their title, lifecycle, line changes, checks, and
  comments through Macro's authenticated GitHub proxy, and retain the stored
  association when live enrichment is unavailable. The Zed-themed task panel
  opens only validated HTTPS GitHub URLs and keeps unknown future statuses
  renderable.
- Task lists support native multi-selection and backend-persisted bulk status
  transitions across every Macro task state. Status calls run with bounded
  concurrency, reconcile successful task/property caches, and retain only
  failed tasks for targeted retry. Bulk tag additions and removals use Macro's
  generated cross-entity `/properties/options/bulk` contract so per-task
  permission skips and failures remain visible instead of being flattened into
  a false all-or-nothing result.
- Task details expose one native planning editor for status, priority,
  assignees, due date, parent, dependencies, effort, story points, and relevant
  documents. It hydrates existing values by stable system-property IDs, creates
  previously unset properties, deletes fields the user clears, validates dates
  and numeric values before sending, and reports partial backend failures.
- Tasks expose Macro's generated `/documents/{id}/branch_name` workflow through
  a native button and the product shortcut `Shift-Command-B`. The returned
  branch name is copied to the clipboard; Macro's GitHub app performs the
  server-side pull-request association and bidirectional status transitions.
- Markdown-capable native editors load the dedicated personal/team snippet
  feed used by Macro's semicolon typeahead. `Up`/`Down` wrap through the eight
  visible results, `Enter` inserts the selected result, and `Escape` dismisses
  only the exact active trigger so editing or moving the caret can start a new
  session. Selecting `;query` fetches the snippet's permission-scoped raw
  Lexical state from the sync worker, prevents duplicate requests and
  self-insertion, and applies Macro's block-flattening rules. Existing unknown
  nodes and typed mention payloads remain lossless rather than being pasted as
  plain display text. The same insertion path is available in document/task
  editors and native mutation composers such as mail, messages, and comments.
- Entity linking uses a typed, searchable Zed list instead of asking for
  backend IDs or wire aliases. People, documents, tasks, CRM records, channels,
  projects, mail, calls, calendar items, and supported agent entities retain
  Macro's generated mention types, exclude the source entity, and submit the
  real bidirectional `/channels/mentions` contract. Forward links and incoming
  backlinks resolve cached human names, open the correct native entity or
  channel target, and invalidate only the affected target's backlink cache.
- Permission-gated entity views fail closed once a permission request starts.
  Loading, failed, and explicit no-access results never expose an already-cached
  detail, property, tag assignment, sharing policy, or entity graph. A compact
  Zed-style lock state retains only a safe access refresh action; view-only
  records keep readable metadata while edit, tag, link, and sharing mutations
  stay unavailable.
- Canvas live-entity embeds use a filtered Zed picker for Macro's supported
  document, chat, project, channel, email, and call nodes. The canvas itself is
  excluded, backend IDs and type aliases remain internal, and only the optional
  product subpath is editable before the node is added to the real canvas model.
- Workspace administration uses native Zed segmented controls for member,
  admin, and owner roles and for no-default, workspace, and public link access.
  The typed auth request distinguishes an omitted setting from an explicit
  `null`, so choosing no default really clears Macro's server-side policy.
- The native canvas supports Macro-compatible focused shortcuts for selection,
  clipboard, undo/redo, grouping, one-step and absolute z-order, zoom, tools,
  nudging, cancellation, and deletion; shortcuts stay scoped to the canvas
  instead of overriding the document editor or workspace. Selections have a
  native eight-anchor bounding box; multi-node and free-connector resizing is
  proportional for media by default, Shift toggles proportional scaling,
  Option scales from center, and the whole drag is one undo step.
  Line, text, rectangle, ellipse, pencil, hand, selection, and click-zoom tools
  are pointer-driven. Rectangle and ellipse tools create normalized drag
  geometry, while text supports click-to-follow-width and fixed-width drag
  creation. Connectors have live previews, free endpoints, Shift 45-degree
  snapping, nearest-side node attachment, and draggable endpoints that can be
  reattached or made free without corrupting node edge indexes. Native drag
  marquee selection supports Shift accumulation and expands grouped nodes and
  connectors together.
  Selected connectors can be reversed as one undoable operation, and local
  images can be dropped directly onto the canvas for upload and insertion at
  the pointer position.
- Native agent details decode Macro's tagged session status instead of treating
  it as display text. Disconnected and stopped sessions expose a resume
  composer; a failed turn can be retried once through the real control endpoint
  using only that turn's original prompt. Duplicate retry requests are blocked.
  Settings display the backend-owned unified memory and provide copyable Macro
  MCP setup for Claude Code, Codex CLI, Claude.ai, ChatGPT, and HTTP-capable
  IDEs using Macro's canonical MCP endpoint. Channels expose an “Ask Macro”
  composer that sends the canonical `bot|…` principal mention, allowing the
  storage backend to dispatch the same threaded Macro-agent response as the web
  client instead of creating a separate desktop-only chat path. New-agent
  creation supports both managed sessions and external bot workspaces, while
  validating Macro's contract that managed prompts and external workspace/bot
  fields are mutually exclusive. Scheduled-agent started/stopped gateway frames
  now patch claimed state and execution history without polling races; each
  history record carrying Macro's `resource_id` opens the real output chat,
  including failed and still-running executions.
- The connection-gateway socket runs with native certificate roots, application
  heartbeats, reconnect/backoff, visible diagnostics, and cache reconciliation
  for Macro's documented invalidation events. A second authenticated
  `graphql-transport-ws` Soup subscription consumes DSS entity updates and
  cache-deletion/access-loss frames. It refreshes only the affected entity's
  permission, sharing, property, backlink, and open-detail caches; deletion
  frames fail closed and purge stale normalized records and document state.
- Native LiveKit calls support start/join/leave/end, ring accept/dismiss,
  terminal-disconnect reconnect, microphone state, audio-device hot-plug
  fallback, native macOS camera capture/device switching/local preview with
  hot-plug and stalled-session recovery, screen share, remote audio/video tiles,
  active speakers, and network quality. Completed calls use native AVFoundation
  recording playback with seeking and time-synchronized transcripts, preserve
  transcript speaker identification, resolve participant names, and can start a
  managed “Ask Macro” session with the call context.
- Application chrome and mutation dialogs use Zed's embedded SVG icon assets,
  compact GPUI layout primitives, and semantic colors from Zed's active theme.
  The app follows the OS with Zed's One Light and One Dark themes.
- Appearance settings can enable GPUI's process-wide reduced-motion mode and a
  higher-contrast projection of the active Zed theme. Higher contrast stays
  inside Zed's semantic palette, replacing low-emphasis Macro text, icon, and
  border roles across every surface rather than introducing a second theme.
- A distinct `Zacro.app` can be built under `target/zacro` with an ad-hoc
  signature, an isolated bundle identity, and the `zacro:` URL scheme:

  ```sh
  crates/macro-gpui/scripts/bundle-macos.sh release
  ```
- `.github/workflows/macro_gpui.yml` runs formatting, the complete native crate
  matrix, application graph checking, ad-hoc bundle construction, signature and
  bundle-ID verification, and short-lived artifact upload on macOS. A protected
  `workflow_dispatch` release job imports an ephemeral Developer-ID certificate,
  signs with the hardened runtime, submits to Apple notarization, staples and
  validates the ticket, verifies Gatekeeper plus update-manifest identity, and
  uploads the signed archive. The job requires the `macro-production`
  environment and its distribution secrets, so pull requests never receive
  signing credentials.
- Packaged builds can use a signed HTTPS update feed. The native updater streams
  and verifies release archives, validates the Developer-ID team and Gatekeeper
  assessment, performs a journaled atomic bundle swap, and retains the previous
  app until a launch health check confirms the new release. Interrupted or
  repeatedly crashing launches roll back automatically.
- The desktop process reuses Zed's native minidump sidecar. It records bounded
  crash metadata and compressed dumps under the Macro-specific logs directory;
  users can disable local crash capture in Data & Privacy, with the change
  taking effect on restart. Reports are not silently uploaded by this client.
- All API paths added in this pass are grounded in Macro's generated SDK under
  `macro/packages/sdk/generated`, not inferred from frontend route names.
- Backend-provided browser handoffs (OAuth, billing, exports, recordings,
  conferences, integration setup, and artifacts) are constrained to parsed
  HTTP(S) URLs before GPUI asks the operating system to open them.
- Consequential agent controls are explicit confirmation boundaries: stopping a
  run requires `STOP` and compacting away prior context requires `COMPACT`.
  Prompting and model selection remain ordinary user-initiated actions, while
  runtime permission requests remain visible typed transcript events.

Run the current checks from the Zed repository root:

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

cargo run -p macro_app
```

The production render tree also has a headless Metal capture path for
deterministic, sanitized visual fixtures. It does not read credentials or call
Macro services:

```sh
cargo run -p macro_app --features visual-tests --bin macro_visual_test_runner -- \
  --theme light --surface inbox --state populated
target/debug/macro_visual_test_runner \
  --theme dark --surface auth --state code_entry
target/debug/macro_visual_test_runner \
  --theme light --surface mail --state compose
python3 crates/macro-gpui/scripts/capture-gpui-fixtures.py
```

The matrix script builds the feature-gated runner from the current source and
atomically stages a private executable before launching parallel captures. This
prevents `cargo check` or a stale `target/debug` artifact from producing a false
attestation. Pass `--runner /absolute/path/to/a/prebuilt/runner` only when an
external build system has already established that runner's provenance;
`--validate-only` checks existing pixels and preserves the runner hash recorded
by the render run.

CI captures both 1440×960 logical (2880×1920 Retina) themes for every declared
primary GPUI surface. That includes all four auth, Home, inbox, Search,
Activity, Reminders, Calendar, and integration baseline states, plus ten
Channels states covering deep-linked thread targets and explicit private-invite
acceptance, participant actions, realtime typing, native rich send/edit
composers with attachment lifecycle controls, and reconnect recovery;
nineteen document states, including in-place editing, an
actual overlapping remote merge conflict, and keyboard-selected semicolon
snippet typeahead, fail-closed realtime access restoration, plus real pointer,
screen-reader, and IME input mutations, five native mail workflow states,
including quoted reply, forwarded-attachment, and view-only channel-share UI,
and six task states including enriched linked pull requests, the native
bulk-selection toolbar, and the dedicated Zed-native task composer; agent permission, canvas remote-conflict, and file
upload states, six CRM list/group/board states, six calls states
including ringing and synchronized recording playback, seven integration states
including one-time bot credentials and webhook editing, plus nine settings
states covering external-client MCP setup and resumable onboarding
(226 candidates total). The manifest-driven capture script verifies every PNG's
dimensions and every production-rendered AccessKit tree for a root, keyboard
tab stops, human-readable named actions, and focusable actions. It writes SHA-256 image and
accessibility attestations before CI uploads the matrix for review.
The inline-document states additionally expose the active Zed editor as one
named text-input node with a readable value and screen-reader `Focus` and
`SetValue` actions. The accessibility-edit state dispatches a real AccessKit
`SetValue` request through GPUI's production action router and verifies the
resulting Japanese text value independently in One Light and One Dark.
The IME state also focuses that same native Zed editor, sends marked-text
updates through GPUI's platform input handler, commits `日本語`, and attests the
post-composition AccessKit value in both themes.
The pointer-editing state targets the paragraph's painted AccessKit bounds,
dispatches real GPUI mouse move/down/up events, verifies that the rich preview
entered its inline Zed editor, then replaces text through the focused platform
input handler and attests the rendered result.

A sanitized SolidJS reference harness is also checked in. Its initial corpus is
deliberately limited to data that Macro's `local_e2e_smoke` seed actually owns:
Documents/populated, Search/populated, Channels/populated, and the exact seeded
deep-thread target, in Macro Light and Macro Dark (eight captures). It does not
reuse screenshots from a personal production account or pretend that unseeded
mail, tasks, calls, and integration states are deterministic references.

```sh
python3 crates/macro-gpui/scripts/capture-solid-reference.py --prepare-only
python3 crates/macro-gpui/scripts/capture-solid-reference.py
python3 crates/macro-gpui/scripts/compare-parity-captures.py
```

The capture command verifies the pinned Macro revision, creates a temporary
Playwright harness inside the Macro web workspace, and runs it through Macro's
official isolated local-E2E orchestrator. The browser uses a fixed 1440×960
logical viewport at 2× scale, fixed locale/timezone, settled motion and fonts,
seeded production routes/selectors, and in-page email/secret redaction. Each
image is paired with a DOM/layout/text-hash attestation. The comparison command
then verifies both source contracts and hashes, the immutable GPUI runner,
dimensions, reference metadata, and candidate AccessKit artifacts before
producing left/right review sheets. Pixel equality is intentionally not the
criterion across different design systems; the manual gate is feature
presence, hierarchy, density, interaction affordance, and Zed design quality.

Executing the SolidJS browser corpus requires Nix and Docker because those are
hard prerequisites of Macro's isolated service stack. They are not installed
on the current development Mac, so the harness is source-validated and ready
but no SolidJS reference attestation is claimed here.

Every raw pointer action in `macro_app` is also a named keyboard tab stop with
a button accessibility role and a visible Zed-token focus state. Macro's Zed
buttons and sidebar list items are enrolled in the same tab order. `macro_parity`
audits the source invariant, while the capture matrix audits the actual
production-rendered accessibility trees so new pointer-only or unnamed controls
fail the build. Native tabs, regions, headings, status surfaces, splitters, and
media controls retain their component-provided accessibility semantics.

The workspace dev profile keeps the Macro-native crates at `opt-level = 2`, so
the ordinary command above retains debug assertions and symbols without forcing
all product hot paths through unoptimized Rust. For release-performance or
side-by-side timing, use `cargo run -p macro_app --release`; comparing a default
debug binary with Macro's shipped production client is not a valid performance
baseline.

CI also runs an optimized, deterministic core-data soak:

```sh
cargo run -p macro_parity --bin macro_performance_soak
```

It exercises 100,000-item inbox classification, 50,000 realtime channel
frames, a 25,000-block document analysis/search/Lexical projection, 100,000 CRM
row interactions, a 10,000-node canvas, and a 2,000-turn agent transcript. Each
surface has a checked wall-clock budget and the aggregate must finish within 12
seconds; the JSON report is uploaded by CI. This gate found and removed a
quadratic document block line-number scan (the local 25,000-block case fell
from roughly 9.9 seconds to roughly 0.3 seconds). It is a native core-data soak,
not a substitute for rendered interaction timing.

CI separately builds an immutable release-mode GPUI runner and drives 120 real
pointer selections through the populated inbox in both One Light and One Dark:

```sh
python3 crates/macro-gpui/scripts/measure-gpui-interactions.py
```

The measurement begins at the production root mouse-down capture and ends only
after GPUI draws and submits the changed scene to its headless Metal window. It
asserts exactly one latency sample per click, zero unrelated top-level requests,
and the `PARITY_BASELINE.json` 50 ms p95 budget, then records the release binary
SHA-256, host, per-theme JSON, screenshots, accessibility trees, and combined
attestation under `target/macro-gpui/performance-current/`. This corrected an
earlier telemetry boundary that ran at the start of the next frame, before
`draw()` and `present()`. A matched SolidJS-vs-GPUI trace on the same production
dataset and representative hardware is still required before claiming relative
performance parity with Macro's shipped client.

The 2026-09-01 local release attestation passed with 120/120 samples per theme:
One Light measured 1.062 ms p50 / 1.084 ms p95 and One Dark measured 1.051 ms
p50 / 1.075 ms p95, with zero top-level request deltas. The attestation records
the exact staged runner SHA-256 under
`target/macro-gpui/performance-current/interaction-performance-attestation.json`.

Production is the default. Use `MACRO_ENVIRONMENT=dev` or
`MACRO_ENVIRONMENT=local` to select another SDK host set. Local development may
also inject an ephemeral `MACRO_API_TOKEN`; it is never saved by the app.
Distribution builds may supply Macro's public PostHog project token through
`MACRO_POSTHOG_API_KEY` so optional account-deletion feedback follows the same
single-purpose event contract as the web client; deletion itself never depends
on telemetry delivery.
The packaged app uses a separate keychain namespace from `cargo run`. On first
launch it performs a bounded lookup of both the current account-scoped key and
the legacy development/single-account keys, validates or refreshes the session,
and copies valid credentials into the packaged account-scoped namespace. If
macOS denies the one-time legacy keychain read, the lookup times out safely and
the user can sign in normally; credentials are never migrated through a file.

## Important status boundary

This is a substantial authenticated GPUI implementation with real read,
mutation, realtime, collaboration, mail, call, agent, CRM, calendar, settings,
integration, file, folder, and canvas subsystems. The original baseline labels the implementation rows through M11 as implemented.
`macro_parity` checks the ledger and selected source markers; these checks do not
establish complete workflow equivalence or compatibility with current Macro.
Overall release parity remains open until the external/reference gates pass.

The remaining release gates require environments or authority absent from a
normal source checkout: side-by-side SolidJS captures from Macro's Nix/Docker
local stack, a live native↔web collaboration/reconnect/awareness run, OS
VoiceOver and broader multilingual IME review, a distribution security review,
Apple Developer-ID signing/notarization credentials, and AGPL/GPL legal review.
The source contains protected CI jobs for the signing/notarization portion and
deterministic GPUI coverage for pointer editing, Japanese IME, mixed
Arabic/Hebrew/English layout, reduced motion, high contrast, and screen-reader
metadata; those local checks are not presented as substitutes for the external
release gates.

`PARITY_RELEASE_GATES.json` is the machine-readable closure ledger. It records
reproducible commands for every passing local gate and the exact environment,
credential, manual-review, or legal input required for every open external
gate. `macro_parity` rejects missing gates, stale source pins, state-count drift,
or a locally executable gate that is not marked passing.

`PARITY_FIXTURE_MANIFEST.json` now defines the sanitized fixture profile,
reference/candidate clients, light/dark capture targets, deterministic viewport,
redaction boundary, state matrix, and interaction checks for every primary
surface. It is validated by `macro_parity`; all declared GPUI candidate state
matrices are now generated from production render paths headlessly in both
themes. The current candidate matrix contains 117 states and 234 One Light/One
Dark image plus AccessKit pairs. `SOLID_REFERENCE_CAPTURE_PLAN.json` and the
paired runner contract define the first eight deterministic SolidJS theme/state
comparisons, but executing that corpus on a Nix/Docker host and extending
Macro's seed data to cover the full GPUI matrix remain outstanding.
Fixture/golden parity therefore remains partial.

Production authentication, soup loading, and the revised shell have been
visually QA'd with an authorized Macro account. Passwordless, Google/enterprise
SSO deep links, persisted session layout, multi-account selection, team
selection, invitation/admin paths, and the backend-owned resumable onboarding
aggregate now have native routes. The full new-user parity suite and live
service exit test remain pending.

Auth token exchange is deliberately side-effect-free: passwordless, SSO,
keychain restore, and periodic refresh results are activated only when their
account-and-generation guard still owns the session. Switching, adding, or
signing out an account invalidates older auth/realtime/media work and clears
account-scoped documents, canvases, uploads, messages, entity caches, editors,
tabs, history, search, CRM state, and optimistic mutations before another
account can render. Saved credentials remain isolated per user in the platform
keychain; preferences never contain authentication tokens. Zed shell state is
partitioned per account, so tabs, active section, back/forward history,
fullscreen detail, list filters, and saved-view selections restore only for the
account that created them, with automatic migration from the older global
preference shape.

The independent Macro palette has been removed. The client loads Zed's bundled
theme registry, follows system appearance using One Light and One Dark, maps
application surfaces/statuses from Zed semantic tokens, and persists an
explicit user theme preference.

The workspace shell also follows Zed's split geometry instead of treating the
sidebar as a fixed web rail. The primary navigation can be collapsed or resized
between 168 and 420 pixels by pointer, keyboard arrows/Home/End, or AccessKit
SetValue/Increment/Decrement; its width is restored with the list/detail split,
tabs, active entity, fullscreen detail state, and navigation history. The
command palette and editable shortcut registry cover back/forward, close/reopen,
next/previous tab traversal, detail fullscreen, sidebar visibility, section
navigation, document creation, and task branch-name copying.

The list/detail divider follows the same native splitter contract: pointer
dragging, Left/Right/Home/End, AccessKit SetValue/Increment/Decrement, focus and
hover accents, a 20–80% range, and double-click reset. Entity tabs use Zed's
stable-width horizontal overflow behavior with a tracked GPUI scroll handle;
opening or cycling to an offscreen tab reveals it without squeezing every tab
into unreadable fragments.

Unified Soup rows now preserve Macro's email-first unread contract and its
notification fallback instead of rendering every entity with identical visual
weight. Unread rows use a stable accent indicator and stronger Zed semantic
ink, while selection replaces that indicator without shifting row content.
The shared list action model distinguishes Enter fullscreen from Shift+Enter
split-open, Space preview, and `e` mark-done. Rows expose those shortcuts,
selection state, logical position, and the keyboard-focused active descendant
through AccessKit; multi-select controls are announced as real checkboxes.
Inbox defaults to Macro's Unread view and cycles through All, Unread, and Read;
the choice is account-scoped, saved-view/deep-link serializable, and compiled
to the backend's entity-specific `ns`/`NotificationSeen` Soup predicates.
Mail threads, Documents, Tasks, and Agents now share the same `j`/`k`, Space,
Enter, Shift+Enter, `x`, and `e` interaction reducer and stable multi-selection
presentation rather than implementing unrelated per-screen keyboard behavior.
Calendar uses that same Soup reducer. Activity and Reminders use their native
typed stores with the equivalent stable-ID focus and multi-selection contract:
arrows/`j`/`k` move, `x` selects, Enter/Space opens or edits, and `e` performs
real done/completion mutations; reminder bulk completion is undoable.
CRM list and grouped-list modes project the filtered backend records into their
visual row order before handling arrows/`j`/`k`, `x`, Enter, Shift+Enter, or
Space. Rows and the select-all control expose checkbox, position, set-size,
shortcut, and active-descendant semantics while retaining the existing real
stage, owner, revenue, saved-view, and bulk-mutation contracts.
Active Calls uses the same Zed keyboard focus language: arrows/`j`/`k` move
between live rooms and Enter/Space invokes the existing native RTC join path.
The live-room list exposes named list items, set positions, join shortcuts, and
its active descendant without replacing the call controls with mock actions.
The hierarchical Files browser uses a dedicated stable-ID tree reducer instead
of pretending folders are a flat Soup list. It preserves backend sibling order,
tracks collapse and focus across refreshes, and supports Arrow/j-k traversal,
Home/End, Left/Right parent-child navigation, Enter/Space opening, and pointer
disclosures. The folder pane is exposed as an AccessKit tree with named tree
items, levels, sibling positions, selection, expansion state, shortcuts, and an
active descendant while retaining the same Zed selection and focus styling.

Document mentions no longer default to a raw backend-ID form. The native editor
opens a Zed `ListItem` picker populated from the real people cache, document and
task Soup records, CRM contacts/companies, and personal/team tags. Selecting a
row inserts Macro's typed Lexical mention node at the current caret; the generic
form remains available behind Custom mention for uncommon group/date payloads.
The picker uses Zed's native single-line editor to filter cached people,
documents, tasks, customers, and tags; Up/Down move the highlighted result and
Enter inserts the selected typed Lexical entity at the current document caret.

Document, folder, and email sharing no longer expose Macro transport values or
channel IDs as text fields. The native composer uses Zed segmented buttons for
private/workspace/public links, Zed access menus, and a named workspace-channel
list backed by the real channels cache. Existing channel grants render with
their current access and Remove sends Macro's typed `remove` permission
operation before refetching the authoritative policy. Email stays channel-only
and view-only, matching Macro's backend constraint.

Entity properties also retain their typed metadata inside the native dialog.
Boolean values use toggles; select, numeric-select, and tag values use named
option rows with real selected state and color swatches; entity references use
the cached people/document/task/company picker and serialize the chosen row back
to Macro's typed property reference contract. Number, date, string, and link
properties keep Zed's native accessible text editor for direct values.

Tag creation and editing use the same fixed twelve-color palette as Macro's
open-source web client and Rust properties toolset. The GPUI dialog now exposes
a Zed-native tag-name editor, labeled color choices with visible selected state,
and Personal/Workspace sharing controls; hex values and `user`/`team` transport
tokens remain internal. Edits omit unchanged name and color fields so an
uncolored tag displayed with Macro's gray default is not silently recolored.

Property definitions use a native Zed form as well: the service's lowercase
type and scope tokens, boolean multi-select flag, and entity constraint are no
longer editable text fields. A Zed menu exposes Macro's Input, Checkbox, Date,
Link, Option, and typed-entity choices; segmented Owner and Single/Multi modes
appear only when applicable, while select options remain user-authored values.
Request validation rejects missing names, invalid numeric options, and duplicate
options before the backend mutation begins.

## Contract rules

1. Treat `macro/packages/sdk/src/config.ts` and the generated service clients as
   the current frontend contract.
2. Do not invent a REST route from a Macro web route. For example, tasks are
   documents created through `POST /documents/create_task`; there is no generic
   `GET /tasks` API in the generated storage client.
3. Preserve raw JSON at the API boundary until the corresponding generated
   response schema has been ported and tested. A guessed Rust struct that
   compiles is worse than an explicit untyped boundary.
4. Keep authentication tokens out of logs and percent-encode tokens embedded in
   connection-gateway URLs.
5. A feature is complete only when it has a real data path, loading/error/empty
   states, interactions, and tests. Rendering a mock screen does not count.

## Delivery sequence

### 1. Authentication and application state — implemented for passwordless accounts

- Passwordless-login screens and code verification.
- Access-token exchange, refresh, and platform-keychain persistence.
- Explicit restoring, signed-out, sending, verifying, ready, SSO, and failed
  states.

### 2. Read-only vertical slice — implemented

- Real unified soup/inbox with cursor pagination.
- Persistent tabs for selected server entities.
- Paginated channel lists and read-only channel timelines using generated
  storage response shapes.

### 3. First write paths — implemented

- Create a task through `/documents/create_task`.
- Send a channel message and reconcile the response with the live list.
- Compose and reply to text email through `/email/messages`.
- Create and `simple_save` Markdown documents.
- Create tasks and apply the system Completed option.
- Start and control managed agent sessions.

### 4. Realtime and editing — native transport and collaboration implemented; compatibility proof pending

- Run the connection-gateway WebSocket with heartbeat, native trust roots,
  reconnect/backoff, and invalidation reconciliation, plus Macro's canonical
  GraphQL Soup subscription for exact entity update and cache-deletion targets.
- Document collaboration runs as its own Loro/Bebop subsystem with snapshots,
  awareness, WAL replay, reconnect, comments, and version history rather than
  being collapsed into ordinary REST saves.
- Contract tests import Macro's exact 784-byte TypeScript-generated blank
  Lexical/Loro golden, preserve its stable block identity through a Rust
  snapshot round-trip, and converge a native incremental edit into a second
  client. A separate JavaScript/WASM↔Rust fixture now proves disjoint browser
  and native edits converge in both delivery orders and after duplicate frames.
  A live service-backed simultaneous/reconnect run remains pending.
- A loopback WebSocket integration test now forces an unacknowledged native
  update through disconnect/reconnect, verifies the SQLite WAL replays the exact
  encoded frame, then checks acknowledgement, snapshot persistence, remote
  update delivery, and awareness delivery. The real Macro local-stack run is
  still a separate release gate.
- The editor preserves unknown Lexical nodes losslessly and provides native
  formatting, structure commands, typed mentions, find/replace, slash commands,
  rich clipboard envelopes, and remote selection painting. Static cross-runtime
  byte compatibility now passes. Deterministic headless fixtures now dispatch a
  real AccessKit `SetValue` action and a marked-text Japanese IME composition
  through the focused Zed editor in both themes. Mixed Arabic/Hebrew/English
  document text is also rendered and asserted through AccessKit in both themes;
  live reconnect plus the wider OS VoiceOver and multilingual IME matrix remain.

### 5. Remaining product areas

Continue the remaining compatibility, interaction, accessibility, performance,
security, and distribution work as independently testable subsystems; consult
`PARITY_BASELINE.json` rather than treating the presence of a screen as parity.

## Licensing and repository shape

The current crates depend on `gpui`, `gpui_platform`, and Zed's GPL-licensed
embedded `assets`/`icons` crates. The built binary is therefore not an
Apache-only graph.
Macro's repository is AGPL-3.0; do not copy its implementation into a
differently licensed client without an explicit licensing decision. This note
is an engineering guardrail, not legal advice.
