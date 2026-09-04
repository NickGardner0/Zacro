use anyhow::{Context as _, Result};
use macro_api::{
    ActionExecutionRecord, ActiveCallSummary, AttachmentEntityReference, Bot, BotChannel, BotToken,
    ChannelParticipant, CrmCommentThread, CrmTeamSettingsResponse, CursorApiKeyStatus,
    CursorModelsResponse, DocumentCommentThread, GetContactsResponse, GithubConnectionState,
    GithubPullRequest, GroupedSoupPage, ImportState, ListCalendarsResponse,
    ListEmailLabelsResponse, ListEmailLinksResponse, MacroNotification, McpServerResponse,
    MemoryResponse, NotificationTypePreferences, NotificationUnsubscribe, OnboardingState,
    PipedreamCatalogResponse, PipedreamConnectionResponse, ProfilePictures,
    PropertyDefinitionResponse, Reminder, SandboxSizeBody, SavedViewsResponse, ScheduledAction,
    ScheduledEmailMessage, SharePermissionV2, SoupGroupMeta, StarterDocumentsResponse,
    StorageProject, TagSetResponse, TaskDuplicate, Team, TeamInvitesResponse, TeamWithMembers,
    UnifiedSearchResponse, UnifiedSearchResultKind, UserName, UserQuota, Webhook,
};
use macro_entity::{EntityPermission, EntityProperties, Favorite};
use macro_models::EntityType;
use macro_store::{Identified, NormalizedCollection};
use macro_sync::HistorySession;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum DataState<T> {
    Idle,
    Loading,
    Loaded(T),
    Failed(String),
}

