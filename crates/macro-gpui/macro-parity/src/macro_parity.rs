use serde_json::Value;
use std::collections::HashSet;

pub const BASELINE: &str = include_str!("../../PARITY_BASELINE.json");
pub const SOURCE_INVENTORY: &str = include_str!("../../SOURCE_INVENTORY.json");
pub const FIXTURE_MANIFEST: &str = include_str!("../../PARITY_FIXTURE_MANIFEST.json");
pub const RELEASE_GATES: &str = include_str!("../../PARITY_RELEASE_GATES.json");
pub const MACOS_INFO_PLIST: &str = include_str!("../../macro-app/resources/macos/Info.plist");
pub const MACOS_ENTITLEMENTS: &str =
    include_str!("../../macro-app/resources/macos/Macro.entitlements");
pub const MACOS_BUNDLE_SCRIPT: &str = include_str!("../../scripts/bundle-macos.sh");
pub const MACOS_CI_WORKFLOW: &str = include_str!("../../../../.github/workflows/macro_gpui.yml");
pub const GPUI_CAPTURE_SCRIPT: &str = include_str!("../../scripts/capture-gpui-fixtures.py");
pub const GPUI_PERFORMANCE_SCRIPT: &str =
    include_str!("../../scripts/measure-gpui-interactions.py");
pub const SOLID_REFERENCE_PLAN: &str = include_str!("../../SOLID_REFERENCE_CAPTURE_PLAN.json");
pub const SOLID_REFERENCE_CAPTURE_SCRIPT: &str =
    include_str!("../../scripts/capture-solid-reference.py");
pub const PARITY_COMPARISON_SCRIPT: &str = include_str!("../../scripts/compare-parity-captures.py");
pub const SOLID_REFERENCE_PLAYWRIGHT_CONFIG: &str =
    include_str!("../../solid-reference/playwright.config.ts");
pub const SOLID_REFERENCE_PLAYWRIGHT_SPEC: &str =
    include_str!("../../solid-reference/solid-reference.spec.ts");
pub const MACRO_APP_SOURCE: &str = include_str!("../../macro-app/src/macro_app.rs");
pub const EMBEDDED_VIDEO_SOURCE: &str = include_str!("../../macro-app/src/embedded_video.rs");
pub const MACRO_API_SOURCE: &str = include_str!("../../macro-api/src/macro_api.rs");
pub const MACRO_CANVAS_SOURCE: &str = include_str!("../../macro-canvas/src/macro_canvas.rs");
pub const MACRO_REALTIME_SOURCE: &str = include_str!("../../macro-realtime/src/macro_realtime.rs");
pub const MACRO_LAYOUT_SOURCE: &str = include_str!("../../macro-layout/src/macro_layout.rs");
pub const MACRO_LIST_SOURCE: &str = include_str!("../../macro-list/src/macro_list.rs");
pub const MACRO_UPDATER: &str = include_str!("../../macro-updater/src/macro_updater.rs");
pub const SYNC_TYPESCRIPT_FIXTURES: &str =
    include_str!("../../macro-sync/fixtures/sync_bebop_typescript.json");
pub const MARKDOWN_TYPESCRIPT_GOLDEN: &str =
    include_str!("../../macro-sync/fixtures/markdown_golden_typescript.json");
pub const MARKDOWN_TWO_CLIENT_FIXTURE: &str =
    include_str!("../../macro-sync/fixtures/markdown_two_client_typescript.json");
pub const SYNC_FIXTURE_PACKAGE_LOCK: &str =
    include_str!("../../macro-sync/fixtures/package-lock.json");

const BUNDLE_IDENTIFIER: &str = "com.macro.desktop.gpui";

