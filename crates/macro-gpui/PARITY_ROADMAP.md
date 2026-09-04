# Macro GPUI parity roadmap

## Objective

Build a native desktop Macro client whose product behavior matches the open-source
Macro application, whose presentation and interaction primitives come from Zed,
and whose client implementation is Rust/GPUI rather than SolidJS/Tauri.

This is not a source-to-source port. Macro's current frontend contains product
state machines, collaboration logic, editor schemas, media orchestration, and
platform behavior that must be recreated against the same backend contracts.

## Sources of truth

- Macro routes and product behavior: `macro/apps/web/src/routes`,
  `macro/apps/web/src/features`, and `macro/apps/web/src/lib/queries`.
- Publicly documented behavior: `macro/apps/docs/product` and
  `macro/apps/docs/concepts`.
- Service contracts: `macro/packages/sdk/generated`, GraphQL schemas, and Bebop
  schemas. A frontend route name is never sufficient evidence for an API path.
- Realtime and collaboration compatibility: `macro/packages/collaboration`,
  `macro/packages/lexical-core`, `macro/packages/loro-mirror`, and
  `macro/services/sync-service`.
- Server behavior: Macro's Rust crates and services. The desktop client should
  consume these services rather than duplicate their business logic.
- Visual system and native interaction patterns: Zed's `theme`, `ui`, `editor`,
  `workspace`, `command_palette`, `picker`, `menu`, and virtual-list components.

## Definition of parity

A feature is complete only when it has all of the following:

1. The same production service contract as Macro, represented by typed Rust
   request, response, error, pagination, and event models.
2. Loading, empty, populated, partial, offline, reconnecting, forbidden, and
   recoverable-error states where applicable.
3. Read and mutation workflows, including undo or confirmation for destructive
   actions.
4. Entity-specific realtime reconciliation without reloading unrelated stores.
5. Keyboard, pointer, focus, clipboard, drag/drop, deep-link, and accessibility
   behavior appropriate to the feature.
6. Unit tests for reducers and models, contract fixtures for API compatibility,
   integration tests against local Macro services, and GPUI interaction tests.
7. A side-by-side parity check against the SolidJS client using the same fixture
   account. A route or mock screen does not count as parity.

## Target architecture

### Process boundaries

- Keep Macro's Rust services as the backend. Do not embed databases, queues,
  cloud workers, or service-only crates in the desktop executable.
- Share versioned schemas at the network boundary. Generate Rust client models
  from OpenAPI/GraphQL/Bebop where possible; add fixture-based compatibility
  tests where code generation is not yet available.
- Keep secrets in the OS credential store. Keep a single authenticated transport
  layer responsible for token refresh, request IDs, retry policy, and redaction.
- Run parsing, indexing, uploads, CRDT work, and media work off the GPUI render
  thread. UI state is updated through small, typed messages.

### Client crates

Evolve the current crates toward these responsibilities rather than growing one
large `macro_app.rs`:

| Crate | Responsibility |
| --- | --- |
| `macro_app` | Bootstrap, windows, deep links, lifecycle, dependency composition |
| `macro_api` | Generated service clients, auth transport, error taxonomy |
| `macro_models` | Stable domain IDs and typed wire/domain models |
| `macro_store` | Normalized entities, queries, optimistic mutations, persistence |
| `macro_realtime` | Connection gateway, event router, backoff, targeted reconciliation |
| `macro_sync` | Loro/Bebop collaboration, awareness, snapshots, WAL, offline replay |
| `macro_shell` | Zed-style workspace, panes, tabs, navigation, history, commands |
| `macro_editor` | Native block/markdown document model and GPUI editing surface |
| `macro_ui` | Zed-token application components with no service knowledge |
| feature crates | Inbox, mail, channels, docs, tasks, agents, calls, CRM, files, canvas, search, settings |

### Store rules