impl<T> DataState<T> {
    pub fn loaded(&self) -> Option<&T> {
        match self {
            Self::Loaded(value) => Some(value),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InboxItem {
    pub id: String,
    pub tag: String,
    pub sub_type: Option<String>,
    pub entity_type: EntityType,
    pub title: String,
    pub subtitle: String,
    pub timestamp: String,
    pub raw: Value,
}

impl Identified for InboxItem {
    fn id(&self) -> &str {
        &self.id
    }
}

#[derive(Clone, Copy)]
pub enum SoupFilter {
    All,
    Exact(EntityType),
    Documents,
    Tasks,
    Crm,
}

impl SoupFilter {
    pub fn matches(self, item: &InboxItem) -> bool {
        match self {
            Self::All => true,
            Self::Exact(entity_type) => item.entity_type == entity_type,
            Self::Documents => {
                item.entity_type == EntityType::Document && item.sub_type.as_deref() != Some("task")
            }
            Self::Tasks => {
                item.entity_type == EntityType::Document && item.sub_type.as_deref() == Some("task")
            }
            Self::Crm => matches!(
                item.entity_type,
                EntityType::CrmCompany | EntityType::CrmContact
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChannelSummary {
    pub id: String,
    pub name: String,
    pub channel_type: String,
    pub preview: String,
    pub updated_at: String,
    pub raw: Value,
}

impl Identified for ChannelSummary {
    fn id(&self) -> &str {
        &self.id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChannelReaction {
    pub emoji: String,
    pub users: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChannelAttachment {
    pub id: String,
    pub entity_id: String,
    pub entity_type: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChannelMessage {
    pub id: String,
    pub channel_id: String,
    pub sender_id: String,
    pub sender: String,
    pub content: String,
    pub created_at: String,
    pub edited_at: Option<String>,
    pub deleted_at: Option<String>,
    pub reactions: Vec<ChannelReaction>,
    pub attachments: Vec<ChannelAttachment>,
    pub thread_reply_count: usize,
    pub raw: Value,
}

impl Identified for ChannelMessage {
    fn id(&self) -> &str {
        &self.id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChannelThreadReply {
    pub id: String,
    pub sender_id: String,
    pub sender: String,
    pub content: String,
    pub created_at: String,
    pub edited_at: Option<String>,
    pub reactions: Vec<ChannelReaction>,
    pub attachments: Vec<ChannelAttachment>,
    pub raw: Value,
}

impl Identified for ChannelThreadReply {
    fn id(&self) -> &str {
        &self.id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReminderItem(pub Reminder);

impl Identified for ReminderItem {
    fn id(&self) -> &str {
        &self.0.id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NotificationItem(pub MacroNotification);

impl Identified for NotificationItem {
    fn id(&self) -> &str {
        &self.0.id
    }
}

#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    pub previous_cursor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GroupedInboxInitialPage {
    pub items: Vec<InboxItem>,
    pub groups: Vec<SoupGroupMeta>,
}

#[derive(Debug, Clone)]
pub struct GroupedInboxGroupPage {
    pub items: Vec<InboxItem>,
    pub group: SoupGroupMeta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedItem {
    pub id: String,
    pub entity_type: EntityType,
    pub title: String,
    pub deleted_at: String,
}

#[derive(Debug, Clone)]
pub struct WorkspaceData {
    pub inbox: NormalizedCollection<InboxItem>,
    /// The server-owned "touched by me" feed used by Macro's Home/Recent
    /// surface. Its ordering is meaningful and must not be re-sorted locally.
    pub recent: NormalizedCollection<InboxItem>,
    /// All personal and team-shared snippet documents available to native
    /// semicolon typeahead. This is deliberately separate from the first page
    /// of the general Soup feed so unopened team snippets remain discoverable.
    pub snippets: NormalizedCollection<InboxItem>,
    /// The currently selected Inbox tab. This is intentionally separate from
    /// `inbox`: the latter is the broad entity soup used by Mail, Documents,
    /// Tasks, Files, and CRM, while this collection is server-filtered using
    /// Macro's Signal/Noise/All AST contract.
    pub inbox_view: NormalizedCollection<InboxItem>,
    pub filtered_list_views: HashMap<String, NormalizedCollection<InboxItem>>,
    pub search_results: NormalizedCollection<InboxItem>,
    pub channels: NormalizedCollection<ChannelSummary>,
    pub channel_messages: HashMap<String, NormalizedCollection<ChannelMessage>>,
    pub channel_thread_replies: HashMap<String, NormalizedCollection<ChannelThreadReply>>,
    pub channel_participants: HashMap<String, DataState<Vec<ChannelParticipant>>>,
    pub channel_bots: HashMap<String, DataState<Vec<Bot>>>,
    pub reminders: NormalizedCollection<ReminderItem>,
    pub notifications: NormalizedCollection<NotificationItem>,
    pub notification_preferences: DataState<NotificationTypePreferences>,
    pub notification_unsubscribes: DataState<Vec<NotificationUnsubscribe>>,
    pub entity_details: HashMap<String, DataState<Value>>,
    pub entity_permissions: HashMap<String, DataState<EntityPermission>>,
    pub entity_share_permissions: HashMap<String, DataState<SharePermissionV2>>,
    pub entity_properties: HashMap<String, DataState<EntityProperties>>,
    pub property_definitions: DataState<Vec<PropertyDefinitionResponse>>,
    pub entity_backlinks: HashMap<String, DataState<Vec<AttachmentEntityReference>>>,
    pub document_comments: HashMap<String, DataState<Vec<DocumentCommentThread>>>,
    pub crm_comments: HashMap<String, DataState<Vec<CrmCommentThread>>>,
    pub crm_emails: HashMap<String, NormalizedCollection<InboxItem>>,
    pub document_history: HashMap<String, DataState<Vec<HistorySession>>>,
    pub task_duplicates: HashMap<String, DataState<Vec<TaskDuplicate>>>,
    pub task_github_pull_requests: HashMap<String, DataState<Vec<GithubPullRequest>>>,
    pub tags: DataState<Vec<TagSetResponse>>,
    pub favorites: DataState<Vec<Favorite>>,
    pub active_calls: DataState<Vec<ActiveCallSummary>>,
    pub agent_sandbox_size: DataState<SandboxSizeBody>,
    pub scheduled_actions: DataState<Vec<ScheduledAction>>,
    pub scheduled_action_history: HashMap<String, DataState<Vec<ActionExecutionRecord>>>,
    pub email_links: DataState<ListEmailLinksResponse>,
    pub email_labels: DataState<ListEmailLabelsResponse>,
    pub scheduled_email_drafts: DataState<Vec<ScheduledEmailMessage>>,
    pub calendars: DataState<ListCalendarsResponse>,
    pub memory: DataState<MemoryResponse>,
    pub onboarding: DataState<OnboardingState>,
    pub starter_documents: DataState<StarterDocumentsResponse>,
    pub import_state: DataState<ImportState>,
    pub mcp_servers: DataState<Vec<McpServerResponse>>,
    pub pipedream_connections: DataState<Vec<PipedreamConnectionResponse>>,
    pub pipedream_catalog: DataState<PipedreamCatalogResponse>,
    pub bots: DataState<Vec<Bot>>,
    pub bot_channels: HashMap<String, DataState<Vec<BotChannel>>>,
    pub bot_tokens: HashMap<String, DataState<Vec<BotToken>>>,
    pub webhooks: DataState<Vec<Webhook>>,
    pub saved_views: DataState<SavedViewsResponse>,
    pub crm_team_settings: DataState<CrmTeamSettingsResponse>,
    pub github_connection: DataState<GithubConnectionState>,
    pub cursor_api_key: DataState<CursorApiKeyStatus>,
    pub cursor_models: DataState<CursorModelsResponse>,
    pub team: DataState<Option<TeamWithMembers>>,
    pub user_teams: DataState<Vec<Team>>,
    pub team_invites: DataState<TeamInvitesResponse>,
    pub user_invites: DataState<TeamInvitesResponse>,
    pub user_name: DataState<UserName>,
    pub profile_pictures: DataState<ProfilePictures>,
    pub contacts: DataState<GetContactsResponse>,
    pub user_names: DataState<HashMap<String, UserName>>,
    pub user_quota: DataState<Option<UserQuota>>,
    pub projects: DataState<Vec<StorageProject>>,
    pub project_contents: HashMap<String, DataState<Vec<Value>>>,
    pub recently_deleted: DataState<Vec<DeletedItem>>,
}

impl Default for WorkspaceData {
    fn default() -> Self {
        Self {
            inbox: NormalizedCollection::default(),
            recent: NormalizedCollection::default(),
            snippets: NormalizedCollection::default(),
            inbox_view: NormalizedCollection::default(),
            filtered_list_views: HashMap::new(),
            search_results: NormalizedCollection::default(),
            channels: NormalizedCollection::default(),
            channel_messages: HashMap::new(),
            channel_thread_replies: HashMap::new(),
            channel_participants: HashMap::new(),
            channel_bots: HashMap::new(),
            reminders: NormalizedCollection::default(),
            notifications: NormalizedCollection::default(),
            notification_preferences: DataState::Idle,
            notification_unsubscribes: DataState::Idle,
            entity_details: HashMap::new(),
            entity_permissions: HashMap::new(),
            entity_share_permissions: HashMap::new(),
            entity_properties: HashMap::new(),
            property_definitions: DataState::Idle,
            entity_backlinks: HashMap::new(),
            document_comments: HashMap::new(),
            crm_comments: HashMap::new(),
            crm_emails: HashMap::new(),
            document_history: HashMap::new(),
            task_duplicates: HashMap::new(),
            task_github_pull_requests: HashMap::new(),
            tags: DataState::Idle,
            favorites: DataState::Idle,
            active_calls: DataState::Idle,
            agent_sandbox_size: DataState::Idle,
            scheduled_actions: DataState::Idle,
            scheduled_action_history: HashMap::new(),
            email_links: DataState::Idle,
            email_labels: DataState::Idle,
            scheduled_email_drafts: DataState::Idle,
            calendars: DataState::Idle,
            memory: DataState::Idle,
            onboarding: DataState::Idle,
            starter_documents: DataState::Idle,
            import_state: DataState::Idle,
            mcp_servers: DataState::Idle,
            pipedream_connections: DataState::Idle,
            pipedream_catalog: DataState::Idle,
            bots: DataState::Idle,
            bot_channels: HashMap::new(),
            bot_tokens: HashMap::new(),
            webhooks: DataState::Idle,
            saved_views: DataState::Idle,
            crm_team_settings: DataState::Idle,
            github_connection: DataState::Idle,
            cursor_api_key: DataState::Idle,
            cursor_models: DataState::Idle,
            team: DataState::Idle,
            user_teams: DataState::Idle,
            team_invites: DataState::Idle,
            user_invites: DataState::Idle,
            user_name: DataState::Idle,
            profile_pictures: DataState::Idle,
            contacts: DataState::Idle,
            user_names: DataState::Idle,
            user_quota: DataState::Idle,
            projects: DataState::Idle,
            project_contents: HashMap::new(),
            recently_deleted: DataState::Idle,
        }
    }
}

fn string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(ToOwned::to_owned)
}

fn nested_string(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_str().map(ToOwned::to_owned)
}

fn entity_type(tag: &str) -> EntityType {
    match tag {
        "document" => EntityType::Document,
        "chat" => EntityType::Chat,
        "project" => EntityType::Project,
        "emailThread" => EntityType::EmailThread,
        "channel" => EntityType::Channel,
        "channelThread" => EntityType::ChannelMessage,
        "call" => EntityType::CallRecord,
        "calendarEvent" => EntityType::CalendarEvent,
        "crmCompany" => EntityType::CrmCompany,
        "crmContact" => EntityType::CrmContact,
        "reminder" => EntityType::Reminder,
        "agentSession" | "agent_session" => EntityType::AgentSession,
        "staticFile" | "static_file" => EntityType::StaticFile,
        _ => EntityType::ForeignEntity,
    }
}

pub fn parse_recently_deleted(value: &Value) -> Result<Vec<DeletedItem>> {
    let response = value.get("data").unwrap_or(value);
    let items = response
        .get("items")
        .and_then(Value::as_array)
        .context("recently deleted response did not contain an items array")?;

    items
        .iter()
        .map(|item| {
            let item = item.get("item").unwrap_or(item);
            let tag = string(item, &["type", "tag"])
                .unwrap_or_default()
                .to_ascii_lowercase();
            let entity_type = match tag.as_str() {
                "document" => EntityType::Document,
                "project" => EntityType::Project,
                "chat" => EntityType::Chat,
                _ => EntityType::ForeignEntity,
            };
            let id = string(item, &["id", "documentId", "chatId"])
                .or_else(|| nested_string(item, &["documentMetadata", "documentId"]))
                .with_context(|| format!("recently deleted {tag} item did not contain an id"))?;
            let title = string(item, &["name", "documentName", "title"])
                .or_else(|| nested_string(item, &["documentMetadata", "documentName"]))
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| format!("Untitled {tag}"));
            let deleted_at = string(item, &["deletedAt", "deleted_at"])
                .or_else(|| nested_string(item, &["documentMetadata", "deletedAt"]))
                .or_else(|| string(item, &["updatedAt", "updated_at"]))
                .unwrap_or_default();
            Ok(DeletedItem {
                id,
                entity_type,
                title,
                deleted_at,
            })
        })
        .collect()
}

pub fn parse_soup_page(value: Value) -> Result<Page<InboxItem>> {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .context("soup response did not contain an items array")?;
    let mut parsed = Vec::with_capacity(items.len());

    for item in items {
        let tag = item.get("tag").and_then(Value::as_str).unwrap_or("unknown");
        let data = item.get("data").unwrap_or(&Value::Null);
        let nested_channel = data.get("channel").unwrap_or(data);
        let id = string(data, &["id", "messageId", "callId", "foreignEntityId"])
            .or_else(|| string(nested_channel, &["id"]))
            .or_else(|| string(data, &["channel_id"]))
            .unwrap_or_else(|| format!("{tag}-{}", parsed.len()));

        let title = string(data, &["name", "title", "customName", "channelName"])
            .or_else(|| string(nested_channel, &["name"]))
            .or_else(|| string(data, &["senderName", "senderEmail"]))
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| match tag {
                "emailThread" => "Untitled email".into(),
                "channelThread" => "Channel thread".into(),
                "call" => "Call".into(),
                _ => format!("Untitled {tag}"),
            });

        let subtitle = string(data, &["snippet", "description", "fileType", "status"])
            .or_else(|| nested_string(data, &["latest_message", "content"]))
            .or_else(|| string(data, &["content"]))
            .unwrap_or_else(|| tag.to_owned());
        let timestamp = string(
            data,
            &[
                "sortTs",
                "viewedAt",
                "viewed_at",
                "updatedAt",
                "updated_at",
                "createdAt",
                "created_at",
            ],
        )
        .or_else(|| string(nested_channel, &["updated_at", "created_at"]))
        .unwrap_or_default();
        let sub_type = nested_string(data, &["subType", "type"])
            .or_else(|| string(data, &["subType", "sub_type"]))
            .map(|value| value.to_lowercase());

        parsed.push(InboxItem {
            id,
            tag: tag.to_owned(),
            sub_type,
            entity_type: entity_type(tag),
            title,
            subtitle,
            timestamp,
            raw: item.clone(),
        });
    }

    Ok(Page {
        items: parsed,
        next_cursor: value
            .get("next_cursor")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        previous_cursor: None,
    })
}

pub fn parse_grouped_soup_initial_page(page: GroupedSoupPage) -> Result<GroupedInboxInitialPage> {
    let GroupedSoupPage::Initial { items, groups } = page else {
        anyhow::bail!("grouped Soup initial request returned a group page");
    };
    let items = parse_soup_page(serde_json::json!({
        "items": items.into_values().collect::<Vec<_>>()
    }))?
    .items;
    validate_group_membership(&items, &groups)?;
    Ok(GroupedInboxInitialPage { items, groups })
}

pub fn parse_grouped_soup_group_page(page: GroupedSoupPage) -> Result<GroupedInboxGroupPage> {
    let GroupedSoupPage::GroupPage { items, group } = page else {
        anyhow::bail!("grouped Soup pagination request returned an initial page");
    };
    let items = parse_soup_page(serde_json::json!({
        "items": items.into_values().collect::<Vec<_>>()
    }))?
    .items;
    validate_group_membership(&items, std::slice::from_ref(&group))?;
    Ok(GroupedInboxGroupPage { items, group })
}

fn validate_group_membership(items: &[InboxItem], groups: &[SoupGroupMeta]) -> Result<()> {
    let item_ids = items
        .iter()
        .map(|item| item.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    for group in groups {
        if let Some(missing) = group
            .item_ids
            .iter()
            .find(|item_id| !item_ids.contains(item_id.as_str()))
        {
            anyhow::bail!(
                "group {:?} referenced missing Soup item {:?}",
                group.key,
                missing
            );
        }
    }
    Ok(())
}

pub fn parse_search_page(value: UnifiedSearchResponse) -> Result<Page<InboxItem>> {
    let mut items = Vec::with_capacity(value.results.len());
    for (index, result) in value.results.into_iter().enumerate() {
        let tag = match result.kind() {
            UnifiedSearchResultKind::Document => "document",
            UnifiedSearchResultKind::Chat => "chat",
            UnifiedSearchResultKind::Email => "emailThread",
            UnifiedSearchResultKind::ChannelMessage => "channelThread",
            UnifiedSearchResultKind::Channel => "channel",
            UnifiedSearchResultKind::Project => "project",
            UnifiedSearchResultKind::Call => "call",
            UnifiedSearchResultKind::Company => "crmCompany",
            UnifiedSearchResultKind::CalendarEvent => "calendarEvent",
            UnifiedSearchResultKind::Unknown(_) => "foreign",
        };
        let result_type = result.kind().as_wire().to_owned();
        let result = result.into_raw();
        let id = string(
            &result,
            &[
                "id",
                "document_id",
                "thread_id",
                "channel_id",
                "message_id",
                "call_id",
                "company_id",
                "event_id",
            ],
        )
        .unwrap_or_else(|| format!("search-{index}"));
        let title = string(
            &result,
            &[
                "name",
                "document_name",
                "subject",
                "channel_name",
                "company_name",
                "title",
            ],
        )
        .unwrap_or_else(|| format!("{result_type} result"));
        let subtitle = first_nested_search_text(&result)
            .or_else(|| string(&result, &["description", "domain", "snippet"]))
            .unwrap_or_else(|| "Matched by Macro search".into());
        let timestamp = string(
            &result,
            &["updated_at", "created_at", "viewed_at", "timestamp"],
        )
        .unwrap_or_default();
        let sub_type = string(&result, &["sub_type", "subType"]);
        let raw = serde_json::json!({ "tag": tag, "data": result });
        items.push(InboxItem {
            id,
            tag: tag.into(),
            sub_type,
            entity_type: entity_type(tag),
            title,
            subtitle,
            timestamp,
            raw,
        });
    }
    Ok(Page {
        items,
        next_cursor: value.next_cursor,
        previous_cursor: None,
    })
}

fn first_nested_search_text(value: &Value) -> Option<String> {
    for key in [
        "document_search_results",
        "email_message_search_results",
        "chat_search_results",
        "message_search_results",
        "search_results",
    ] {
        let Some(first) = value
            .get(key)
            .and_then(Value::as_array)
            .and_then(|items| items.first())
        else {
            continue;
        };
        if let Some(text) = string(first, &["text", "content", "snippet", "body"]) {
            return Some(text);
        }
        if let Some(highlight) = first
            .get("highlight")
            .or_else(|| first.get("highlights"))
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(Value::as_str)
        {
            return Some(highlight.to_owned());
        }
    }
    None
}

pub fn parse_channel_page(value: Value) -> Result<Page<ChannelSummary>> {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .context("channel response did not contain an items array")?;
    let parsed = items
        .iter()
        .map(|item| {
            let id = string(item, &["id"]).context("channel item missing id")?;
            let channel_type =
                string(item, &["channel_type"]).unwrap_or_else(|| "channel".to_owned());
            let name = string(item, &["name"])
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| {
                    if channel_type == "direct_message" {
                        "Direct message".to_owned()
                    } else {
                        "Unnamed channel".to_owned()
                    }
                });
            let preview = nested_string(item, &["latest_message", "content"])
                .or_else(|| nested_string(item, &["latest_non_thread_message", "content"]))
                .unwrap_or_else(|| "No messages yet".to_owned());
            // Macro's channel list timestamp represents the latest visible
            // message, not merely the channel row's creation/update time.
            // Prefer the all-message boundary (which includes thread replies)
            // and retain the channel timestamps as the empty-channel fallback.
            let updated_at = nested_string(item, &["latest_message", "created_at"])
                .or_else(|| nested_string(item, &["latest_non_thread_message", "created_at"]))
                .or_else(|| string(item, &["updated_at", "interacted_at", "created_at"]))
                .unwrap_or_default();
            Ok(ChannelSummary {
                id,
                name,
                channel_type,
                preview,
                updated_at,
                raw: item.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(Page {
        items: parsed,
        next_cursor: value
            .get("next_cursor")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        previous_cursor: None,
    })
}

pub fn parse_message_page(value: Value) -> Result<Page<ChannelMessage>> {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .context("message response did not contain an items array")?;
    let parsed = items
        .iter()
        .map(parse_channel_message)
        .collect::<Result<Vec<_>>>()?;

    Ok(Page {
        items: parsed,
        next_cursor: value
            .get("next_cursor")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        previous_cursor: value
            .get("previous_cursor")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
    })
}

pub fn parse_channel_message(item: &Value) -> Result<ChannelMessage> {
    let channel_id = string(item, &["channel_id"]).context("channel message missing channel_id")?;
    let (sender_id, sender) = parse_message_sender(item);
    Ok(ChannelMessage {
        id: string(item, &["id"]).context("channel message missing id")?,
        channel_id,
        sender_id,
        sender,
        content: string(item, &["content"]).unwrap_or_default(),
        created_at: string(item, &["created_at"]).unwrap_or_default(),
        edited_at: string(item, &["edited_at"]),
        deleted_at: string(item, &["deleted_at"]),
        reactions: parse_reactions(item),
        attachments: parse_attachments(item),
        thread_reply_count: item
            .pointer("/thread/reply_count")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
        raw: item.clone(),
    })
}

pub fn parse_thread_replies(value: Value) -> Result<Vec<ChannelThreadReply>> {
    let items = value
        .as_array()
        .or_else(|| value.get("items").and_then(Value::as_array))
        .context("thread replies response was not an array")?;
    items.iter().map(parse_channel_thread_reply).collect()
}

pub fn parse_channel_thread_reply(item: &Value) -> Result<ChannelThreadReply> {
    let (sender_id, sender) = parse_message_sender(item);
    Ok(ChannelThreadReply {
        id: string(item, &["id"]).context("thread reply missing id")?,
        sender_id,
        sender,
        content: string(item, &["content"]).unwrap_or_default(),
        created_at: string(item, &["created_at"]).unwrap_or_default(),
        edited_at: string(item, &["edited_at"]),
        reactions: parse_reactions(item),
        attachments: parse_attachments(item),
        raw: item.clone(),
    })
}

fn parse_message_sender(item: &Value) -> (String, String) {
    let sender_id = string(item, &["sender_id"])
        .or_else(|| nested_string(item, &["sender", "id"]))
        .unwrap_or_default();
    let sender = nested_string(item, &["sender", "name"])
        .or_else(|| nested_string(item, &["sender", "id"]))
        .or_else(|| string(item, &["sender_id"]))
        .unwrap_or_else(|| "Unknown sender".to_owned());
    (sender_id, sender)
}

fn parse_reactions(item: &Value) -> Vec<ChannelReaction> {
    item.get("reactions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|reaction| {
            Some(ChannelReaction {
                emoji: reaction.get("emoji")?.as_str()?.to_owned(),
                users: reaction
                    .get("users")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect(),
            })
        })
        .collect()
}

pub fn parse_channel_reactions(item: &Value) -> Vec<ChannelReaction> {
    parse_reactions(item)
}

fn parse_attachments(item: &Value) -> Vec<ChannelAttachment> {
    item.get("attachments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|attachment| {
            Some(ChannelAttachment {
                id: attachment.get("id")?.as_str()?.to_owned(),
                entity_id: attachment.get("entity_id")?.as_str()?.to_owned(),
                entity_type: attachment.get("entity_type")?.as_str()?.to_owned(),
                name: string(attachment, &["name", "file_name", "filename"]).unwrap_or_else(|| {
                    attachment["entity_id"]
                        .as_str()
                        .unwrap_or("Attachment")
                        .to_owned()
                }),
            })
        })
        .collect()
}

pub fn parse_channel_attachments(item: &Value) -> Vec<ChannelAttachment> {
    parse_attachments(item)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_mixed_soup_items_without_guessing_one_schema() {
        let page = parse_soup_page(json!({
            "items": [
                {"tag": "document", "data": {"id": "doc-1", "name": "Plan", "fileType": "md", "subType": {"type": "task"}, "updatedAt": "2026-01-01"}, "frecency_score": 1.0, "is_favorited": false},
                {"tag": "emailThread", "data": {"id": "mail-1", "name": "Hello", "snippet": "World", "sortTs": "2026-01-02"}, "frecency_score": 1.0, "is_favorited": false}
            ],
            "next_cursor": "cursor-1"
        }))
        .unwrap();

        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items[0].entity_type, EntityType::Document);
        assert_eq!(page.items[0].tag, "document");
        assert_eq!(page.items[0].sub_type.as_deref(), Some("task"));
        assert_eq!(page.items[1].subtitle, "World");
        assert_eq!(page.next_cursor.as_deref(), Some("cursor-1"));
    }

    #[test]
    fn parses_recently_deleted_documents_projects_and_chats() {
        let items = parse_recently_deleted(&json!({
            "data": {
                "items": [
                    {
                        "type": "document",
                        "documentId": "doc-1",
                        "documentName": "Roadmap",
                        "deletedAt": "2026-08-27T12:00:00Z"
                    },
                    {
                        "type": "project",
                        "id": "folder-1",
                        "name": "Archive",
                        "deletedAt": "2026-08-26T12:00:00Z"
                    },
                    {
                        "type": "chat",
                        "id": "chat-1",
                        "name": "Research",
                        "deletedAt": "2026-08-25T12:00:00Z"
                    }
                ]
            },
            "error": false
        }))
        .unwrap();

        assert_eq!(items.len(), 3);
        assert_eq!(items[0].entity_type, EntityType::Document);
        assert_eq!(items[0].title, "Roadmap");
        assert_eq!(items[1].entity_type, EntityType::Project);
        assert_eq!(items[2].entity_type, EntityType::Chat);
    }

    #[test]
    fn parses_channel_and_message_pages() {
        let channels = parse_channel_page(json!({
            "items": [{
                "id": "channel-1",
                "channel_type": "public",
                "name": "general",
                "latest_message": {
                    "content": "Welcome",
                    "created_at": "2026-01-02"
                },
                "updated_at": "2026-01-01"
            }]
        }))
        .unwrap();
        assert_eq!(channels.items[0].preview, "Welcome");
        assert_eq!(channels.items[0].updated_at, "2026-01-02");

        let messages = parse_message_page(json!({
            "next_cursor": "older-page",
            "previous_cursor": "newer-page",
            "items": [{
                "id": "message-1",
                "channel_id": "channel-1",
                "content": "Hello",
                "sender_id": "user-1",
                "sender": {"id": "user-1", "name": "Nick", "type": "user"},
                "created_at": "2026-01-01",
                "edited_at": "2026-01-02",
                "reactions": [{"emoji": "👍", "users": ["user-2"]}],
                "attachments": [{"id": "a-1", "entity_id": "doc-1", "entity_type": "document"}],
                "thread": {"reply_count": 2, "preview": []}
            }]
        }))
        .unwrap();
        assert_eq!(messages.items[0].sender, "Nick");
        assert_eq!(messages.items[0].sender_id, "user-1");
        assert_eq!(messages.items[0].reactions[0].emoji, "👍");
        assert_eq!(messages.items[0].attachments[0].entity_id, "doc-1");
        assert_eq!(messages.items[0].thread_reply_count, 2);
        assert_eq!(messages.next_cursor.as_deref(), Some("older-page"));
        assert_eq!(messages.previous_cursor.as_deref(), Some("newer-page"));

        let replies = parse_thread_replies(json!([{
            "id": "reply-1",
            "content": "Thread reply",
            "sender_id": "user-2",
            "sender": {"id": "user-2", "name": "Ada", "type": "user"},
            "created_at": "2026-01-03",
            "reactions": [],
            "attachments": []
        }]))
        .unwrap();
        assert_eq!(replies[0].sender, "Ada");
        assert_eq!(replies[0].content, "Thread reply");
    }

    #[test]
    fn parses_unified_search_discriminated_results() {
        let response = serde_json::from_value(json!({
            "results": [
                {
                    "type": "document",
                    "id": "doc-1",
                    "document_name": "Native roadmap",
                    "document_search_results": [{"text": "GPUI editor"}],
                    "updated_at": "2026-08-27T12:00:00Z"
                },
                {
                    "type": "email",
                    "id": "mail-1",
                    "thread_id": "mail-1",
                    "subject": "Launch",
                    "email_message_search_results": [{"content": "Ship it"}]
                }
            ],
            "next_cursor": "search-next"
        }))
        .unwrap();
        let page = parse_search_page(response).unwrap();

        assert_eq!(page.items[0].entity_type, EntityType::Document);
        assert_eq!(page.items[0].subtitle, "GPUI editor");
        assert_eq!(page.items[1].entity_type, EntityType::EmailThread);
        assert_eq!(page.next_cursor.as_deref(), Some("search-next"));
    }

    #[test]
    fn filters_tasks_and_documents_from_real_soup_subtypes() {
        let task = InboxItem {
            id: "task-1".into(),
            tag: "document".into(),
            sub_type: Some("task".into()),
            entity_type: EntityType::Document,
            title: "Task".into(),
            subtitle: String::new(),
            timestamp: String::new(),
            raw: json!({}),
        };
        let mut document = task.clone();
        document.id = "document-1".into();
        document.sub_type = None;

        assert!(SoupFilter::Tasks.matches(&task));
        assert!(!SoupFilter::Tasks.matches(&document));
        assert!(SoupFilter::Documents.matches(&document));
        assert!(!SoupFilter::Documents.matches(&task));
    }

    #[test]
    fn recognizes_agent_and_static_file_soup_tags() {
        let page = parse_soup_page(json!({
            "items": [
                {"tag": "agentSession", "data": {"id": "agent-1", "name": "Research"}},
                {"tag": "staticFile", "data": {"id": "file-1", "name": "image.png"}}
            ]
        }))
        .unwrap();

        assert_eq!(page.items[0].entity_type, EntityType::AgentSession);
        assert_eq!(page.items[1].entity_type, EntityType::StaticFile);
    }

    #[test]
    fn channel_thread_store_keys_cannot_collide_across_channels() {
        assert_ne!(
            crate::channel_thread_key("channel-a", "message-1"),
            crate::channel_thread_key("channel-b", "message-1")
        );
        assert_eq!(
            crate::comma_separated_values(" user-1, user-2 ,,"),
            vec!["user-1", "user-2"]
        );
        assert_eq!(
            crate::parse_channel_type("direct message"),
            Some(macro_api::ChannelType::DirectMessage)
        );
        let (mentions, attachments) =
            crate::parse_channel_composer_fields("USER:user-2", "document:doc-1").unwrap();
        assert_eq!(mentions[0].entity_type, "USER");
        assert_eq!(mentions[0].entity_id, "user-2");
        assert_eq!(attachments[0].entity_type, "document");
        assert!(crate::parse_channel_composer_fields("missing-colon", "").is_err());
        let preferences: crate::AppPreferences =
            serde_json::from_value(json!({"theme": "light"})).unwrap();
        assert!(preferences.sidebar_expanded);
        assert_eq!(preferences.detail_split_position, 0.42);
        assert_eq!(
            crate::parse_nav_section("channels"),
            Some(crate::NavSection::Channels)
        );
    }

    #[test]
    fn file_preview_classification_and_truncation_are_bounded() {
        assert!(crate::is_image_file_type("PNG"));
        assert!(crate::is_text_preview_file_type("rs"));
        assert!(!crate::is_text_preview_file_type("pdf"));
        assert_eq!(crate::truncate_preview_text("hello", 5), "hello");
        assert!(crate::truncate_preview_text("abcdef", 3).starts_with("abc\n\n…"));
    }
}