pub fn validate() -> Result<(), String> {
    let baseline: Value = serde_json::from_str(BASELINE).map_err(|error| error.to_string())?;
    let inventory: Value =
        serde_json::from_str(SOURCE_INVENTORY).map_err(|error| error.to_string())?;
    let fixture_manifest: Value =
        serde_json::from_str(FIXTURE_MANIFEST).map_err(|error| error.to_string())?;
    let release_gates: Value =
        serde_json::from_str(RELEASE_GATES).map_err(|error| error.to_string())?;
    let solid_reference_plan: Value =
        serde_json::from_str(SOLID_REFERENCE_PLAN).map_err(|error| error.to_string())?;
    let sync_fixtures: Value =
        serde_json::from_str(SYNC_TYPESCRIPT_FIXTURES).map_err(|error| error.to_string())?;
    let markdown_golden: Value =
        serde_json::from_str(MARKDOWN_TYPESCRIPT_GOLDEN).map_err(|error| error.to_string())?;
    let markdown_two_client: Value =
        serde_json::from_str(MARKDOWN_TWO_CLIENT_FIXTURE).map_err(|error| error.to_string())?;
    let sync_fixture_package_lock: Value =
        serde_json::from_str(SYNC_FIXTURE_PACKAGE_LOCK).map_err(|error| error.to_string())?;
    let features = baseline["features"]
        .as_array()
        .ok_or("parity baseline has no feature array")?;
    let mut ids = HashSet::new();
    let mut milestones = HashSet::new();
    for feature in features {
        let id = feature["id"].as_str().ok_or("feature has no id")?;
        if !ids.insert(id) {
            return Err(format!("duplicate parity feature id: {id}"));
        }
        milestones.insert(
            feature["milestone"]
                .as_str()
                .ok_or_else(|| format!("feature {id} has no milestone"))?,
        );
        let status = feature["status"]
            .as_str()
            .ok_or_else(|| format!("feature {id} has no status"))?;
        if !matches!(
            status,
            "not_started" | "partial" | "implemented" | "external_review_required"
        ) {
            return Err(format!("feature {id} has unknown status {status}"));
        }
    }
    for index in 0..=12 {
        let milestone = format!("M{index}");
        if !milestones.contains(milestone.as_str()) {
            return Err(format!("missing {milestone}"));
        }
    }
    let intentionally_open = HashSet::from([
        "M0.fixture-goldens",
        "M0.license-review",
        "M12.packaging-signing-updates",
        "M12.accessibility-security",
        "M12.parity-closure",
    ]);
    for feature in features {
        let id = feature["id"].as_str().ok_or("feature has no id")?;
        let status = feature["status"].as_str().ok_or("feature has no status")?;
        if !intentionally_open.contains(id) && status != "implemented" {
            return Err(format!(
                "locally implemented parity row {id} regressed to {status}"
            ));
        }
    }
    if baseline["source_revisions"]["macro"] != inventory["macro_revision"] {
        return Err("Macro revision does not match generated source inventory".into());
    }
    if baseline["source_revisions"]["zed"] != inventory["zed_revision"] {
        return Err("Zed revision does not match generated source inventory".into());
    }
    if baseline["source_revisions"]["macro"] != sync_fixtures["macro_revision"] {
        return Err("TypeScript sync fixtures do not match the pinned Macro revision".into());
    }
    if baseline["source_revisions"]["macro"] != markdown_golden["macro_revision"] {
        return Err("TypeScript Markdown golden does not match the pinned Macro revision".into());
    }
    if markdown_golden["schema"] != "macro.markdown-loro.typescript.v1"
        || markdown_golden["source"] != "static_assets/markdown-golden.1.bin"
        || markdown_golden["generator"]
            .as_str()
            .is_none_or(|generator| !generator.contains("loro-crdt 1.13.7"))
        || markdown_golden["snapshot_hex"]
            .as_str()
            .is_none_or(|snapshot| snapshot.len() != 1_568)
    {
        return Err("TypeScript Markdown golden is incomplete or has invalid provenance".into());
    }
    if baseline["source_revisions"]["macro"] != markdown_two_client["macro_revision"]
        || markdown_two_client["schema"] != "macro.markdown-loro.two-client.v1"
        || markdown_two_client["generator"]
            .as_str()
            .is_none_or(|generator| !generator.contains("loro-crdt 1.13.7"))
        || markdown_two_client["seed_snapshot_hex"]
            .as_str()
            .is_none_or(|snapshot| snapshot.len() != 4_700)
        || markdown_two_client["browser_update_hex"]
            .as_str()
            .is_none_or(|update| update.len() != 196)
        || markdown_two_client["native_update_hex"]
            .as_str()
            .is_none_or(|update| update.len() != 214)
        || markdown_two_client["expected_converged"].is_null()
    {
        return Err(
            "native/TypeScript Markdown convergence fixture is incomplete or invalid".into(),
        );
    }
    let collaboration_coverage = markdown_two_client["coverage"]
        .as_array()
        .ok_or("native/TypeScript Markdown fixture has no coverage declaration")?;
    for required in ["paragraph", "heading", "checklist", "unknown_opaque_block"] {
        if !collaboration_coverage
            .iter()
            .any(|coverage| coverage == required)
        {
            return Err(format!(
                "native/TypeScript Markdown fixture does not cover {required}"
            ));
        }
    }
    if sync_fixtures["frames"]
        .as_object()
        .is_none_or(|frames| frames.len() != 11)
    {
        return Err("TypeScript sync fixtures do not cover every Bebop union variant".into());
    }
    let locked_loro = &sync_fixture_package_lock["packages"]["node_modules/loro-crdt"];
    if locked_loro["version"] != "1.13.7"
        || locked_loro["integrity"]
            .as_str()
            .is_none_or(|integrity| !integrity.starts_with("sha512-") || integrity.len() < 20)
    {
        return Err("JavaScript collaboration verifier does not lock loro-crdt 1.13.7".into());
    }
    if inventory["sources"]["routes"]
        .as_array()
        .is_none_or(|routes| routes.len() < 20)
    {
        return Err("source inventory did not capture Macro routes".into());
    }
    if inventory["sources"]["websocket_events"]
        .as_array()
        .is_none_or(|events| events.len() < 10)
    {
        return Err("source inventory did not capture websocket events".into());
    }
    if inventory["schema_version"]
        .as_u64()
        .is_none_or(|version| version < 2)
    {
        return Err("source inventory schema does not include behavioral surfaces".into());
    }
    let sdk_methods = inventory["sources"]["generated_sdk_methods"]
        .as_object()
        .ok_or("source inventory did not capture generated SDK methods")?;
    let sdk_method_count = sdk_methods
        .values()
        .map(|methods| methods.as_array().map_or(0, Vec::len))
        .sum::<usize>();
    if sdk_methods.len() < 10 || sdk_method_count < 100 {
        return Err("source inventory did not capture the generated SDK surface".into());
    }
    if inventory["sources"]["command_registrations"]
        .as_array()
        .is_none_or(|commands| commands.len() < 25)
    {
        return Err("source inventory did not capture command registrations".into());
    }
    if inventory["sources"]["settings_surfaces"]
        .as_array()
        .is_none_or(|surfaces| surfaces.len() < 15)
    {
        return Err("source inventory did not capture settings surfaces".into());
    }
    validate_fixture_manifest(&fixture_manifest)?;
    validate_release_gates(&baseline, &fixture_manifest, &release_gates)?;
    validate_solid_reference_plan(&baseline, &fixture_manifest, &solid_reference_plan)?;
    if !MACOS_INFO_PLIST.contains(&format!("<string>{BUNDLE_IDENTIFIER}</string>")) {
        return Err("macOS Info.plist does not use the stable bundle identifier".into());
    }
    for localization_requirement in [
        "CFBundleDevelopmentRegion",
        "CFBundleLocalizations",
        "public.app-category.productivity",
    ] {
        if !MACOS_INFO_PLIST.contains(localization_requirement) {
            return Err(format!(
                "macOS bundle is missing its localization boundary: {localization_requirement}"
            ));
        }
    }
    for entitlement in [
        "com.apple.security.device.audio-input",
        "com.apple.security.device.camera",
        "com.apple.security.files.user-selected.read-write",
        "com.apple.security.network.client",
    ] {
        if !MACOS_ENTITLEMENTS.contains(entitlement) {
            return Err(format!("macOS bundle is missing entitlement {entitlement}"));
        }
    }
    for crash_requirement in [
        "--crash-handler",
        "crashes::crash_server",
        "crashes::init",
        "crash_reports_enabled",
        "macro-gpui/crashes",
    ] {
        if !MACRO_APP_SOURCE.contains(crash_requirement) {
            return Err(format!(
                "native crash reporting is missing required behavior: {crash_requirement}"
            ));
        }
    }
    for accessibility_requirement in [
        "cx.set_reduce_motion",
        "macro_theme::set_high_contrast",
        "toggle-reduce-motion",
        "toggle-high-contrast",
    ] {
        if !MACRO_APP_SOURCE.contains(accessibility_requirement) {
            return Err(format!(
                "native accessibility preferences are missing required behavior: {accessibility_requirement}"
            ));
        }
    }
    validate_pointer_accessibility(MACRO_APP_SOURCE)?;
    for shell_requirement in [
        "DraggedSidebarHandle",
        "SIDEBAR-RESIZE-HANDLE",
        "Role::Splitter",
        "AccessibleAction::SetValue",
        "AccessibleAction::Increment",
        "AccessibleAction::Decrement",
        "on_sidebar_resize",
        "MIN_SIDEBAR_WIDTH",
        "MAX_SIDEBAR_WIDTH",
        "DEFAULT_SPLIT_POSITION",
        "SPLIT_POSITION_STEP",
        "Resize list and detail panes",
        "tab_scroll_handle",
        "scroll_to_item",
        "overflow_x_scroll",
    ] {
        if !MACRO_LAYOUT_SOURCE.contains(shell_requirement) {
            return Err(format!(
                "Zed-style split shell is missing required behavior: {shell_requirement}"
            ));
        }
    }
    for history_requirement in [
        "sidebar_width",
        "navigation_history",
        "SelectNextTab",
        "SelectPreviousTab",
        "ToggleDetailFullscreen",
        "cycled_tab_index",
        "ReopenClosedTab",
        "shortcut_overrides",
        "command_palette",
    ] {
        if !MACRO_APP_SOURCE.contains(history_requirement) {
            return Err(format!(
                "native command/history shell is missing required behavior: {history_requirement}"
            ));
        }
    }
    for auth_requirement in [
        "auth_generation",
        "auth_transition_is_current",
        "reset_account_scoped_state",
        "account_sessions",
        "prepare_account_session_restore",
        "this.api.set_token(macro_api_token)",
        "channel_outbox_restored_accounts.clear()",
        "document_editors.clear()",
        "canvas_models.clear()",
    ] {
        if !MACRO_APP_SOURCE.contains(auth_requirement) {
            return Err(format!(
                "native multi-account isolation is missing required behavior: {auth_requirement}"
            ));
        }
    }
    if !MACRO_API_SOURCE
        .contains("Exchanging an auth-service access token must not implicitly change")
    {
        return Err("Macro API token exchange still has implicit session side effects".into());
    }
    for list_requirement in [
        "pub const ALL: [Self; 3]",
        "DateGroup",
        "OpenSplit",
        "ReadFilter",
        "inbox_default",
        "pub fn is_unread",
        "notification_is_unread",
        "selected_count",
        "TreeInteraction",
        "visible_rows",
        "collapse_or_ascend",
        "Malformed cyclic server data must remain visible",
    ] {
        if !MACRO_LIST_SOURCE.contains(list_requirement) {
            return Err(format!(
                "native shared-list contract is missing required behavior: {list_requirement}"
            ));
        }
    }
    for list_accessibility_requirement in [
        "Shift+Enter opens in the split",
        "aria_keyshortcuts",
        "aria_active_descendant",
        "Role::CheckBox",
        "load_more_inbox_view",
        "shared_list_key_down",
        "activity_key_down",
        "reminders_key_down",
        "complete_reminders",
        "crm_key_down",
        "visible_crm_list_items",
        "Up and Down move between customers",
        "calls_key_down",
        "active-calls",
        "Escape returns to recordings",
        "list_interactions",
        "NotificationSeen",
    ] {
        if !MACRO_APP_SOURCE.contains(list_accessibility_requirement) {
            return Err(format!(
                "native shared-list accessibility is missing required behavior: {list_accessibility_requirement}"
            ));
        }
    }
    for search_requirement in [
        "search_editor",
        "ZedTextEditor::single_line",
        "Search all Macro content",
        "Role::TextInput",
        "AccessibleAction::SetValue",
        "search_results_focus_handle",
        "SelectNextSearchResult",
        "SelectPreviousSearchResult",
        "OpenSearchResultSplit",
        "Up and Down move between results",
    ] {
        if !MACRO_APP_SOURCE.contains(search_requirement) {
            return Err(format!(
                "native unified search is missing Zed input/list behavior: {search_requirement}"
            ));
        }
    }
    for search_contract_requirement in [
        "UnifiedSearchRequest",
        "UnifiedSearchEntityFilters",
        "UnifiedSearchResponse",
        "UnifiedSearchResponseItem",
        "UnifiedSearchResultKind",
        "Unknown(String)",
        "request: &UnifiedSearchRequest",
        "Result<UnifiedSearchResponse>",
    ] {
        if !MACRO_API_SOURCE.contains(search_contract_requirement) {
            return Err(format!(
                "native unified search is missing its typed Macro SDK contract: {search_contract_requirement}"
            ));
        }
    }
    for email_contract_requirement in [
        "EmailThreadPreviewsResponse",
        "GetEmailThreadResponse",
        "EmailThreadDetail",
        "EmailMessageDetail",
        "ParsedEmailMessage",
        "ListEmailLinksResponse",
        "GetEmailAttachmentDocumentIdResponse",
        "SendEmailMessageResponse",
        "Result<GetEmailThreadResponse>",
        "Result<Vec<ParsedEmailMessage>>",
        "Result<ListEmailLinksResponse>",
        "Result<SendEmailMessageResponse>",
    ] {
        if !MACRO_API_SOURCE.contains(email_contract_requirement) {
            return Err(format!(
                "native email workflow is missing its typed Macro SDK contract: {email_contract_requirement}"
            ));
        }
    }
    for file_tree_requirement in [
        "Role::Tree",
        "Role::TreeItem",
        "file_tree_key_down",
        "toggle_expanded",
        "Left and Right collapse or expand",
        "aria_level",
        "aria_position_in_set",
    ] {
        if !MACRO_APP_SOURCE.contains(file_tree_requirement) {
            return Err(format!(
                "native Files hierarchy is missing tree behavior: {file_tree_requirement}"
            ));
        }
    }
    for mention_picker_requirement in [
        "DocumentMentionChoice",
        "document_mention_choices",
        "render_document_mention_picker",
        "People and workspace entities",
        "Custom mention…",
        "ZedListItem::new",
        "choose_document_mention",
        "document_mention_picker_key_down",
        "document_mention_matches_query",
        "Search people, documents, customers, and tags",
    ] {
        if !MACRO_APP_SOURCE.contains(mention_picker_requirement) {
            return Err(format!(
                "native entity mention picker is missing required behavior: {mention_picker_requirement}"
            ));
        }
    }
    for sharing_requirement in [
        "render_entity_sharing_fields",
        "Available sharing channels",
        "Control link visibility and workspace channel access",
        "remove_entity_channel_share",
        "remove_channel_share_request",
        "ShareUpdateOperation::Remove",
    ] {
        if !MACRO_APP_SOURCE.contains(sharing_requirement) {
            return Err(format!(
                "native sharing composer is missing required behavior: {sharing_requirement}"
            ));
        }
    }
    for property_requirement in [
        "PropertyEditorSpec",
        "render_entity_property_fields",
        "Property options",
        "Property entity results",
        "property_reference_for_mention_choice",
        "toggle_property_token_value",
        "Choose typed property values without copying backend IDs",
    ] {
        if !MACRO_APP_SOURCE.contains(property_requirement) {
            return Err(format!(
                "native property editor is missing required behavior: {property_requirement}"
            ));
        }
    }
    for tag_requirement in [
        "TAG_COLOR_OPTIONS",
        "DEFAULT_TAG_COLOR",
        "render_tag_editor_fields",
        "Tag colors",
        "tag_editor_update_request",
        "Macro's native tag palette",
    ] {
        if !MACRO_APP_SOURCE.contains(tag_requirement) {
            return Err(format!(
                "native tag editor is missing required behavior: {tag_requirement}"
            ));
        }
    }
    for property_definition_requirement in [
        "render_property_definition_fields",
        "property_definition_supports_multi",
        "property_definition_has_options",
        "Property type",
        "Define reusable metadata with native types and ownership",
        "Property options must be unique",
    ] {
        if !MACRO_APP_SOURCE.contains(property_definition_requirement) {
            return Err(format!(
                "native property-definition editor is missing required behavior: {property_definition_requirement}"
            ));
        }
    }
    for entity_link_requirement in [
        "EntityLinkChoice",
        "entity_link_choices",
        "render_entity_link_picker",
        "Linkable entities",
        "entity_link_picker_key_down",
        "entity_link_matches_query",
        "Choose a workspace entity to create a bidirectional link",
    ] {
        if !MACRO_APP_SOURCE.contains(entity_link_requirement) {
            return Err(format!(
                "native entity-link picker is missing required behavior: {entity_link_requirement}"
            ));
        }
    }
    for entity_graph_requirement in [
        "load_entity_backlinks",
        "entity_reference_label",
        "open_linked_entity",
        "Open backlink from",
        "Remove link to",
    ] {
        if !MACRO_APP_SOURCE.contains(entity_graph_requirement) {
            return Err(format!(
                "resolved entity graph is missing required behavior: {entity_graph_requirement}"
            ));
        }
    }
    for permission_boundary_requirement in [
        "EntityAccessGate",
        "entity_access_gate_from_permission",
        "You don’t have access",
        "Access could not be verified",
        "Check access again",
    ] {
        if !MACRO_APP_SOURCE.contains(permission_boundary_requirement) {
            return Err(format!(
                "fail-closed entity permission boundary is missing required behavior: {permission_boundary_requirement}"
            ));
        }
    }
    for permission_contract_requirement in [
        "ApiEntityPermission",
        "GetEntityPermissionResponse",
        "Result<GetEntityPermissionResponse>",
        "entity_permission_from_api",
    ] {
        if !MACRO_API_SOURCE.contains(permission_contract_requirement)
            && !MACRO_APP_SOURCE.contains(permission_contract_requirement)
        {
            return Err(format!(
                "entity permission boundary is missing its typed Macro SDK contract: {permission_contract_requirement}"
            ));
        }
    }
    for soup_realtime_requirement in [
        "SoupUpdatesStream",
        "graphql-transport-ws",
        "GraphqlCacheDeletion",
        "soup_websocket_url",
    ] {
        if !MACRO_API_SOURCE.contains(soup_realtime_requirement) {
            return Err(format!(
                "Macro Soup websocket transport is missing required behavior: {soup_realtime_requirement}"
            ));
        }
    }
    for soup_patch_requirement in ["SoupRealtimePatch", "parse_soup_realtime_patches"] {
        if !MACRO_REALTIME_SOURCE.contains(soup_patch_requirement) {
            return Err(format!(
                "Macro Soup patch parser is missing required behavior: {soup_patch_requirement}"
            ));
        }
    }
    for soup_app_requirement in [
        "start_soup_realtime",
        "handle_soup_realtime_patches",
        "soup_entity_tombstones",
        "load_entity_capabilities_targeted",
    ] {
        if !MACRO_APP_SOURCE.contains(soup_app_requirement) {
            return Err(format!(
                "targeted Soup reconciliation is missing required behavior: {soup_app_requirement}"
            ));
        }
    }
    for channel_conversation_api_requirement in [
        "get_or_create_direct_message",
        "get_or_create_private_channel",
        "/channels/get_or_create_dm",
        "/channels/get_or_create_private",
        "get_channel_join_link",
        "join_channel_by_code",
        "/join-link",
        "/channels/join/",
        "upload_static_file",
    ] {
        if !MACRO_API_SOURCE.contains(channel_conversation_api_requirement) {
            return Err(format!(
                "Macro channel conversation API is missing required behavior: {channel_conversation_api_requirement}"
            ));
        }
    }
    for channel_invite_app_requirement in [
        "ChannelInvite",
        "channel-invite",
        "open_channel_invite",
        "copy_channel_join_link",
        "channel_invite_url",
        "NewConversation",
        "open_direct_message",
        "Copy invite link",
        "Confirm that you want to join this private Macro channel",
        "record_channel_typing_keystroke",
        "stop_local_channel_typing",
        "LocalChannelTypingState",
        "Duration::from_secs(4)",
        "Duration::from_secs(2)",
        "reconcile_channel_summary_realtime",
        "ChannelSummaryReconcileOutcome::ReloadRequired",
        "latest_non_thread_message",
        "channel_has_unread",
        "if unread { \"unread\" } else { \"read\" }",
        "ChannelComposerAttachmentKind",
        "insert_channel_composer_mention",
        "render_channel_composer_tools",
        "channel_composer_attachments",
        "channel_edit_payload",
        "channel_original_attachments",
        "optimistic_edited_channel_attachments",
        "_gpui_reaction_nonce",
        "server_attachment_id",
        "render_channel_attachment_chip",
        "static/image",
        "static/video",
        "channel_message_scroll_anchor",
        "channel_message_target_is_loaded",
        "scroll_to_item_strict",
        "channel_message_deep_link",
        "uniform_list_has_deferred_scroll",
        "A matching realtime frame can acknowledge and replace",
        "reconcile_open_channels_after_reconnect",
        "channel_typing_users.clear()",
        "two_clients_converge_after_concurrent_channel_work_and_reconnect_refresh",
    ] {
        if !MACRO_APP_SOURCE.contains(channel_invite_app_requirement) {
            return Err(format!(
                "native channel conversation flow is missing required behavior: {channel_invite_app_requirement}"
            ));
        }
    }
    for channel_bot_api_requirement in [
        "list_bots",
        "create_bot",
        "patch_bot",
        "delete_bot",
        "create_bot_token",
        "list_bot_tokens",
        "revoke_bot_token",
        "list_channel_bots",
        "add_bot_to_channel",
        "create_channel_scoped_bot",
        "remove_bot_from_channel",
        "list_webhooks",
        "create_webhook",
        "patch_webhook",
        "delete_webhook",
        "validate_webhook",
        "/channels/{}/bots/scoped",
        "/webhook/webhooks/{}/validate",
    ] {
        if !MACRO_API_SOURCE.contains(channel_bot_api_requirement) {
            return Err(format!(
                "Macro bot/webhook API is missing required behavior: {channel_bot_api_requirement}"
            ));
        }
    }
    for channel_bot_app_requirement in [
        "render_channel_bots",
        "render_bot_integrations",
        "render_webhook_integrations",
        "render_add_channel_bot_picker",
        "render_bot_agent_toggle",
        "render_webhook_choice",
        "copy_channel_webhook_url",
        "channel_bot_webhook_url",
        "x-macro-channel-bot-token",
        "one_time_secret",
        "MACRO_AGENT_PRINCIPAL_ID",
    ] {
        if !MACRO_APP_SOURCE.contains(channel_bot_app_requirement) {
            return Err(format!(
                "native channel bot/webhook flow is missing required behavior: {channel_bot_app_requirement}"
            ));
        }
    }
    for canvas_entity_requirement in [
        "canvas_entity_choices",
        "render_canvas_entity_picker",
        "Embeddable entities",
        "canvas_entity_picker_key_down",
        "canvas_mention_wire_type",
        "Embed a live Macro entity without copying backend IDs",
    ] {
        if !MACRO_APP_SOURCE.contains(canvas_entity_requirement) {
            return Err(format!(
                "native canvas entity picker is missing required behavior: {canvas_entity_requirement}"
            ));
        }
    }
    for task_project_requirement in [
        "open_edit_task_properties",
        "bulk_set_task_status",
        "bulk_mutate_task_tag",
        "render_task_duplicates",
        "render_task_github_pull_requests",
        "copy_task_branch_name",
        "open_new_list_saved_view",
        "apply_list_saved_view",
        "open_new_project",
        "open_move_project",
        "open_delete_project",
    ] {
        if !MACRO_APP_SOURCE.contains(task_project_requirement) {
            return Err(format!(
                "native task/project workflow is missing required behavior: {task_project_requirement}"
            ));
        }
    }
    for task_project_api_requirement in [
        "create_task",
        "get_task_duplicates",
        "dismiss_task_duplicates",
        "get_document_branch_name",
        "get_document_github_pull_requests",
        "create_saved_view",
        "patch_saved_view",
        "delete_saved_view",
        "create_project",
        "restore_project",
        "permanently_delete_project",
    ] {
        if !MACRO_API_SOURCE.contains(task_project_api_requirement) {
            return Err(format!(
                "Macro task/project API is missing required behavior: {task_project_api_requirement}"
            ));
        }
    }
    for snippet_file_requirement in [
        "open_new_snippet",
        "render_snippet_typeahead",
        "open_set_document_team_share",
        "pick_and_upload_files",
        "start_file_upload",
        "cancel_file_upload",
        "retry_file_upload",
        "pick_and_upload_zip_folder",
        "render_recently_deleted",
        "video-file-preview",
        "download_document",
        "export_document",
    ] {
        if !MACRO_APP_SOURCE.contains(snippet_file_requirement) {
            return Err(format!(
                "native snippet/file workflow is missing required behavior: {snippet_file_requirement}"
            ));
        }
    }
    for snippet_file_api_requirement in [
        "upload_document_bytes_with_progress",
        "upload_zip_folder_bytes",
        "move_document",
        "delete_document",
        "restore_document",
        "set_document_team_share",
        "download_document_bytes",
    ] {
        if !MACRO_API_SOURCE.contains(snippet_file_api_requirement) {
            return Err(format!(
                "Macro snippet/file API is missing required behavior: {snippet_file_api_requirement}"
            ));
        }
    }
    for canvas_requirement in [
        "select_intersecting_rect",
        "add_pencil_stroke",
        "connect_selected_nodes",
        "align_selected",
        "distribute_selected",
        "group_selected",
        "bring_selected_to_front",
        "send_selected_to_back",
        "copy_selection_json",
        "paste_json",
        "reconcile_remote",
        "to_svg",
    ] {
        if !MACRO_CANVAS_SOURCE.contains(canvas_requirement) {
            return Err(format!(
                "native canvas model is missing required behavior: {canvas_requirement}"
            ));
        }
    }
    for agent_requirement in [
        "open_new_agent",
        "open_rename_agent",
        "open_resume_agent",
        "open_delete_agent",
        "open_set_agent_model",
        "open_set_agent_sandbox_size",
        "retry_agent_prompt",
        "render_agent_transcript_part",
        "render_agent_tool_detail",
        "render_agent_artifact",
        "download_agent_artifact",
        "apply_agent_log_event",
        "agent_entity_context_prompt",
        "email_agent_send_confirmation_required",
    ] {
        if !MACRO_APP_SOURCE.contains(agent_requirement) {
            return Err(format!(
                "native agent workflow is missing required behavior: {agent_requirement}"
            ));
        }
    }
    for agent_api_requirement in [
        "create_agent_session",
        "get_agent_session_log",
        "control_agent_session",
        "rename_agent_session",
        "set_agent_session_sandbox_size",
        "delete_agent_session",
        "fold_agent_session_log",
    ] {
        if !MACRO_API_SOURCE.contains(agent_api_requirement) {
            return Err(format!(
                "Macro agent API is missing required behavior: {agent_api_requirement}"
            ));
        }
    }
    for automation_memory_mcp_requirement in [
        "open_new_scheduled_action",
        "open_edit_scheduled_action",
        "open_delete_scheduled_action",
        "set_scheduled_action_enabled",
        "execute_scheduled_action_now",
        "load_scheduled_action_history",
        "scheduled_action_run_item",
        "handle_scheduled_action_event",
        "load_memory",
        "render_mcp_setup_settings",
        "open_new_mcp_server",
        "toggle_mcp_server",
        "start_mcp_server_auth",
    ] {
        if !MACRO_APP_SOURCE.contains(automation_memory_mcp_requirement) {
            return Err(format!(
                "native automation/memory/MCP workflow is missing required behavior: {automation_memory_mcp_requirement}"
            ));
        }
    }
    for automation_memory_mcp_api_requirement in [
        "list_scheduled_actions",
        "create_scheduled_action",
        "update_scheduled_action",
        "delete_scheduled_action",
        "execute_scheduled_action",
        "list_scheduled_action_history",
        "get_memory",
        "list_mcp_servers",
        "add_mcp_server",
        "update_mcp_server",
        "delete_mcp_server",
        "start_mcp_auth",
    ] {
        if !MACRO_API_SOURCE.contains(automation_memory_mcp_api_requirement) {
            return Err(format!(
                "Macro automation/memory/MCP API is missing required behavior: {automation_memory_mcp_api_requirement}"
            ));
        }
    }
    for crm_requirement in [
        "render_crm_detail",
        "render_crm_discussion",
        "render_crm_emails",
        "crm_email_soup_request",
        "DraggedCrmCompany",
        "drag_over::<DraggedCrmCompany>",
        "crm_optimistic_stages",
        "render_crm_pipeline_column",
        "uniform_list(\n                            \"crm-record-list\"",
        "overflow_x_scroll",
        "open_new_team_crm_saved_view",
        "bulk_set_crm_stage",
        "bulk_mutate_crm_tag",
        "normalize_crm_contact_email",
        "open_set_crm_company_email_sync",
        "can_delete_crm_records",
        "render_crm_settings",
    ] {
        if !MACRO_APP_SOURCE.contains(crm_requirement) {
            return Err(format!(
                "native CRM workflow is missing required behavior: {crm_requirement}"
            ));
        }
    }
    for crm_api_requirement in [
        "create_crm_company",
        "create_crm_contact",
        "set_crm_company_hidden",
        "set_crm_contact_hidden",
        "set_crm_company_email_sync",
        "list_crm_comments",
        "create_crm_comment",
        "edit_crm_comment",
        "delete_crm_comment",
        "get_crm_team_settings",
        "update_crm_team_settings",
        "bulk_update_entities_property_options",
        "list_soup_ast_items",
    ] {
        if !MACRO_API_SOURCE.contains(crm_api_requirement) {
            return Err(format!(
                "Macro CRM API is missing required behavior: {crm_api_requirement}"
            ));
        }
    }
    for call_requirement in [
        "handle_incoming_call_response",
        "connect_native_call",
        "leave_native_call",
        "reconnect_native_call",
        "schedule_native_call_hard_rejoin",
        "schedule_native_audio_device_health_check",
        "schedule_native_camera_health_check",
        "toggle_native_call_microphone",
        "toggle_native_call_camera",
        "toggle_native_call_screen_share",
        "cycle_call_audio_device",
        "cycle_call_camera_device",
        "native-call-participant-grid",
        "active_speakers",
        "participant_quality",
        "call-recording-consent",
        "toggle_native_call_team_share",
        "CallListTab",
        "calls-missed",
        "calls-unattended",
        "render_call_record_detail",
        "set_call_record_team_share",
        "render_entity_capabilities(item.entity_type, &item.id, cx)",
        "open_ask_macro_about_call",
        "CallEvent::ShareWithTeamToggled",
        "CallEvent::Ended",
    ] {
        if !MACRO_APP_SOURCE.contains(call_requirement) {
            return Err(format!(
                "native calls/recordings workflow is missing required behavior: {call_requirement}"
            ));
        }
    }
    for call_api_requirement in [
        "list_active_calls",
        "get_or_create_call",
        "check_active_call",
        "get_ring_status",
        "leave_call",
        "get_call_record",
        "edit_call_record",
        "edit_call_transcript",
        "toggle_call_share_with_team",
        "delete_call_record",
    ] {
        if !MACRO_API_SOURCE.contains(call_api_requirement) {
            return Err(format!(
                "Macro calls/recordings API is missing required behavior: {call_api_requirement}"
            ));
        }
    }
    for recording_requirement in [
        "NativeVideoPlayback",
        "new_call_recording",
        "active_transcript_sequence",
        "call-transcript-search",
        "transcript_match_indices",
        "seek_to_transcript_match",
    ] {
        if !EMBEDDED_VIDEO_SOURCE.contains(recording_requirement) {
            return Err(format!(
                "native recording/transcript view is missing required behavior: {recording_requirement}"
            ));
        }
    }
    for workspace_access_requirement in [
        "render_workspace_editor_fields",
        "render_team_role_fields",
        "team_update_request",
        "normalized_team_link_share",
        "Default link sharing",
        "Choose a workspace role with explicit access semantics",
    ] {
        if !MACRO_APP_SOURCE.contains(workspace_access_requirement) {
            return Err(format!(
                "native workspace access editor is missing required behavior: {workspace_access_requirement}"
            ));
        }
    }
    if !MACRO_API_SOURCE.contains("default_link_share: Option<Option<String>>") {
        return Err("workspace link sharing cannot explicitly clear the backend default".into());
    }
    for settings_administration_requirement in [
        "render_account_team_settings",
        "pick_and_upload_profile_picture",
        "remove_profile_picture",
        "render_mobile_app_settings",
        "render_onboarding_settings",
        "load_starter_documents",
        "open_billing_checkout",
        "open_billing_portal",
        "complete_native_onboarding",
        "toggle_team_setting",
        "write:admin_panel",
        "write:stripe_subscription",
        "bounded content-column layout",
    ] {
        if !MACRO_APP_SOURCE.contains(settings_administration_requirement) {
            return Err(format!(
                "native settings/administration flow is missing required behavior: {settings_administration_requirement}"
            ));
        }
    }
    for settings_administration_api_requirement in [
        "get_profile_pictures",
        "put_profile_picture",
        "get_starter_documents",
        "get_onboarding_state",
        "complete_onboarding",
        "patch_user_tutorial",
        "get_user_quota",
        "create_checkout_session",
        "create_portal_session",
        "create_team",
        "invite_to_team",
        "accept_team_invite",
        "reject_team_invite",
        "capture_account_deletion_feedback",
        "delete_user",
    ] {
        if !MACRO_API_SOURCE.contains(settings_administration_api_requirement) {
            return Err(format!(
                "Macro settings/administration API is missing required behavior: {settings_administration_api_requirement}"
            ));
        }
    }
    for integration_requirement in [
        "render_integrations",
        "render_workspace_imports",
        "render_bot_integrations",
        "render_webhook_integrations",
        "start_github_connect",
        "disconnect_github",
        "start_pipedream_connect",
        "start_mcp_server_auth",
        "open_edit_email_signature",
        "complete_email_link",
        "disconnect_cursor",
        "set_cursor_default_model",
        "MacroDeepLink::InboxLinkCallback",
        "MacroDeepLink::GithubLinkCallback",
    ] {
        if !MACRO_APP_SOURCE.contains(integration_requirement) {
            return Err(format!(
                "native integration flow is missing required behavior: {integration_requirement}"
            ));
        }
    }
    for integration_api_requirement in [
        "list_email_links",
        "init_email_link",
        "health_check_email_links",
        "delete_email_link",
        "resync_email_link",
        "patch_email_settings",
        "github_connection_state",
        "init_github_link",
        "delete_github_link",
        "get_cursor_api_key_status",
        "put_cursor_api_key",
        "delete_cursor_api_key",
        "put_cursor_default_model",
        "get_import_state",
        "run_import",
        "list_mcp_servers",
        "start_mcp_auth",
        "list_pipedream_connections",
        "browse_pipedream_catalog",
    ] {
        if !MACRO_API_SOURCE.contains(integration_api_requirement) {
            return Err(format!(
                "Macro integrations API is missing required behavior: {integration_api_requirement}"
            ));
        }
    }
    for starter_realtime_requirement in [
        "starter_docs_initialized",
        "ReconcileTarget::StarterDocuments",
    ] {
        if !MACRO_REALTIME_SOURCE.contains(starter_realtime_requirement) {
            return Err(format!(
                "starter-document realtime reconciliation is missing required behavior: {starter_realtime_requirement}"
            ));
        }
    }
    for performance_requirement in [
        "on_next_frame_presented",
        "record_input_to_paint",
        "visual_dispatch_pointer_click",
        "--performance-output",
    ] {
        if !MACRO_APP_SOURCE.contains(performance_requirement) {
            return Err(format!(
                "native interaction telemetry is missing required behavior: {performance_requirement}"
            ));
        }
    }
    for capture_requirement in [
        "run-local-e2e.sh",
        "--config",
        "macro.gpui.solid-reference-attestation.v1",
        "png_dimensions",
        "plan_sha256",
        "solid-reference-attestation.json",
        "shutil.rmtree",
        "fixture_profile",
    ] {
        if !SOLID_REFERENCE_CAPTURE_SCRIPT.contains(capture_requirement) {
            return Err(format!(
                "SolidJS reference capture harness is missing required behavior: {capture_requirement}"
            ));
        }
    }
    for comparison_requirement in [
        "macro.gpui.visual-parity-comparison.v1",
        "side-by-side",
        "candidate-manifest.json",
        ".a11y.json",
        "pixel equality is not expected",
        "feature presence",
        "source_revisions",
    ] {
        if !PARITY_COMPARISON_SCRIPT.contains(comparison_requirement) {
            return Err(format!(
                "visual parity comparison harness is missing required behavior: {comparison_requirement}"
            ));
        }
    }
    for playwright_requirement in [
        "../playwright.config",
        "deviceScaleFactor: 2",
        "timezoneId: 'UTC'",
        "workers: 1",
    ] {
        if !SOLID_REFERENCE_PLAYWRIGHT_CONFIG.contains(playwright_requirement) {
            return Err(format!(
                "SolidJS Playwright configuration is missing required behavior: {playwright_requirement}"
            ));
        }
    }
    for playwright_requirement in [
        "[data-split-container]",
        "[data-soup-search] [contenteditable]",
        "replacement_email_domain",
        "forbidden_text",
        "data-theme-light",
        "scale: plan.viewport.screenshot_scale",
        "macro.gpui.solid-reference-capture.v1",
        "text_sha256",
    ] {
        if !SOLID_REFERENCE_PLAYWRIGHT_SPEC.contains(playwright_requirement) {
            return Err(format!(
                "SolidJS Playwright reference test is missing required behavior: {playwright_requirement}"
            ));
        }
    }
    for security_requirement in [
        "email_remote_image_urls",
        "safe_external_resource_url",
        "safe_download_filename",
        "account_credentials_key",
        "agent_control_confirmation",
        "Type the confirmation word",
        "MAX_MACRO_DEEP_LINK_BYTES",
        "deep_link_value_is_safe",
        "APP_PREFERENCES_SCHEMA_VERSION",
        "write_app_preferences_atomically",
        "Settings use newer schema",
        "MAX_GENERAL_UPLOAD_BYTES",
        "MAX_EMAIL_ATTACHMENTS_BYTES",
        "read_bounded_file",
        "upload_document_path_with_progress",
        "total attachments exceed Macro's 18 MB limit",
    ] {
        if !MACRO_APP_SOURCE.contains(security_requirement) {
            return Err(format!(
                "native security boundary is missing required behavior: {security_requirement}"
            ));
        }
    }
    for streaming_upload_requirement in [
        "bounded_file_digest",
        "spawn_blocking",
        "upload source changed after checksum",
        "Body::wrap_stream",
    ] {
        if !MACRO_API_SOURCE.contains(streaming_upload_requirement) {
            return Err(format!(
                "native file streaming is missing required behavior: {streaming_upload_requirement}"
            ));
        }
    }
    if !MACOS_BUNDLE_SCRIPT.contains(&format!(
        "\\\"bundle_identifier\\\": \\\"{BUNDLE_IDENTIFIER}\\\""
    )) {
        return Err("macOS update manifest does not use the stable bundle identifier".into());
    }
    if !MACOS_BUNDLE_SCRIPT.contains("notarytool submit")
        || !MACOS_BUNDLE_SCRIPT.contains("stapler staple")
        || !MACOS_BUNDLE_SCRIPT.contains("codesign --verify")
    {
        return Err("macOS release script is missing signing/notarization verification".into());
    }
    if !MACOS_BUNDLE_SCRIPT.contains("\\\"schema_version\\\": 2")
        || !MACOS_BUNDLE_SCRIPT.contains("team_identifier")
        || !MACOS_BUNDLE_SCRIPT.contains("export MACRO_VERSION")
        || !MACOS_BUNDLE_SCRIPT.contains("MACRO_UPDATE_BASE_URL must use a JSON-safe HTTPS URL")
    {
        return Err("macOS update manifest is missing its signed-release metadata".into());
    }
    for ci_requirement in [
        "cargo fmt --check",
        "npm ci --ignore-scripts",
        "node verify_markdown_two_client.mjs",
        "cargo test",
        "cargo check -p macro_sync --examples",
        "cargo check -p macro_app",
        "macro_performance_soak",
        "macro-gpui/performance-soak.json",
        "measure-gpui-interactions.py",
        "performance-current/",
        "capture-gpui-fixtures.py",
        "visual-current/",
        "bundle-macos.sh debug",
        "codesign --verify --deep --strict",
        "signed-notarized-release-macos",
        "environment: macro-production",
        "MACRO_DEVELOPER_ID_CERTIFICATE_P12",
        "notarytool store-credentials",
        "bundle-macos.sh release",
        "stapler validate",
        "spctl --assess --type execute",
        "actions/upload-artifact@",
    ] {
        if !MACOS_CI_WORKFLOW.contains(ci_requirement) {
            return Err(format!(
                "Macro GPUI CI is missing its required gate: {ci_requirement}"
            ));
        }
    }
    for capture_requirement in [
        "gpui_capture_runner",
        "macro.gpui.visual-candidates.v2",
        "expected_capture_count",
        "png_dimensions",
        "validate_accessibility_tree",
        "unlabeled_actionable_node_count",
        "technical_actionable_node_count",
        "sha256",
        "--validate-only",
        "build_and_stage_runner",
        "built_by_capture_script",
        "shutil.copy2",
        "inline document text input",
        "SetValue",
        "named unread inbox rows",
        "Shift+Enter opens in the split",
        "default Unread inbox filter",
        "shared multi-select checkboxes",
        "native unified search text input",
        "keyboard-navigable search results",
        "search-results focus does not report its active descendant",
        "named Files folder tree",
        "keyboard-navigable Files folder rows",
        "Files folder tree does not report its active descendant",
        "keyboard-navigable Activity notifications",
        "keyboard-navigable Reminders",
        "keyboard-navigable CRM records",
        "CRM does not report its active descendant",
        "keyboard-navigable active call",
        "Calls does not report its active descendant",
        "typed document mention choices",
        "raw backend-ID fields before Custom mention",
        "entity mention search must support Focus and SetValue",
        "did not filter to the matching typed entity",
        "exactly one selected mention result",
        "one named sharing channel list",
        "selected sharing channel",
        "raw sharing transport fields",
        "existing channel remove control",
        "one named property option list",
        "selected property option",
        "raw property option ID input",
        "one named property entity list",
        "raw entity reference fields",
        "property entity search is not editable",
        "one named tag color list",
        "fixed tag palette",
        "selected tag color",
        "raw tag scope or color fields",
        "native tag sharing mode",
        "raw property-definition transport fields",
        "native property-type menu",
        "native owner and selection modes",
        "one named linkable entity list",
        "typed entity-link choice",
        "entity-link picker includes source entity",
        "exactly one selected entity-link result",
        "raw entity-link transport fields",
        "entity-link search is not editable",
        "one named embeddable entity list",
        "typed canvas entity choice",
        "canvas entity picker includes source canvas",
        "exactly one selected canvas entity result",
        "raw canvas entity transport fields",
        "canvas entity search is not editable",
        "raw workspace access transport fields",
        "native default link sharing modes",
        "raw workspace role field",
        "native workspace role choices",
        "three typed composer attachments",
        "one rich-composer action",
        "one remove action per attachment",
        "both directions of centered channel pagination",
    ] {
        if !GPUI_CAPTURE_SCRIPT.contains(capture_requirement) {
            return Err(format!(
                "GPUI capture attestation is missing required behavior: {capture_requirement}"
            ));
        }
    }
    for performance_requirement in [
        "cargo",
        "--release",
        "macro_visual_test_runner",
        "macro.gpui.interaction-performance-attestation.v1",
        "mouse_down_to_scene_submitted",
        "cached_input_to_paint_p95_ms",
        "top_level_request_delta",
        "sha256",
        "One Light",
        "One Dark",
    ] {
        if !GPUI_PERFORMANCE_SCRIPT.contains(performance_requirement) {
            return Err(format!(
                "GPUI interaction performance gate is missing required behavior: {performance_requirement}"
            ));
        }
    }
    for updater_requirement in [
        "verify_archive",
        "verify_developer_id_bundle",
        "validate_release_identity",
        "expected_team_identifier",
        "redirected to an insecure URL",
        "AwaitingHealthCheck",
        "restore_previous_bundle",
        "Gatekeeper assessment",
    ] {
        if !MACRO_UPDATER.contains(updater_requirement) {
            return Err(format!(
                "native updater is missing required recovery behavior: {updater_requirement}"
            ));
        }
    }
    for packaged_update_requirement in [
        "option_env!(\"MACRO_UPDATE_BASE_URL\")",
        "option_env!(\"MACRO_TEAM_IDENTIFIER\")",
        "option_env!(\"MACRO_VERSION\")",
        "parse_https_update_manifest_url",
    ] {
        if !MACRO_APP_SOURCE.contains(packaged_update_requirement) {
            return Err(format!(
                "packaged updater configuration is missing required behavior: {packaged_update_requirement}"
            ));
        }
    }
    Ok(())
}