- One normalized record per entity ID and entity type.
- Query results store ordered IDs and pagination metadata, not copied entities.
- Components subscribe to the smallest observable slice they render.
- Mutations update an optimistic transaction, then commit or roll back.
- Realtime events patch or invalidate the affected entity/query only.
- Persist safe cache metadata and pending offline operations in SQLite. Never
  persist access tokens or confidential logs in plaintext.
- Virtualize every unbounded list and preserve scroll anchors during pagination
  and reconciliation.

## Delivery milestones

### M0 — Baseline, licensing, and parity harness

- Record the exact Macro and Zed revisions used by the port.
- Create a generated feature inventory from routes, command registrations,
  settings pages, SDK methods, and websocket event types.
- Capture sanitized fixture accounts and golden screenshots for every primary
  surface, dialog, empty state, and error state.
- Add input-to-paint, frame time, allocation, request-count, cache-hit, and
  reconnect telemetry. Establish budgets before adding more screens.
- Confirm distribution obligations: Macro is AGPL-3.0 and much of Zed UI/theme
  is GPL-3.0-or-later while GPUI itself is Apache-2.0. Obtain legal review and
  plan source/notice delivery before distributing a combined application.

Exit criteria: reproducible debug/release builds, CI on supported desktop
platforms, a machine-readable parity matrix, and performance traces for the
current app and the SolidJS reference.

### M1 — Zed-native shell and application foundation

- Use Zed semantic theme tokens everywhere; follow OS One Light/One Dark and add
  a settings-backed Zed theme selector.
- Replace custom ad-hoc controls with Zed buttons, icons, menus, popovers,
  pickers, text inputs, scrollbars, tooltips, toasts, and focus rings.
- Rebuild the shell as a pane tree: sidebar, list pane, preview/detail pane,
  resizable splits, tabs, tab history, back/forward, reopen, and session restore.
- Implement Zed-style command registration, command palette, key context, and
  editable shortcut settings.
- Finish passwordless auth, Google/enterprise SSO callbacks, token refresh,
  logout, multi-team/workspace selection, deep links, and account switching.
- Introduce `macro_store` and `macro_realtime`; remove whole-workspace loading
  transitions and broad cache clears.
- Replace eager row construction with GPUI virtual lists and stable row IDs.

Exit criteria: cached navigation and selection do not issue unrelated requests;
scrolling remains within the frame budget; light mode matches native chrome; a
realtime event changes only affected UI.

### M2 — Unified inbox, list framework, and search

- Implement the shared list framework used by inbox, mail, files, tasks, agents,
  calls, CRM, activity, reminders, and search.
- Add Signal/Noise/All, date grouping, unread state, multi-selection, keyboard
  `j`/`k`, preview with Space, fullscreen with Enter, split-open with
  Shift+Enter, mark-done with `e`, bulk actions, and cursor pagination.
- Support filters, sort, grouping, saved views, tag Any/All semantics, search
  chips, people mentions, URL/deep-link serialization, and empty/error states.
- Port unified full-text search across titles and bodies for documents, email,
  messages, call transcripts, agent chats, and extracted file content.
- Implement recent items, favorites, activity, unified notifications, reminders,
  native notifications, and reconnect-safe notification navigation.

Exit criteria: inbox and search behavior match Macro's documented shortcuts,
filters, grouping, previews, and mutations against the same fixture dataset.

### M3 — Cross-cutting entity graph, properties, and permissions

- Implement canonical entity references and bidirectional @links for users,
  groups, dates, docs, tasks, email, channels/messages, calls, agents, files,
  contacts, companies, canvases, and pull requests.
- Build a shared native mention picker, inline pill renderer, backlink panel, and
  permission propagation workflow.
- Port property definitions and values: status, priority, assignee, dates,
  numbers, strings, selects, entity links, tags, pinned properties, and system
  properties.
- Add personal/team tags with create, rename, recolor, delete, filters, and
  realtime propagation.
- Implement owner/editor/commenter/viewer permissions, public links, team and
  channel-derived access, share dialogs, invitations, and forbidden states.
