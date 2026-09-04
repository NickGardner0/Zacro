use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

// === Entity System ===

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    User,
    Chat,
    Channel,
    ChannelMessage,
    Document,
    Project,
    EmailThread,
    EmailMessage,
    CalendarEvent,
    Team,
    Call,
    CallRecord,
    ForeignEntity,
    StaticFile,
    CrmCompany,
    CrmContact,
    Reminder,
    Skill,
    AgentSession,
}

impl fmt::Display for EntityType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EntityType::User => write!(f, "user"),
            EntityType::Chat => write!(f, "chat"),
            EntityType::Channel => write!(f, "channel"),
            EntityType::ChannelMessage => write!(f, "channel_message"),
            EntityType::Document => write!(f, "document"),
            EntityType::Project => write!(f, "project"),
            EntityType::EmailThread => write!(f, "email"),
            EntityType::EmailMessage => write!(f, "email_message"),
            EntityType::CalendarEvent => write!(f, "calendar_event"),
            EntityType::Team => write!(f, "team"),
            EntityType::Call => write!(f, "call"),
            EntityType::CallRecord => write!(f, "call_record"),
            EntityType::ForeignEntity => write!(f, "foreign"),
            EntityType::StaticFile => write!(f, "static_file"),
            EntityType::CrmCompany => write!(f, "crm_company"),
            EntityType::CrmContact => write!(f, "crm_contact"),
            EntityType::Reminder => write!(f, "reminder"),
            EntityType::Skill => write!(f, "skill"),
            EntityType::AgentSession => write!(f, "agent_session"),
        }
    }
}

// === User ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub email: String,
    pub name: String,
    pub avatar_url: Option<String>,
    pub created_at: DateTime<Utc>,
}

// === Team ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMember {
    pub user_id: String,
    pub team_id: String,
    pub role: TeamRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamRole {
    Owner,
    Admin,
    Member,
}

// === Document ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: String,
    pub name: String,
    pub file_type: String,
    pub owner: String,
    pub project_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub sub_type: Option<String>,
    pub is_completed: Option<bool>,
}

// === Chat (AI) ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chat {
    pub id: String,
    pub name: String,
    pub user_id: String,
    pub model: Option<String>,
    pub project_id: Option<String>,
    pub token_count: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub chat_id: String,
    pub role: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

// === Project ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub user_id: String,
    pub parent_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