fn validate_release_gates(
    baseline: &Value,
    fixture_manifest: &Value,
    release_gates: &Value,
) -> Result<(), String> {
    if release_gates["schema"] != "macro.gpui.parity-release-gates.v1" {
        return Err("parity release-gate schema is unsupported".into());
    }
    if release_gates["source_revisions"] != baseline["source_revisions"] {
        return Err("parity release gates do not use the pinned source revisions".into());
    }
    let local_gates = release_gates["local_gates"]
        .as_array()
        .ok_or("parity release gates have no local gate array")?;
    let required_local = [
        "native-test-matrix",
        "browser-native-collaboration",
        "gpui-visual-accessibility-matrix",
        "large-data-performance-soak",
        "release-input-to-present-latency",
        "configured-adhoc-bundle",
    ];
    for id in required_local {
        let gate = local_gates
            .iter()
            .find(|gate| gate["id"] == id)
            .ok_or_else(|| format!("parity release gates are missing local gate {id}"))?;
        if gate["status"] != "passed" || gate["command"].as_str().is_none_or(str::is_empty) {
            return Err(format!(
                "local parity gate {id} is not reproducibly passing"
            ));
        }
    }
    let expected_captures = fixture_manifest["gpui_capture_runner"]["states"]
        .as_object()
        .ok_or("GPUI fixture state matrix is missing")?
        .values()
        .map(|states| states.as_array().map_or(0, Vec::len))
        .sum::<usize>()
        * fixture_manifest["appearances"]
            .as_array()
            .map_or(0, Vec::len);
    let visual_gate = local_gates
        .iter()
        .find(|gate| gate["id"] == "gpui-visual-accessibility-matrix")
        .ok_or("visual accessibility gate is missing")?;
    if visual_gate["expected_captures"] != expected_captures {
        return Err(
            "visual accessibility release-gate count drifted from the fixture matrix".into(),
        );
    }

    let external_gates = release_gates["external_gates"]
        .as_array()
        .ok_or("parity release gates have no external gate array")?;
    let required_external = [
        "solid-reference-captures",
        "macro-local-stack-contract-suite",
        "live-native-web-collaboration",
        "voiceover-and-multilingual-ime",
        "distribution-security-review",
        "developer-id-sign-and-notarize",
        "agpl-gpl-distribution-review",
    ];
    for id in required_external {
        let gate = external_gates
            .iter()
            .find(|gate| gate["id"] == id)
            .ok_or_else(|| format!("parity release gates are missing external gate {id}"))?;
        let status = gate["status"].as_str().unwrap_or_default();
        if !matches!(
            status,
            "environment_required"
                | "manual_review_required"
                | "credentials_required"
                | "legal_review_required"
                | "passed"
        ) {
            return Err(format!(
                "external parity gate {id} has invalid status {status}"
            ));
        }
        if gate["required_inputs"].as_array().is_none_or(Vec::is_empty) {
            return Err(format!("external parity gate {id} has no required inputs"));
        }
    }
    Ok(())
}