- Add user, team, role, group, presence, and contact caches shared by every
  feature.

Exit criteria: an entity linked or shared from one surface appears correctly in
every other surface without a full refresh, and permission changes take effect
on open views immediately.

### M4 — Channels and messaging

- Channel list, DMs, channel creation, recipient deduplication, invitations,
  participant management, presence, typing indicators, and unread/read state.
- Rich message composer with markdown, code, lists, mentions, emoji, snippets,
  drag/drop and picker attachments, upload progress, and inline media previews.
- Main timeline plus thread pane, inline initial replies, pagination in both
  directions, scroll anchoring, jump-to-message, and copied deep links.
- Send, retry, edit with edit history, delete, reply, reactions, copy text/link,
  attachment lifecycle, and optimistic reconciliation.
- Bot identities, bot settings, webhook/API token workflows, and `@Macro` agent
  participation.
- Start/join call affordances only after the call subsystem's native media layer
  is ready.

Exit criteria: two clients can concurrently message, react, edit, type, manage a
thread, and recover across disconnect/reconnect with no duplicate or lost UI.

### M5 — Native collaborative editor and documents

This is a program of work, not a text-area replacement.

- Port the Loro client runtime to Rust: sync-service Bebop protocol, snapshots,
  version vectors, awareness, peer identity, incremental updates, reconnect,
  compaction, write-ahead log, crash recovery, and offline replay.
- Define a native block document model compatible with Macro's Lexical/Loro
  schema. Preserve unknown nodes losslessly so newer clients never destroy data.
- Implement paragraphs; H1/H2/H3; bullet, numbered, and checklist lists; quotes;
  code blocks and syntax highlighting; tables; dividers; links; images; videos;
  inline/block math; emoji; inline formatting; superscript/subscript; mentions;
  tags; snippets; and inline tasks.
- Implement selection, multi-range editing as needed, IME, accessibility,
  clipboard HTML/markdown/plaintext, drag reorder, indentation, autoformatting,
  slash menu, find/replace with regex, and undo/redo that respects remote edits.
- Add remote cursors/selections, collaboration status, comments and anchored
  threads, replies, drafts, resolve/unresolve, mentions, and notifications.
- Add outline navigation, title editing, side-panel metadata/properties,
  backlinks, word/character counts, YAML front matter, share/export, and print.
- Implement version history, historical rendering, diffing, time travel, and fork
  from a historical version.
- Cover markdown docs, tasks, snippets, agent instructions, and rich channel
  messages with the same editor core while allowing surface-specific schemas.

Exit criteria: native and SolidJS clients can edit the same document
simultaneously, including offline edits and reconnect, with byte-compatible
snapshots and no lost/duplicated nodes.

### M6 — Email

- Multiple Gmail/Workspace account connection, OAuth callbacks, account health,
  refresh status, disconnect, and per-account views.
- Unified mail list with Signal/Noise, labels, unread/read, important, star,
  archive, spam, trash, sender classification, keyboard triage, filters, and
  search.
- Thread detail with collapsed/expanded messages, sanitized HTML and plaintext,
  remote-image policy, participants, dates, quoted content, attachments, inline
  previews, download, and CRM/contact links.
- Native compose, reply, reply-all, forward, From account, To/Cc/Bcc recipient
  picker, subject, rich body, mentions, signatures, attachments, drafts and
  autosave, send, scheduling, and failure recovery.
- Share email/thread into channels; enforce view-only semantics for shared
  recipients; create linked tasks; expose tags, properties, and backlinks.
- Agent-authored drafts must open as editable drafts and require the intended
  permission/confirmation policy before sending.

Exit criteria: a user can complete their normal multi-account mail workflow
without opening the web client, with mutations reflected correctly in Gmail and
other signed-in clients.

### M7 — Tasks, projects, snippets, folders, and files