// === Email ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailLink {
    pub id: String,
    pub email_address: String,
    pub provider: String,
    pub is_sync_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailThread {
    pub id: String,
    pub provider_id: Option<String>,
    pub link_id: String,
    pub subject: String,
    pub is_read: bool,
    pub is_starred: bool,
    pub inbox_visible: bool,
    pub project_id: Option<String>,
    pub latest_inbound_message_ts: Option<DateTime<Utc>>,
    pub latest_outbound_message_ts: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailMessage {
    pub id: String,
    pub thread_id: String,
    pub from_contact_id: Option<String>,
    pub subject: String,
    pub body_text: Option<String>,
    pub body_html_sanitized: Option<String>,
    pub is_read: bool,
    pub is_starred: bool,
    pub is_sent: bool,
    pub is_draft: bool,
    pub replying_to_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailContact {
    pub id: String,
    pub link_id: String,
    pub email_address: String,
    pub name: Option<String>,
    pub sfs_photo_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailLabel {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailAttachment {
    pub id: String,
    pub message_id: String,
    pub filename: String,
    pub content_type: Option<String>,
    pub size: i64,
}

// === Channel / Messages ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub channel_type: ChannelType,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelType {
    Public,
    Organization,
    Private,
    DirectMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelParticipant {
    pub channel_id: String,
    pub user_id: String,
    pub role: ChannelRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelRole {
    Owner,
    Admin,
    Member,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub channel_id: String,
    pub thread_id: Option<String>,
    pub sender_id: String,
    pub content: String,
    pub edited_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageReaction {
    pub message_id: String,
    pub user_id: String,
    pub emoji: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageAttachment {
    pub id: String,
    pub message_id: String,
    pub entity_type: EntityType,
    pub entity_id: String,
}

// === Task ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub is_completed: bool,
    pub project_id: Option<String>,
    pub document_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// === Call ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallRecord {
    pub id: String,
    pub channel_id: String,
    pub duration_ms: Option<i64>,
    pub recording_url: Option<String>,
    pub summary: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

// === CRM ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrmCompany {
    pub id: String,
    pub team_id: String,
    pub name: String,
    pub email_sync: bool,
    pub hidden: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrmContact {
    pub id: String,
    pub company_id: Option<String>,
    pub email: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrmDomain {
    pub id: String,
    pub company_id: String,
    pub domain: String,
}

// === Calendar ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarEvent {
    pub id: String,
    pub owner_id: String,
    pub title: String,
    pub status: Option<String>,
    pub starts_at: Option<DateTime<Utc>>,
    pub ends_at: Option<DateTime<Utc>>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub conference_url: Option<String>,
    pub is_read_only: bool,
    pub created_at: DateTime<Utc>,
}

// === Reminder ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reminder {
    pub id: String,
    pub user_id: String,
    pub description: String,
    pub entity_type: Option<EntityType>,
    pub entity_id: Option<String>,
    pub remind_at: Option<DateTime<Utc>>,
    pub cron: Option<String>,
    pub timezone: Option<String>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub enabled: bool,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

// === Favorite ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Favorite {
    pub entity_type: EntityType,
    pub entity_id: String,
    pub position: Option<i32>,
}

// === Notification ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: String,
    pub user_id: String,
    pub entity_type: EntityType,
    pub entity_id: String,
    pub notification_type: String,
    pub message: String,
    pub is_seen: bool,
    pub is_done: bool,
    pub created_at: DateTime<Utc>,
}

// === Properties ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertyDefinition {
    pub id: String,
    pub name: String,
    pub data_type: PropertyDataType,
    pub owner_type: PropertyOwnerType,
    pub owner_id: Option<String>,
    pub entity_types: Vec<EntityType>,
    pub options: Vec<PropertyOption>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropertyDataType {
    Boolean,
    Date,
    Number,
    String,
    SelectNumber,
    SelectString,
    Tag,
    Entity,
    Link,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropertyOwnerType {
    User,
    Team,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertyOption {
    pub id: String,
    pub label: String,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityProperty {
    pub id: String,
    pub entity_type: EntityType,
    pub entity_id: String,
    pub property_definition_id: String,
    pub value: serde_json::Value,
}

// === Agent Session ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSession {
    pub id: String,
    pub name: Option<String>,
    pub user_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// === Search ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    pub query: String,
    pub entity_types: Option<Vec<EntityType>>,
    pub project_id: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub entity_type: EntityType,
    pub entity_id: String,
    pub title: String,
    pub preview: Option<String>,
    pub score: f64,
}

// === WebSocket Events ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub entity_type: EntityType,
    pub entity_id: String,
    pub data: serde_json::Value,
}

// === API Auth ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthToken {
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub id: String,
    pub email: String,
    pub name: String,
    pub avatar_url: Option<String>,
    pub teams: Vec<Team>,
}

// === Soup (Unified Feed) ===

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "entity_type", content = "data")]
pub enum SoupItem {
    #[serde(rename = "document")]
    Document(Document),
    #[serde(rename = "chat")]
    Chat(Chat),
    #[serde(rename = "project")]
    Project(Project),
    #[serde(rename = "email")]
    EmailThread(EmailThread),
    #[serde(rename = "channel")]
    Channel(Channel),
    #[serde(rename = "call_record")]
    CallRecord(CallRecord),
    #[serde(rename = "calendar_event")]
    CalendarEvent(CalendarEvent),
    #[serde(rename = "crm_company")]
    CrmCompany(CrmCompany),
    #[serde(rename = "reminder")]
    Reminder(Reminder),
}

impl SoupItem {
    pub fn entity_type(&self) -> EntityType {
        match self {
            SoupItem::Document(_) => EntityType::Document,
            SoupItem::Chat(_) => EntityType::Chat,
            SoupItem::Project(_) => EntityType::Project,
            SoupItem::EmailThread(_) => EntityType::EmailThread,
            SoupItem::Channel(_) => EntityType::Channel,
            SoupItem::CallRecord(_) => EntityType::CallRecord,
            SoupItem::CalendarEvent(_) => EntityType::CalendarEvent,
            SoupItem::CrmCompany(_) => EntityType::CrmCompany,
            SoupItem::Reminder(_) => EntityType::Reminder,
        }
    }

    pub fn id(&self) -> &str {
        match self {
            SoupItem::Document(d) => &d.id,
            SoupItem::Chat(c) => &c.id,
            SoupItem::Project(p) => &p.id,
            SoupItem::EmailThread(e) => &e.id,
            SoupItem::Channel(ch) => &ch.id,
            SoupItem::CallRecord(cr) => &cr.id,
            SoupItem::CalendarEvent(ce) => &ce.id,
            SoupItem::CrmCompany(cc) => &cc.id,
            SoupItem::Reminder(r) => &r.id,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            SoupItem::Document(d) => &d.name,
            SoupItem::Chat(c) => &c.name,
            SoupItem::Project(p) => &p.name,
            SoupItem::EmailThread(e) => &e.subject,
            SoupItem::Channel(ch) => &ch.name,
            SoupItem::CallRecord(_) => "Call",
            SoupItem::CalendarEvent(ce) => &ce.title,
            SoupItem::CrmCompany(cc) => &cc.name,
            SoupItem::Reminder(r) => &r.description,
        }
    }

    pub fn updated_at(&self) -> DateTime<Utc> {
        match self {
            SoupItem::Document(d) => d.updated_at,
            SoupItem::Chat(c) => c.updated_at,
            SoupItem::Project(p) => p.created_at,
            SoupItem::EmailThread(e) => e.latest_inbound_message_ts.unwrap_or(e.created_at),
            SoupItem::Channel(ch) => ch.created_at,
            SoupItem::CallRecord(cr) => cr.ended_at.unwrap_or(cr.started_at),
            SoupItem::CalendarEvent(ce) => ce.created_at,
            SoupItem::CrmCompany(cc) => cc.created_at,
            SoupItem::Reminder(r) => r.created_at,
        }
    }
}