fn validate_pointer_accessibility(source: &str) -> Result<(), String> {
    const POINTER: &str = ".cursor_pointer()";
    const REQUIRED: [&str; 3] = [".focusable()", ".tab_stop(true)", ".focus_visible("];
    let mut cursor = 0;
    while let Some(relative) = source[cursor..].find(POINTER) {
        let offset = cursor + relative;
        // Rustfmt can expand a deeply nested GPUI chain by hundreds of bytes
        // before its focus styling. Stop at the control's own click handler
        // instead of using a brittle byte window that changes with indentation.
        let control = &source[offset..];
        let control_end = control
            .find(".on_click(")
            .unwrap_or(2_048)
            .min(2_048)
            .min(control.len());
        let end = offset + control_end;
        let chain = &source[offset..end];
        if !chain.contains(".role(Role::Button)")
            && !chain.contains(".role(Role::CheckBox)")
            && !chain.contains(".role(Role::TreeItem)")
        {
            let line = source[..offset]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1;
            return Err(format!(
                "pointer action at macro_app.rs:{line} is missing an actionable accessibility role"
            ));
        }
        if !chain.contains(".aria_label_from_id()") && !chain.contains(".aria_label(") {
            let line = source[..offset]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1;
            return Err(format!(
                "pointer action at macro_app.rs:{line} is missing an accessibility label"
            ));
        }
        if let Some(missing) = REQUIRED
            .into_iter()
            .find(|requirement| !chain.contains(requirement))
        {
            let line = source[..offset]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1;
            return Err(format!(
                "pointer action at macro_app.rs:{line} is missing accessibility behavior {missing}"
            ));
        }
        cursor = offset + POINTER.len();
    }
    Ok(())
}