- Tasks: create/edit title and body, four status states, priority, assignees,
  dates, dependencies, properties, comments, tags, sharing, duplicates, source
  backlinks, bulk mutations, grouped/list views, and team visibility.
- GitHub linkage: connect account, link PRs, render PR state/checks, and apply
  bidirectional status transitions with conflict handling.
- Projects and saved task views with grouping, filters, ordering, and shareable
  state.
- Snippets: create/edit/delete, personal/team sharing, semicolon typeahead, and
  formatted insertion across every compatible editor.
- Files/folders: create, rename, move, copy, delete/restore, upload, staged and
  multipart progress, cancellation/resume, extraction status, attachments,
  thumbnails, previews for PDF/image/video/code, download/export, sharing, and
  search.
- Canvas: infinite viewport, pan/zoom, shapes, text, connectors, freehand,
  images, embedded live entities, selection, alignment, grouping, z-order,
  clipboard, undo/redo, collaboration, sharing, download, and framed deep links.

Exit criteria: all storage entities can be created, organized, searched, shared,
linked, edited, and recovered through native workflows.

### M8 — Agents, automations, and unified memory

- Agent session list, create, rename, resume, archive/delete, model selection,
  permissions, quotas/paywall state, and workspace/mention context selection.
- Streaming transcript with text, reasoning/status, plans, tool calls, terminal
  output, searches, file paths, diffs, artifacts, permissions, failures, retry,
  stop, compact, and reconnect/resume.
- Editable agent-created email drafts, docs, tasks, CRM changes, and channel posts
  using the same native feature components and authorization rules.
- Scheduled automations: create/edit/pause/delete, schedules and timezone,
  execution history, failures, notifications, and links to output sessions.
- `@Macro` in channels and cross-feature agent dispatch.
- Unified personal/team memory display and controls exposed by the backend,
  without moving memory synthesis into the desktop process.
- MCP connection/setup surfaces, external client instructions, and connected-tool
  state.

Exit criteria: long-running agents survive app restarts and network changes,
their actions reconcile into native stores, and every tool result has a faithful
inspectable representation.

### M9 — CRM

- Contacts and companies derived from email, enrichment state, first/last
  interaction, associated people, threads, files, and backlinks.
- Board with pipeline stages and drag/drop optimistic mutation; list/table with
  inline editing, columns, grouping, filtering, sorting, saved/shareable views,
  and virtualization in both dimensions.
- Stage, owner, revenue, last interaction, custom properties, tags, and bulk
  updates.
- Record discussions using the channel/message subsystem, including mentions and
  notifications.
- Team Email Sync controls, hidden records, admin-only actions, domain grouping,
  generic-domain exclusions, and permission-sensitive email visibility.

Exit criteria: company/contact records and their email/discussion context match
the web client's visibility and mutation behavior for members and admins.

### M10 — Calls and recordings

- Reuse the Zed/LiveKit Rust stack where compatible; isolate it behind a native
  call controller with explicit state transitions.
- Start, ring, accept, decline, join, leave, reconnect, and end calls from Calls
  and Channels.
- Microphone/camera permissions and device selection; mute, video, speaker,
  screen share, participant grid, active speaker, network quality, and device
  hot-plug/interruption handling.
- Recording consent and personal/team-memory opt-out state.
- Past-call list and detail: participants, summary, video/audio playback,
  transcript synchronized to playback, search, tags, properties, sharing, and
  agent actions.
- Platform integrations such as CallKit/PushKit and background behavior are
  separate platform milestones after desktop call correctness.

Exit criteria: multi-party calls remain usable across device changes and network
reconnects, and completed recordings/transcripts reconcile into search and
memory.

### M11 — Settings, integrations, onboarding, and administration

- Settings: account/profile, appearance/theme, shortcuts, notifications, email,
  signatures, agents/model defaults, CRM, tags, teams, roles/permissions, bots,
  billing, mobile, data/privacy, and account deletion feedback.
