use macro_api::AgentSessionLogEntry;
use macro_models::EntityType;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallEvent {
    Started {
        channel_id: String,
        call_id: String,
        created_by: Option<String>,
    },
    Ended {
        channel_id: String,
        call_id: String,
    },
    Answered {
        channel_id: Option<String>,
        call_id: String,
        answered_by: Option<String>,
    },
    ShareWithTeamToggled {
        channel_id: String,
        call_id: String,
        share_with_team: bool,
        toggled_by: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentSessionLogEvent {
    pub session_id: String,
    pub entry: AgentSessionLogEntry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationUpdateKind {
    CreateComment,
    EditComment,
    DeleteComment,
    CreateAnchor,
    EditAnchor,
    DeleteAnchor,
}

/// The stable addressing envelope emitted by document-storage's `comment`
/// connection-gateway topic. The response body varies by update kind, so the
/// realtime layer only decodes the fields needed for exact cache invalidation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotationRealtimeEvent {
    pub document_id: String,
    pub sender: String,
    pub kind: AnnotationUpdateKind,
}

pub fn parse_annotation_realtime_event(
    event_type: &str,
    raw_data: &Value,
) -> Option<AnnotationRealtimeEvent> {
    if event_type != "comment" {
        return None;
    }
    let data = normalized_data(raw_data)?;
    let payload = data.get("payload")?;
    let kind = match string(&data, "updateType")? {
        "create-comment" => AnnotationUpdateKind::CreateComment,
        "edit-comment" => AnnotationUpdateKind::EditComment,
        "delete-comment" => AnnotationUpdateKind::DeleteComment,
        "create-anchor" => AnnotationUpdateKind::CreateAnchor,
        "edit-anchor" => AnnotationUpdateKind::EditAnchor,
        "delete-anchor" => AnnotationUpdateKind::DeleteAnchor,
        _ => return None,
    };
    Some(AnnotationRealtimeEvent {
        document_id: identifier(payload, &["documentId", "document_id"])?,
        sender: identifier(payload, &["sender"])?,
        kind,
    })
}

/// A scheduled-agent execution update emitted by Macro's scheduled-action
/// service over the connection gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduledActionEvent {
    Started {
        action_id: String,
        chat_id: String,
    },
    Stopped {
        action_id: String,
        chat_id: String,
        is_success: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulkUploadStatus {
    PartiallyCompleted,
    Completed,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkUploadEvent {
    pub request_id: String,
    pub project_id: Option<String>,
    pub status: BulkUploadStatus,
}

/// Targeted entity patch emitted by Macro's authenticated GraphQL Soup
/// subscription. A cache deletion means the entity was deleted or is no longer
/// visible to the current user; callers must fail closed in either case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoupRealtimePatch {
    Updated {
        entity_type: EntityType,
        entity_id: String,
    },
    Deleted {
        entity_type: EntityType,
        entity_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NotificationStatusUpdate {
    #[serde(rename = "type")]
    pub event_type: String,
    pub updates: Vec<NotificationStatusMutation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "t", content = "c")]
pub enum NotificationStatusMutation {
    Patch(NotificationStatusPatch),
    Delete(NotificationStatusDelete),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NotificationStatusPatch {
    pub id: String,
    pub done: bool,
    pub viewed_at: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NotificationStatusDelete {
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommsTypingAction {
    Start,
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CommsTypingEvent {
    pub channel_id: String,
    pub user_id: String,
    pub action: CommsTypingAction,
    #[serde(default)]
    pub thread_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CommsRealtimeEvent {
    Message {
        channel_id: String,
        message_id: String,
        thread_id: Option<String>,
        deleted_at: Option<String>,
        raw: Value,
    },
    Reaction {
        channel_id: String,
        message_id: String,
        raw: Value,
    },
    Attachment {
        channel_id: String,
        message_id: String,
        raw: Value,
    },
}

pub fn parse_comms_realtime_event(
    event_type: &str,
    raw_data: &Value,
) -> Option<CommsRealtimeEvent> {
    let data = normalized_data(raw_data)?;
    let channel_id = identifier(&data, &["channel_id", "channelId"])?;
    match event_type {
        "comms_message" => Some(CommsRealtimeEvent::Message {
            channel_id,
            message_id: identifier(&data, &["id"])?,
            thread_id: identifier(&data, &["thread_id", "threadId"]),
            deleted_at: identifier(&data, &["deleted_at", "deletedAt"]),
            raw: data,
        }),
        "comms_reaction" if data.get("reactions").is_some_and(Value::is_array) => {
            Some(CommsRealtimeEvent::Reaction {
                channel_id,
                message_id: identifier(&data, &["message_id", "messageId"])?,
                raw: data,
            })
        }
        "comms_attachment" if data.get("attachments").is_some_and(Value::is_array) => {
            Some(CommsRealtimeEvent::Attachment {
                channel_id,
                message_id: identifier(&data, &["message_id", "messageId"])?,
                raw: data,
            })
        }
        _ => None,
    }
}

pub fn parse_comms_typing_event(event_type: &str, raw_data: &Value) -> Option<CommsTypingEvent> {
    if event_type != "comms_typing" {
        return None;
    }
    serde_json::from_value(normalized_data(raw_data)?).ok()
}

pub fn parse_notification_status_update(
    event_type: &str,
    raw_data: &Value,
) -> Option<NotificationStatusUpdate> {
    if event_type != "notification_status_updated" {
        return None;
    }
    let data = normalized_data(raw_data)?;
    let update: NotificationStatusUpdate = serde_json::from_value(data).ok()?;
    (update.event_type == event_type).then_some(update)
}

/// Parses connection-gateway's generated `UploadFolderStatusUpdate` union.
/// The gateway may deliver its `data` field as either an object or a JSON
/// string, matching the compatibility behavior used by Macro web.
pub fn parse_bulk_upload_event(event_type: &str, raw_data: &Value) -> Option<BulkUploadEvent> {
    if event_type != "bulk_upload" {
        return None;
    }
    let data = normalized_data(raw_data)?;
    let status = match string(&data, "status")? {
        "partially_completed" => BulkUploadStatus::PartiallyCompleted,
        "completed" => BulkUploadStatus::Completed,
        "failed" => BulkUploadStatus::Failed,
        "unknown" => BulkUploadStatus::Unknown,
        _ => return None,
    };
    Some(BulkUploadEvent {
        request_id: identifier(&data, &["requestId", "request_id"])?,
        project_id: identifier(&data, &["projectId", "project_id"]),
        status,
    })
}

/// Parses the minimal `SoupUpdates` GraphQL selection used by the native
/// client. Unknown future entity kinds are ignored without dropping valid
/// patches from the same transport frame.
pub fn parse_soup_realtime_patches(payload: &Value) -> Vec<SoupRealtimePatch> {
    payload
        .get("data")
        .and_then(|data| data.get("soupUpdates"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|patch| match string(patch, "__typename")? {
            "SoupUpdated" => {
                let item = patch.get("item")?;
                Some(SoupRealtimePatch::Updated {
                    entity_type: soup_entity_type(string(item, "entityType")?)?,
                    entity_id: identifier(item, &["id"])?,
                })
            }
            "GraphqlCacheDeletion" => Some(SoupRealtimePatch::Deleted {
                entity_type: soup_graphql_type(string(patch, "graphqlTypeName")?)?,
                entity_id: identifier(patch, &["entityId", "entity_id"])?,
            }),
            _ => None,
        })
        .collect()
}

fn soup_entity_type(value: &str) -> Option<EntityType> {
    match value {
        "DOCUMENT" => Some(EntityType::Document),
        "CHAT" => Some(EntityType::Chat),
        "PROJECT" => Some(EntityType::Project),
        "EMAIL_THREAD" => Some(EntityType::EmailThread),
        "CHANNEL" => Some(EntityType::Channel),
        "CHANNEL_MESSAGE" => Some(EntityType::ChannelMessage),
        "CALL" => Some(EntityType::CallRecord),
        "CRM_COMPANY" => Some(EntityType::CrmCompany),
        "FOREIGN_ENTITY" => Some(EntityType::ForeignEntity),
        "CALENDAR_EVENT" => Some(EntityType::CalendarEvent),
        "REMINDER" => Some(EntityType::Reminder),
        _ => None,
    }
}

fn soup_graphql_type(value: &str) -> Option<EntityType> {
    match value {
        "GraphqlSoupDocument" => Some(EntityType::Document),
        "GraphqlSoupChat" => Some(EntityType::Chat),
        "GraphqlSoupProject" => Some(EntityType::Project),
        "GraphqlSoupEmailThread" => Some(EntityType::EmailThread),
        "GraphqlSoupChannel" => Some(EntityType::Channel),
        "GraphqlSoupChannelMessage" | "GraphqlSoupChannelMessagePreview" => {
            Some(EntityType::ChannelMessage)
        }
        "GraphqlSoupCall" => Some(EntityType::CallRecord),
        "GraphqlSoupCrmCompany" => Some(EntityType::CrmCompany),
        "GraphqlSoupForeignEntity" => Some(EntityType::ForeignEntity),
        "GraphqlSoupCalendarEvent" => Some(EntityType::CalendarEvent),
        "GraphqlSoupReminder" => Some(EntityType::Reminder),
        _ => None,
    }
}

/// Parse the hand-written realtime log contract emitted by
/// `agent_session::outbound::connection_gateway_realtime`. Apart from the
/// addressing session id, the payload is byte-for-byte the same entry shape
/// returned by the harness log endpoint.
pub fn parse_agent_session_log_event(
    event_type: &str,
    raw_data: &Value,
) -> Option<AgentSessionLogEvent> {
    if event_type != "agent_session_log" {
        return None;
    }
    let data = normalized_data(raw_data)?;
    Some(AgentSessionLogEvent {
        session_id: identifier(&data, &["agentSessionId", "agent_session_id"])?,
        entry: serde_json::from_value(data).ok()?,
    })
}

/// Parse the generated `ScheduledActionUpdate` tagged union. The gateway can
/// carry either the object itself or a JSON-encoded object.
pub fn parse_scheduled_action_event(
    event_type: &str,
    raw_data: &Value,
) -> Option<ScheduledActionEvent> {
    if event_type != "scheduled_action_update" {
        return None;
    }
    let data = normalized_data(raw_data)?;
    let action_id = identifier(&data, &["action_id", "actionId"])?;
    let chat_id = identifier(&data, &["chat_id", "chatId"])?;
    match string(&data, "type")? {
        "started" => Some(ScheduledActionEvent::Started { action_id, chat_id }),
        "stopped" => Some(ScheduledActionEvent::Stopped {
            action_id,
            chat_id,
            is_success: data.get("is_success").and_then(Value::as_bool)?,
        }),
        _ => None,
    }
}

/// Parses the hand-written call event payloads emitted by Macro's call service.
/// Frames may contain an object or a JSON-encoded object, matching the web
/// gateway's compatibility behavior.
pub fn parse_call_event(event_type: &str, raw_data: &Value) -> Option<CallEvent> {
    let data = normalized_data(raw_data)?;
    match event_type {
        "call_started" => Some(CallEvent::Started {
            channel_id: identifier(&data, &["channel_id", "channelId"])?,
            call_id: identifier(&data, &["call_id", "callId"])?,
            created_by: identifier(&data, &["created_by", "createdBy"]),
        }),
        "call_ended" => Some(CallEvent::Ended {
            channel_id: identifier(&data, &["channel_id", "channelId"])?,
            call_id: identifier(&data, &["call_id", "callId"])?,
        }),
        "call_answered" => Some(CallEvent::Answered {
            channel_id: identifier(&data, &["channel_id", "channelId"]),
            call_id: identifier(&data, &["call_id", "callId"])?,
            answered_by: identifier(&data, &["user_id", "userId"]),
        }),
        "call_share_with_team_toggled" => Some(CallEvent::ShareWithTeamToggled {
            channel_id: identifier(&data, &["channel_id", "channelId"])?,
            call_id: identifier(&data, &["call_id", "callId"])?,
            share_with_team: data
                .get("share_with_team")
                .or_else(|| data.get("shareWithTeam"))
                .and_then(Value::as_bool)?,
            toggled_by: identifier(&data, &["toggled_by", "toggledBy"]),
        }),
        _ => None,
    }
}

fn normalized_data(raw_data: &Value) -> Option<Value> {
    match raw_data {
        Value::Object(_) => Some(raw_data.clone()),
        Value::String(data) => serde_json::from_str(data).ok(),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileTarget {
    Inbox,
    Channels,
    ChannelMessages(String),
    DocumentComments(String),
    ActiveCalls,
    EmailLinks,
    /// Every mail query that is already resident in the client. This is
    /// intentionally distinct from `Inbox`: Signal/Noise/Sent/Drafts and
    /// per-account mail tabs are separate server-backed query caches.
    MailViews,
    Contacts,
    Notifications,
    Favorites,
    PropertyDefinitions,
    StarterDocuments,
    Imports,
    EntityType(EntityType),
    Entity {
        entity_type: EntityType,
        entity_id: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcilePlan {
    targets: Vec<ReconcileTarget>,
}

impl ReconcilePlan {
    pub fn new(targets: impl IntoIterator<Item = ReconcileTarget>) -> Self {
        let mut plan = Self::default();
        for target in targets {
            plan.add(target);
        }
        plan
    }

    pub fn targets(&self) -> &[ReconcileTarget] {
        &self.targets
    }

    fn add(&mut self, target: ReconcileTarget) {
        if !self.targets.contains(&target) {
            self.targets.push(target);
        }
    }
}

/// Translate Macro connection-gateway events into the smallest client cache
/// reconciliation plan that can represent them today.
///
/// Payload fields are grounded in Macro's Rust event producers. Stringified JSON
/// is accepted because the web client supports both object and JSON-string
/// payloads at this boundary.
pub fn reconciliation_plan(message_type: &str, raw_data: &Value) -> ReconcilePlan {
    let decoded;
    let data = if let Some(raw) = raw_data.as_str() {
        decoded = serde_json::from_str(raw).unwrap_or(Value::Null);
        &decoded
    } else {
        raw_data
    };

    let mut plan = ReconcilePlan::default();
    match message_type {
        "notification" => {
            plan.add(ReconcileTarget::Notifications);
            plan.add(ReconcileTarget::Inbox);
        }
        "notification_status_updated" => {
            // The native notification collection is patched directly from the
            // typed event; only the notification-backed soup needs a refetch.
            plan.add(ReconcileTarget::Inbox);
        }
        "contacts_invalidation" => {
            plan.add(ReconcileTarget::Contacts);
        }
        "comms_message" => {
            plan.add(ReconcileTarget::Inbox);
            plan.add(ReconcileTarget::Channels);
        }
        "comms_reaction" | "comms_attachment" => {}
        "comment" => {
            if let Some(event) = parse_annotation_realtime_event(message_type, raw_data) {
                plan.add(ReconcileTarget::DocumentComments(event.document_id));
            }
        }
        "refresh_email" => {
            plan.add(ReconcileTarget::Inbox);
            plan.add(ReconcileTarget::MailViews);
            plan.add(ReconcileTarget::EntityType(EntityType::EmailThread));
            if matches!(string(data, "event"), Some("link_removed" | "photo_synced")) {
                plan.add(ReconcileTarget::EmailLinks);
            }
        }
        "refresh_calendar" => {
            plan.add(ReconcileTarget::Inbox);
            plan.add(ReconcileTarget::EntityType(EntityType::CalendarEvent));
        }
        "starter_docs_initialized" => {
            plan.add(ReconcileTarget::Inbox);
            plan.add(ReconcileTarget::Favorites);
            plan.add(ReconcileTarget::PropertyDefinitions);
            plan.add(ReconcileTarget::StarterDocuments);
        }
        "import_updated" => {
            plan.add(ReconcileTarget::Imports);
        }
        "task_duplicate_matches_updated" => {
            if let Some(document_id) =
                identifier(data, &["document_id", "documentId", "task_id", "taskId"])
            {
                plan.add(ReconcileTarget::Entity {
                    entity_type: EntityType::Document,
                    entity_id: document_id,
                });
            }
        }
        "agent_session_log" => {
            if let Some(session_id) = identifier(data, &["agentSessionId", "agent_session_id"]) {
                plan.add(ReconcileTarget::Entity {
                    entity_type: EntityType::AgentSession,
                    entity_id: session_id,
                });
            }
        }
        "agent_session_renamed" => {
            plan.add(ReconcileTarget::Inbox);
            if let Some(session_id) = identifier(data, &["agentSessionId", "agent_session_id"]) {
                plan.add(ReconcileTarget::Entity {
                    entity_type: EntityType::AgentSession,
                    entity_id: session_id,
                });
            }
        }
        "scheduled_action_update" => {
            // The app patches loaded schedules/history from the typed event.
            // Inbox still needs reconciliation because completed agent runs
            // materialize as chats there.
            plan.add(ReconcileTarget::Inbox);
        }
        "call_started" | "call_ended" | "call_answered" | "call_share_with_team_toggled" => {
            plan.add(ReconcileTarget::ActiveCalls);
        }
        "bulk_upload" => {
            plan.add(ReconcileTarget::Inbox);
        }
        _ => {}
    }
    plan
}

fn string<'a>(data: &'a Value, key: &str) -> Option<&'a str> {
    data.get(key).and_then(Value::as_str)
}

fn identifier(data: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| string(data, key))
        .or_else(|| {
            data.get("message")
                .and_then(|message| keys.iter().find_map(|key| string(message, key)))
        })
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn soup_updates_preserve_exact_entity_targets_and_fail_closed_deletions() {
        let patches = parse_soup_realtime_patches(&json!({
            "data": {
                "soupUpdates": [
                    {
                        "__typename": "SoupUpdated",
                        "item": {
                            "__typename": "GraphqlSoupDocument",
                            "id": "document-1",
                            "entityType": "DOCUMENT"
                        }
                    },
                    {
                        "__typename": "GraphqlCacheDeletion",
                        "graphqlTypeName": "GraphqlSoupEmailThread",
                        "entityId": "email-2"
                    },
                    {
                        "__typename": "SoupUpdated",
                        "item": {
                            "id": "company-3",
                            "entityType": "CRM_COMPANY"
                        }
                    },
                    {
                        "__typename": "SoupUpdated",
                        "item": {"id": "future-4", "entityType": "FUTURE_ENTITY"}
                    },
                    {"__typename": "FutureSoupPatch", "id": "future-5"}
                ]
            }
        }));

        assert_eq!(
            patches,
            vec![
                SoupRealtimePatch::Updated {
                    entity_type: EntityType::Document,
                    entity_id: "document-1".into(),
                },
                SoupRealtimePatch::Deleted {
                    entity_type: EntityType::EmailThread,
                    entity_id: "email-2".into(),
                },
                SoupRealtimePatch::Updated {
                    entity_type: EntityType::CrmCompany,
                    entity_id: "company-3".into(),
                },
            ]
        );
    }

    #[test]
    fn malformed_soup_payloads_are_ignored_without_broad_reconciliation() {
        assert!(parse_soup_realtime_patches(&json!({})).is_empty());
        assert!(
            parse_soup_realtime_patches(&json!({
                "data": {"soupUpdates": [
                    {"__typename":"SoupUpdated","item":{"entityType":"DOCUMENT"}},
                    {"__typename":"GraphqlCacheDeletion","entityId":"document-1"}
                ]}
            }))
            .is_empty()
        );
    }

    #[test]
    fn channel_message_refreshes_only_comms_surfaces() {
        let plan = reconciliation_plan(
            "comms_message",
            &json!({"channel_id": "channel-1", "id": "message-1"}),
        );
        assert_eq!(
            plan.targets(),
            [ReconcileTarget::Inbox, ReconcileTarget::Channels,]
        );
    }

    #[test]
    fn nested_and_stringified_channel_payloads_are_supported() {
        let nested = reconciliation_plan(
            "comms_reaction",
            &json!({"message": {"channel_id": "channel-2"}}),
        );
        assert!(nested.targets().is_empty());

        let stringified = reconciliation_plan(
            "comms_attachment",
            &Value::String(r#"{"channel_id":"channel-3"}"#.into()),
        );
        assert!(stringified.targets().is_empty());
    }

    #[test]
    fn comms_realtime_events_keep_exact_message_targets_and_payloads() {
        let message = parse_comms_realtime_event(
            "comms_message",
            &Value::String(
                serde_json::json!({
                    "id":"reply-1",
                    "channel_id":"channel-1",
                    "thread_id":"message-1",
                    "sender_id":"user-2",
                    "content":"Edited",
                    "created_at":"2026-08-28T12:00:00Z",
                    "updated_at":"2026-08-28T12:01:00Z",
                    "edited_at":"2026-08-28T12:01:00Z",
                    "nonce":"n-1"
                })
                .to_string(),
            ),
        )
        .unwrap();
        assert!(matches!(
            message,
            CommsRealtimeEvent::Message { channel_id, message_id, thread_id: Some(thread_id), .. }
                if channel_id == "channel-1" && message_id == "reply-1" && thread_id == "message-1"
        ));
        assert!(matches!(
            parse_comms_realtime_event(
                "comms_reaction",
                &serde_json::json!({
                    "channel_id":"channel-1","message_id":"message-1",
                    "reactions":[{"emoji":"👍","users":["user-1"]}],"nonce":"n-2"
                })
            ),
            Some(CommsRealtimeEvent::Reaction { message_id, .. }) if message_id == "message-1"
        ));
        assert!(
            parse_comms_realtime_event(
                "comms_attachment",
                &serde_json::json!({"channel_id":"channel-1","message_id":"message-1"})
            )
            .is_none()
        );
    }

    #[test]
    fn email_events_do_not_reload_unrelated_services() {
        let plan = reconciliation_plan(
            "refresh_email",
            &json!({"event": "upsert_message", "link_id": "link-1"}),
        );
        assert_eq!(
            plan.targets(),
            [
                ReconcileTarget::Inbox,
                ReconcileTarget::MailViews,
                ReconcileTarget::EntityType(EntityType::EmailThread),
            ]
        );

        let removed = reconciliation_plan(
            "refresh_email",
            &json!({"event": "link_removed", "link_id": "link-1"}),
        );
        assert!(removed.targets().contains(&ReconcileTarget::EmailLinks));
    }

    #[test]
    fn agent_events_address_one_session() {
        let plan = reconciliation_plan(
            "agent_session_log",
            &json!({"agentSessionId": "session-1", "content": {}}),
        );
        assert_eq!(
            plan.targets(),
            [ReconcileTarget::Entity {
                entity_type: EntityType::AgentSession,
                entity_id: "session-1".into(),
            }]
        );
    }

    #[test]
    fn agent_log_event_preserves_the_exact_persisted_entry_shape() {
        let event = parse_agent_session_log_event(
            "agent_session_log",
            &Value::String(
                serde_json::json!({
                    "agentSessionId":"session-1",
                    "createdAt":"2026-08-28T12:00:00Z",
                    "userId":"macro|owner",
                    "direction":"to_server",
                    "content":{"type":"acp","jsonrpc":"2.0","method":"session/update","params":{}}
                })
                .to_string(),
            ),
        )
        .unwrap();
        assert_eq!(event.session_id, "session-1");
        assert_eq!(event.entry.user_id.as_deref(), Some("macro|owner"));
        assert_eq!(
            event.entry.direction,
            macro_api::AgentLogDirection::ToServer
        );
        assert_eq!(event.entry.content["method"], "session/update");
    }

    #[test]
    fn scheduled_action_events_match_the_generated_tagged_union() {
        assert_eq!(
            parse_scheduled_action_event(
                "scheduled_action_update",
                &Value::String(
                    r#"{"type":"started","action_id":"action-1","chat_id":"chat-1","owner":"macro|owner"}"#.into(),
                ),
            ),
            Some(ScheduledActionEvent::Started {
                action_id: "action-1".into(),
                chat_id: "chat-1".into(),
            })
        );
        assert_eq!(
            parse_scheduled_action_event(
                "scheduled_action_update",
                &serde_json::json!({
                    "type":"stopped",
                    "action_id":"action-1",
                    "chat_id":"chat-1",
                    "owner":"macro|owner",
                    "is_success":true
                }),
            ),
            Some(ScheduledActionEvent::Stopped {
                action_id: "action-1".into(),
                chat_id: "chat-1".into(),
                is_success: true,
            })
        );
        assert!(
            parse_scheduled_action_event(
                "scheduled_action_update",
                &serde_json::json!({"type":"stopped","action_id":"action-1"})
            )
            .is_none()
        );
        assert_eq!(
            reconciliation_plan("scheduled_action_update", &serde_json::json!({})).targets(),
            [ReconcileTarget::Inbox]
        );
    }

    #[test]
    fn call_events_refresh_only_call_state() {
        for event in [
            "call_started",
            "call_ended",
            "call_answered",
            "call_share_with_team_toggled",
        ] {
            assert_eq!(
                reconciliation_plan(event, &json!({})).targets(),
                [ReconcileTarget::ActiveCalls]
            );
        }
    }

    #[test]
    fn call_events_match_the_hand_written_call_service_payloads() {
        assert_eq!(
            parse_call_event(
                "call_started",
                &Value::String(
                    r#"{"channel_id":"channel-1","call_id":"call-1","created_by":"user-1"}"#.into(),
                ),
            ),
            Some(CallEvent::Started {
                channel_id: "channel-1".into(),
                call_id: "call-1".into(),
                created_by: Some("user-1".into()),
            })
        );
        assert_eq!(
            parse_call_event(
                "call_answered",
                &json!({"call_id":"call-1","user_id":"user-2"}),
            ),
            Some(CallEvent::Answered {
                channel_id: None,
                call_id: "call-1".into(),
                answered_by: Some("user-2".into()),
            })
        );
        assert!(parse_call_event("call_started", &json!({"call_id":"call-1"})).is_none());
        assert!(parse_call_event("future_call", &json!({})).is_none());
    }

    #[test]
    fn unknown_and_typing_events_do_not_force_refetches() {
        assert!(
            reconciliation_plan("comms_typing", &json!({"channel_id": "channel-1"}))
                .targets()
                .is_empty()
        );
        assert!(
            reconciliation_plan("future_event", &json!({}))
                .targets()
                .is_empty()
        );
    }

    #[test]
    fn annotation_events_target_only_the_affected_document_discussion() {
        let wire = Value::String(
            r#"{"updateType":"edit-comment","payload":{"sender":"user-2","documentId":"document-7","response":{"comment":{}}}}"#
                .into(),
        );
        assert_eq!(
            parse_annotation_realtime_event("comment", &wire),
            Some(AnnotationRealtimeEvent {
                document_id: "document-7".into(),
                sender: "user-2".into(),
                kind: AnnotationUpdateKind::EditComment,
            })
        );
        assert_eq!(
            reconciliation_plan("comment", &wire).targets(),
            [ReconcileTarget::DocumentComments("document-7".into())]
        );
        assert!(
            parse_annotation_realtime_event(
                "comment",
                &json!({
                    "updateType": "future-comment-update",
                    "payload": {"sender":"user-2","documentId":"document-7"}
                })
            )
            .is_none()
        );
    }

    #[test]
    fn comms_typing_events_preserve_main_timeline_and_thread_scopes() {
        assert_eq!(
            parse_comms_typing_event(
                "comms_typing",
                &Value::String(
                    r#"{"channel_id":"channel-1","user_id":"user-2","action":"start","thread_id":"message-8"}"#
                        .into(),
                ),
            ),
            Some(CommsTypingEvent {
                channel_id: "channel-1".into(),
                user_id: "user-2".into(),
                action: CommsTypingAction::Start,
                thread_id: Some("message-8".into()),
            })
        );
        assert_eq!(
            parse_comms_typing_event(
                "comms_typing",
                &serde_json::json!({
                    "channel_id":"channel-1",
                    "user_id":"user-2",
                    "action":"stop"
                }),
            )
            .unwrap()
            .thread_id,
            None
        );
    }

    #[test]
    fn starter_document_initialization_refreshes_seeded_cross_cutting_state() {
        assert_eq!(
            reconciliation_plan("starter_docs_initialized", &serde_json::json!({})).targets(),
            [
                ReconcileTarget::Inbox,
                ReconcileTarget::Favorites,
                ReconcileTarget::PropertyDefinitions,
                ReconcileTarget::StarterDocuments,
            ]
        );
    }

    #[test]
    fn import_updates_refresh_only_the_import_aggregate() {
        assert_eq!(
            reconciliation_plan("import_updated", &serde_json::json!({"id":"import-1"})).targets(),
            [ReconcileTarget::Imports]
        );
    }

    #[test]
    fn notification_events_refresh_notification_and_inbox_views() {
        assert_eq!(
            reconciliation_plan("notification", &json!({"id": "notification-1"})).targets(),
            [ReconcileTarget::Notifications, ReconcileTarget::Inbox]
        );
    }

    #[test]
    fn contacts_invalidation_refreshes_only_the_shared_people_cache() {
        assert_eq!(
            reconciliation_plan("contacts_invalidation", &json!({})).targets(),
            [ReconcileTarget::Contacts]
        );
    }

    #[test]
    fn bulk_upload_status_matches_the_generated_gateway_union() {
        assert_eq!(
            parse_bulk_upload_event(
                "bulk_upload",
                &Value::String(
                    r#"{"requestId":"upload-1","projectId":"project-9","status":"completed"}"#
                        .into(),
                ),
            ),
            Some(BulkUploadEvent {
                request_id: "upload-1".into(),
                project_id: Some("project-9".into()),
                status: BulkUploadStatus::Completed,
            })
        );
        assert_eq!(
            parse_bulk_upload_event(
                "bulk_upload",
                &serde_json::json!({"request_id":"upload-2","status":"failed"}),
            ),
            Some(BulkUploadEvent {
                request_id: "upload-2".into(),
                project_id: None,
                status: BulkUploadStatus::Failed,
            })
        );
        assert!(
            parse_bulk_upload_event(
                "bulk_upload",
                &serde_json::json!({
                    "requestId":"upload-3", "status":"processing"
                })
            )
            .is_none()
        );
    }

    #[test]
    fn notification_status_updates_match_macro_patch_delete_schema() {
        let update = parse_notification_status_update(
            "notification_status_updated",
            &Value::String(
                serde_json::json!({
                    "type":"notification_status_updated",
                    "updates":[
                        {"t":"Patch","c":{"id":"n1","done":false,"viewed_at":"2026-08-28T12:00:00Z","updated_at":"2026-08-28T12:00:00Z"}},
                        {"t":"Delete","c":{"id":"n2"}}
                    ]
                })
                .to_string(),
            ),
        )
        .unwrap();
        assert_eq!(update.updates.len(), 2);
        assert!(matches!(
            &update.updates[0],
            NotificationStatusMutation::Patch(patch)
                if patch.id == "n1" && !patch.done && patch.viewed_at.is_some()
        ));
        assert!(matches!(
            &update.updates[1],
            NotificationStatusMutation::Delete(delete) if delete.id == "n2"
        ));
        assert!(parse_notification_status_update(
            "notification_status_updated",
            &serde_json::json!({"type":"notification_status_updated","updates":[{"t":"Patch","c":{"id":"n1"}}]})
        )
        .is_none());
    }
}