fn validate_fixture_manifest(manifest: &Value) -> Result<(), String> {
    if manifest["schema_version"] != 1 {
        return Err("parity fixture manifest schema is unsupported".into());
    }
    let clients = manifest["client_targets"]
        .as_array()
        .ok_or("parity fixture manifest has no client targets")?;
    for required in ["solid_reference", "gpui_candidate"] {
        if !clients.iter().any(|client| client == required) {
            return Err(format!("parity fixture manifest is missing {required}"));
        }
    }
    let appearances = manifest["appearances"]
        .as_array()
        .ok_or("parity fixture manifest has no appearances")?;
    for required in ["light", "dark"] {
        if !appearances.iter().any(|appearance| appearance == required) {
            return Err(format!(
                "parity fixture manifest is missing {required} mode"
            ));
        }
    }
    let accessibility_checks = manifest["capture"]["accessibility_checks"]
        .as_array()
        .ok_or("parity fixture manifest has no accessibility checks")?;
    for required in [
        "non_empty_tree",
        "keyboard_tab_stops",
        "named_actions",
        "human_action_names",
        "focusable_actions",
        "inline_document_text_input",
        "bidirectional_document_text",
    ] {
        if !accessibility_checks.iter().any(|check| check == required) {
            return Err(format!(
                "parity fixture manifest is missing accessibility check {required}"
            ));
        }
    }
    if manifest["redaction"]["allow_production_accounts"] != false {
        return Err("parity captures must never use production accounts".into());
    }
    let forbidden_fields = manifest["redaction"]["forbidden_fields"]
        .as_array()
        .ok_or("parity fixture manifest has no forbidden fields")?;
    for secret in ["access_token", "refresh_token", "authorization", "cookie"] {
        if !forbidden_fields.iter().any(|field| field == secret) {
            return Err(format!("parity redaction rules do not forbid {secret}"));
        }
    }

    let surfaces = manifest["surfaces"]
        .as_array()
        .ok_or("parity fixture manifest has no surfaces")?;
    let mut ids = HashSet::new();
    for surface in surfaces {
        let id = surface["id"].as_str().ok_or("fixture surface has no id")?;
        if !ids.insert(id) {
            return Err(format!("duplicate fixture surface id: {id}"));
        }
        let states = surface["states"]
            .as_array()
            .ok_or_else(|| format!("fixture surface {id} has no state matrix"))?;
        if states.len() < 4 {
            return Err(format!("fixture surface {id} does not cover enough states"));
        }
        if !states.iter().any(|state| {
            state.as_str().is_some_and(|state| {
                state.ends_with("error") || matches!(state, "forbidden" | "authorization_required")
            })
        }) {
            return Err(format!("fixture surface {id} has no failure state"));
        }
        if surface["checks"]
            .as_array()
            .is_none_or(|checks| checks.len() < 3)
        {
            return Err(format!("fixture surface {id} has no interaction checks"));
        }
    }
    for required in [
        "auth",
        "home",
        "inbox",
        "search",
        "mail",
        "channels",
        "documents",
        "tasks",
        "canvas",
        "agents",
        "calls",
        "crm",
        "files",
        "calendar",
        "activity",
        "reminders",
        "settings",
        "integrations",
    ] {
        if !ids.contains(required) {
            return Err(format!("parity fixture manifest is missing {required}"));
        }
    }
    let calls = surfaces
        .iter()
        .find(|surface| surface["id"] == "calls")
        .ok_or("parity fixture manifest is missing calls")?;
    let call_checks = calls["checks"]
        .as_array()
        .ok_or("calls fixture surface has no checks")?;
    for required in [
        "join_leave",
        "incoming_accept_decline",
        "hard_auto_rejoin",
        "audio_device_hotplug",
        "camera_hotplug",
        "screen_share",
        "participant_names",
        "recording_consent",
        "personal_team_memory_opt_out",
        "past_call_list",
        "all_missed_unattended",
        "recording_playback",
        "recording_seek",
        "synchronized_transcript",
        "transcript_search",
        "speaker_identification",
        "tags_properties",
        "archived_record_sharing",
        "realtime_share_reconciliation",
        "completed_call_reconciliation",
        "ask_macro",
    ] {
        if !call_checks.iter().any(|check| check == required) {
            return Err(format!(
                "parity fixture manifest is missing Calls check {required}"
            ));
        }
    }
    let captured_surfaces = manifest["gpui_capture_runner"]["surfaces"]
        .as_array()
        .ok_or("GPUI capture runner has no surfaces")?;
    let captured_states = manifest["gpui_capture_runner"]["states"]
        .as_object()
        .ok_or("GPUI capture runner has no state matrix")?;
    let mut captured_ids = HashSet::new();
    for captured_surface in captured_surfaces {
        let captured_surface = captured_surface
            .as_str()
            .ok_or("GPUI capture runner surface is not a string")?;
        if !captured_ids.insert(captured_surface) {
            return Err(format!(
                "duplicate GPUI capture runner surface: {captured_surface}"
            ));
        }
    }
    if captured_surfaces.len() != ids.len() || captured_states.len() != ids.len() {
        return Err("GPUI capture runner does not cover every declared surface".into());
    }
    for captured_surface in captured_surfaces {
        let captured_surface = captured_surface
            .as_str()
            .ok_or("GPUI capture runner surface is not a string")?;
        if !ids.contains(captured_surface) {
            return Err(format!(
                "GPUI capture runner references unknown surface {captured_surface}"
            ));
        }
        let runner_states = captured_states
            .get(captured_surface)
            .and_then(Value::as_array)
            .ok_or_else(|| format!("GPUI capture runner has no states for {captured_surface}"))?;
        if runner_states.is_empty() {
            return Err(format!(
                "GPUI capture runner has no states for {captured_surface}"
            ));
        }
        let declared_states = surfaces
            .iter()
            .find(|surface| surface["id"] == captured_surface)
            .and_then(|surface| surface["states"].as_array())
            .ok_or_else(|| format!("fixture surface {captured_surface} has no state matrix"))?;
        if declared_states != runner_states {
            return Err(format!(
                "fixture surface {captured_surface} states differ from the GPUI runner matrix"
            ));
        }
    }
    for captured_surface in captured_states.keys() {
        if !captured_ids.contains(captured_surface.as_str()) {
            return Err(format!(
                "GPUI capture state matrix contains undeclared surface {captured_surface}"
            ));
        }
    }
    let settings_checks = surfaces
        .iter()
        .find(|surface| surface["id"] == "settings")
        .and_then(|surface| surface["checks"].as_array())
        .ok_or("settings fixture has no checks")?;
    for required in [
        "local_crash_reporting_opt_out",
        "reduced_motion",
        "higher_contrast_zed_tokens",
        "centered_zed_settings_layout",
        "profile_picture_upload_remove",
        "starter_content_bootstrap",
        "mobile_app_handoff",
        "billing_entitlements_and_quota",
        "team_admin_permission_boundaries",
        "admin_debug_permission_gate",
    ] {
        if !settings_checks.iter().any(|check| check == required) {
            return Err(format!("settings fixture is missing {required}"));
        }
    }
    let document_checks = surfaces
        .iter()
        .find(|surface| surface["id"] == "documents")
        .and_then(|surface| surface["checks"].as_array())
        .ok_or("documents fixture has no checks")?;
    if !document_checks
        .iter()
        .any(|check| check == "bidirectional_text_layout")
    {
        return Err("documents fixture is missing bidirectional_text_layout".into());
    }
    let integrations_checks = surfaces
        .iter()
        .find(|surface| surface["id"] == "integrations")
        .and_then(|surface| surface["checks"].as_array())
        .ok_or("integrations fixture has no checks")?;
    for required in [
        "oauth_callback_lifecycle",
        "scope_selection",
        "reconnect_revoke_diagnostics",
        "notion_linear_slack_import_review",
        "cursor_connection_and_model_default",
    ] {
        if !integrations_checks.iter().any(|check| check == required) {
            return Err(format!("integrations fixture is missing {required}"));
        }
    }
    Ok(())
}