- Integrations: Gmail/Workspace, GitHub, Pipedream/MCP tools, Cursor/coding-agent
  connections, OAuth callback/deep-link lifecycle, scopes, reconnect, revoke,
  error diagnostics, and import workflows for Notion/Linear/Slack where exposed.
- Onboarding: auth, team creation/join, email connection, workspace bootstrap,
  invitations, sample/starter content, progress resume, and skipped-state repair.
- Team/channel invitations, acceptance routes, admin controls, quotas, feature
  gates, upgrade/paywall states, and billing handoff.

Exit criteria: a new user can install, authenticate, onboard, connect services,
invite a teammate, configure the app, and delete their account without using the
SolidJS client.

### M12 — Distribution and parity closure

- Stable bundle identity, hardened runtime, entitlements, code signing,
  notarization, updater with rollback, crash reporting, protocol/deep-link
  registration, and migration of stored credentials/settings.
- Accessibility audit, reduced motion, high contrast, keyboard-only audit, IME,
  RTL readiness, localization boundaries, and screen-reader labels.
- Security review for HTML email, uploads, URL handling, OAuth, credentials,
  logs, clipboard, link previews, and agent action confirmation.
- Performance soak tests with large inboxes, channels, documents, CRM tables,
  canvases, and long agent transcripts.
- Complete the generated parity matrix. Remove the SolidJS desktop runtime only
  after all release-blocking rows pass; retain the web/mobile app unless they are
  separately being replaced.

Exit criteria: signed release artifacts pass the full behavior, compatibility,
performance, accessibility, security, migration, and rollback suites.

## Workstreams that must remain continuous

### Contract compatibility

- Check generated SDK/schema diffs in CI against the pinned Macro revision.
- Fail builds when a consumed endpoint or event changes without regenerated Rust
  models and fixtures.
- Round-trip unknown enum values and fields where forward compatibility matters.

### Performance budgets

- Cached selection and navigation should paint within one frame whenever no
  service request is required.
- Target p95 input-to-paint below 50 ms for ordinary actions.
- Maintain display-rate scrolling for large virtual lists and long documents.
- A realtime event may not trigger unrelated top-level queries.
- Track allocations and clone sizes in render paths; domain entities stay in the
  store and rows receive stable IDs or shared references.

### Testing pyramid

- Pure Rust unit and property tests for reducers, parsers, editor transforms,
  CRDT operations, and permission decisions.
- Recorded contract fixtures for every service and websocket message family.
- Local-stack integration tests for auth, search, sync, uploads, email, calls,
  notifications, and agent lifecycle.
- Two-client collaboration/realtime tests, including partitions and reordered or
  duplicated messages.
- GPUI interaction tests for focus, keyboard commands, menus, drag/drop, splits,
  dialogs, and accessibility metadata.
- Golden rendering checks for One Light and One Dark at multiple scale factors.

## Recommended execution model

Run platform work continuously and deliver vertical slices through shared
foundations. Do not build every list first and postpone all mutations; complete
one production workflow at a time while extracting reusable infrastructure.

A credible team is approximately six to eight experienced engineers: shell and
store, editor/collaboration, mail, comms/calls, agents, and product-platform QA.
The native editor/collaboration, email, calls, and canvas are each substantial
projects. A single engineer should treat full parity as a multi-year effort; a
staffed team should plan in release trains and re-estimate after M1, M5, and M10
rather than promise a date from the current prototype.

## Immediate next sequence

1. Finish M1: normalized store, event-specific reconciliation, virtual lists,
   split workspace, command system, and persisted theme selection.
2. Make Inbox the parity reference surface in M2, including the exact three-pane
   interaction shown by the current Macro app.
3. Build the shared entity graph/properties/permissions foundation in M3.
4. Deliver Channels in M4 to validate realtime, mentions, uploads, and threads.
5. Start the editor/CRDT compatibility program early and do not label document,
   task, or rich-message parity complete until M5 passes cross-client tests.