fn validate_solid_reference_plan(
    baseline: &Value,
    manifest: &Value,
    plan: &Value,
) -> Result<(), String> {
    if plan["schema"] != "macro.gpui.solid-reference-plan.v1" {
        return Err("SolidJS reference capture plan schema is unsupported".into());
    }
    if plan["macro_revision"] != baseline["source_revisions"]["macro"] {
        return Err("SolidJS reference capture plan does not use the pinned Macro revision".into());
    }
    if plan["fixture_profile"] != "macro-local-e2e-smoke" {
        return Err("SolidJS reference capture plan does not use Macro local E2E fixtures".into());
    }
    if plan["viewport"]["width"] != 1_440
        || plan["viewport"]["height"] != 960
        || plan["viewport"]["device_scale_factor"] != 2
        || plan["viewport"]["screenshot_scale"] != "device"
        || plan["locale"] != "en-US"
        || plan["timezone"] != "UTC"
    {
        return Err("SolidJS reference capture plan has a non-deterministic environment".into());
    }
    if plan["themes"] != serde_json::json!(["light", "dark"]) {
        return Err("SolidJS reference capture plan must cover light and dark themes".into());
    }
    if plan["redaction"]["replacement_email_domain"] != "example.test" {
        return Err("SolidJS reference capture plan does not redact email domains".into());
    }

    let candidate_states = manifest["gpui_capture_runner"]["states"]
        .as_object()
        .ok_or("GPUI capture runner has no state matrix")?;
    let captures = plan["captures"]
        .as_array()
        .ok_or("SolidJS reference capture plan has no captures")?;
    let mut planned = HashSet::new();
    for capture in captures {
        let surface = capture["surface"]
            .as_str()
            .ok_or("SolidJS reference capture has no surface")?;
        let state = capture["state"]
            .as_str()
            .ok_or("SolidJS reference capture has no state")?;
        if !planned.insert((surface, state)) {
            return Err(format!(
                "duplicate SolidJS reference capture {surface}/{state}"
            ));
        }
        if candidate_states
            .get(surface)
            .and_then(Value::as_array)
            .is_none_or(|states| !states.iter().any(|candidate| candidate == state))
        {
            return Err(format!(
                "SolidJS reference capture {surface}/{state} has no GPUI candidate"
            ));
        }
        if capture["route"]
            .as_str()
            .is_none_or(|route| !route.starts_with('/'))
            || capture["wait_selector"].as_str().is_none_or(str::is_empty)
        {
            return Err(format!(
                "SolidJS reference capture {surface}/{state} has no production route or selector"
            ));
        }
    }

    let runner = &manifest["solid_reference_runner"];
    let expected_count = captures.len() * plan["themes"].as_array().map_or(0, Vec::len);
    if runner["source_revision"] != plan["macro_revision"]
        || runner["fixture_profile"] != plan["fixture_profile"]
        || runner["plan"] != "SOLID_REFERENCE_CAPTURE_PLAN.json"
        || runner["capture_script"] != "scripts/capture-solid-reference.py"
        || runner["comparison_script"] != "scripts/compare-parity-captures.py"
        || runner["capture_count"].as_u64() != Some(expected_count as u64)
        || runner["themes"] != serde_json::json!(["Macro Light", "Macro Dark"])
        || runner["status"] != "harness_ready_runtime_pending"
    {
        return Err("SolidJS reference runner manifest has drifted from its capture plan".into());
    }
    let declared_pairs = runner["paired_states"]
        .as_object()
        .ok_or("SolidJS reference runner has no paired state matrix")?;
    let declared = declared_pairs
        .iter()
        .flat_map(|(surface, states)| {
            states
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(move |state| state.as_str().map(|state| (surface.as_str(), state)))
        })
        .collect::<HashSet<_>>();
    if planned != declared {
        return Err("SolidJS reference paired states differ from the capture plan".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn checked_in_parity_artifacts_are_valid() {
        validate().unwrap();
    }

    #[test]
    fn fixture_manifest_rejects_state_matrix_drift() {
        let mut manifest: Value = serde_json::from_str(FIXTURE_MANIFEST).unwrap();
        let inbox = manifest["surfaces"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|surface| surface["id"] == "inbox")
            .unwrap();
        inbox["states"]
            .as_array_mut()
            .unwrap()
            .push(json!("stale_documentation_state"));

        let error = validate_fixture_manifest(&manifest).unwrap_err();
        assert!(error.contains("inbox states differ"), "{error}");
    }

    #[test]
    fn fixture_manifest_rejects_duplicate_capture_surfaces() {
        let mut manifest: Value = serde_json::from_str(FIXTURE_MANIFEST).unwrap();
        let surfaces = manifest["gpui_capture_runner"]["surfaces"]
            .as_array_mut()
            .unwrap();
        surfaces[1] = surfaces[0].clone();

        let error = validate_fixture_manifest(&manifest).unwrap_err();
        assert!(
            error.contains("duplicate GPUI capture runner surface"),
            "{error}"
        );
    }

    #[test]
    fn fixture_manifest_requires_a_real_failure_state() {
        let mut manifest: Value = serde_json::from_str(FIXTURE_MANIFEST).unwrap();
        let home = manifest["surfaces"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|surface| surface["id"] == "home")
            .unwrap();
        home["states"] = json!(["loading", "empty", "populated", "loaded_again"]);

        let error = validate_fixture_manifest(&manifest).unwrap_err();
        assert!(error.contains("home has no failure state"), "{error}");
    }
}
