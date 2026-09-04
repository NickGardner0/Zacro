use anyhow::{Context as _, Result};
use macro_models::EntityType;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntityRef {
    pub entity_type: EntityType,
    pub entity_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specific_message_id: Option<String>,
}

impl EntityRef {
    pub fn new(entity_type: EntityType, entity_id: impl Into<String>) -> Self {
        Self {
            entity_type,
            entity_id: entity_id.into(),
            specific_message_id: None,
        }
    }

    pub fn with_message_id(mut self, message_id: impl Into<String>) -> Self {
        self.specific_message_id = Some(message_id.into());
        self
    }

    pub fn stable_key(&self) -> String {
        match &self.specific_message_id {
            Some(message_id) => format!("{}:{}:{message_id}", self.entity_type, self.entity_id),
            None => format!("{}:{}", self.entity_type, self.entity_id),
        }
    }
}

/// A persisted bidirectional entity link. `source` owns the mention and
/// `target` is the referenced entity. The backend mention id is the stable
/// mutation identity used for removal and realtime reconciliation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityMention {
    pub id: String,
    pub source: EntityRef,
    pub target: EntityRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

/// Normalized edge index shared by mentions, backlinks, canvases, editor
/// pills, and entity property references. Updating an existing mention first
/// removes its old indexes so a realtime replacement cannot leave ghost edges.
#[derive(Debug, Clone, Default)]
pub struct EntityGraph {
    mentions: HashMap<String, EntityMention>,
    outgoing: HashMap<EntityRef, HashSet<String>>,
    incoming: HashMap<EntityRef, HashSet<String>>,
}

impl EntityGraph {
    pub fn upsert(&mut self, mention: EntityMention) {
        self.remove(&mention.id);
        self.outgoing
            .entry(mention.source.clone())
            .or_default()
            .insert(mention.id.clone());
        self.incoming
            .entry(mention.target.clone())
            .or_default()
            .insert(mention.id.clone());
        self.mentions.insert(mention.id.clone(), mention);
    }

    pub fn remove(&mut self, mention_id: &str) -> Option<EntityMention> {
        let mention = self.mentions.remove(mention_id)?;
        remove_edge_index(&mut self.outgoing, &mention.source, mention_id);
        remove_edge_index(&mut self.incoming, &mention.target, mention_id);
        Some(mention)
    }

    pub fn get(&self, mention_id: &str) -> Option<&EntityMention> {
        self.mentions.get(mention_id)
    }

    pub fn outgoing(&self, source: &EntityRef) -> Vec<&EntityMention> {
        self.indexed_mentions(&self.outgoing, source)
    }

    pub fn incoming(&self, target: &EntityRef) -> Vec<&EntityMention> {
        self.indexed_mentions(&self.incoming, target)
    }

    pub fn contains_edge(&self, source: &EntityRef, target: &EntityRef) -> bool {
        self.outgoing(source)
            .into_iter()
            .any(|mention| &mention.target == target)
    }

    pub fn len(&self) -> usize {
        self.mentions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.mentions.is_empty()
    }

    fn indexed_mentions<'a>(
        &'a self,
        index: &'a HashMap<EntityRef, HashSet<String>>,
        entity: &EntityRef,
    ) -> Vec<&'a EntityMention> {
        let mut mentions = index
            .get(entity)
            .into_iter()
            .flatten()
            .filter_map(|mention_id| self.mentions.get(mention_id))
            .collect::<Vec<_>>();
        mentions.sort_by(|left, right| left.id.cmp(&right.id));
        mentions
    }
}

fn remove_edge_index(
    index: &mut HashMap<EntityRef, HashSet<String>>,
    entity: &EntityRef,
    mention_id: &str,
) {
    if let Some(ids) = index.get_mut(entity) {
        ids.remove(mention_id);
        if ids.is_empty() {
            index.remove(entity);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Favorite {
    pub entity_id: String,
    pub entity_type: EntityType,
    pub file_type: Option<String>,
    pub sort_order: i64,
}

pub fn parse_favorites(value: &Value) -> Result<Vec<Favorite>> {
    let favorites = value
        .get("favorites")
        .and_then(Value::as_array)
        .context("favorites response missing favorites")?;
    favorites
        .iter()
        .map(|favorite| {
            Ok(Favorite {
                entity_id: favorite
                    .get("entityId")
                    .and_then(Value::as_str)
                    .context("favorite missing entityId")?
                    .to_owned(),
                entity_type: entity_type_from_favorite_wire(
                    favorite
                        .get("entityType")
                        .and_then(Value::as_str)
                        .context("favorite missing entityType")?,
                ),
                file_type: favorite
                    .get("fileType")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                sort_order: favorite
                    .get("sortOrder")
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessLevel {
    View,
    Comment,
    Edit,
    Owner,
}

impl AccessLevel {
    pub fn rank(self) -> u8 {
        match self {
            Self::View => 0,
            Self::Comment => 1,
            Self::Edit => 2,
            Self::Owner => 3,
        }
    }

    pub fn strongest(self, other: Self) -> Self {
        if self.rank() >= other.rank() {
            self
        } else {
            other
        }
    }

    pub fn can_edit(self) -> bool {
        matches!(self, Self::Edit | Self::Owner)
    }

    pub fn can_comment(self) -> bool {
        matches!(self, Self::Comment | Self::Edit | Self::Owner)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum LinkShare {
    Public,
    Team,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum PermissionSubject {
    User(String),
    Team(String),
    Channel(String),
    Public,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionGrant {
    pub subject: PermissionSubject,
    pub access_level: AccessLevel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharePolicy {
    pub id: String,
    pub owner_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_share: Option<LinkShare>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_access_level: Option<AccessLevel>,
    #[serde(default)]
    pub grants: Vec<PermissionGrant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionContext<'a> {
    pub user_id: &'a str,
    pub team_ids: &'a HashSet<String>,
    pub channel_ids: &'a HashSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveAccess {
    pub access_level: AccessLevel,
    pub sources: Vec<PermissionSubject>,
}

impl SharePolicy {
    /// Resolves direct, team-derived, channel-derived and link access without
    /// discarding the contributing sources. The source list lets a share dialog
    /// explain why a permission cannot be removed at the current entity.
    pub fn effective_access(&self, context: &PermissionContext<'_>) -> Option<EffectiveAccess> {
        if context.user_id == self.owner_id {
            return Some(EffectiveAccess {
                access_level: AccessLevel::Owner,
                sources: vec![PermissionSubject::User(self.owner_id.clone())],
            });
        }

        let mut effective = None::<AccessLevel>;
        let mut sources = Vec::new();
        for grant in &self.grants {
            let applies = match &grant.subject {
                PermissionSubject::User(user_id) => user_id == context.user_id,
                PermissionSubject::Team(team_id) => context.team_ids.contains(team_id),
                PermissionSubject::Channel(channel_id) => context.channel_ids.contains(channel_id),
                PermissionSubject::Public => true,
            };
            if applies {
                effective = Some(
                    effective
                        .map(|current| current.strongest(grant.access_level))
                        .unwrap_or(grant.access_level),
                );
                sources.push(grant.subject.clone());
            }
        }

        if let (Some(link_share), Some(access_level)) = (self.link_share, self.link_access_level) {
            let applies = match link_share {
                LinkShare::Public => true,
                LinkShare::Team => !context.team_ids.is_empty(),
            };
            if applies {
                effective = Some(
                    effective
                        .map(|current| current.strongest(access_level))
                        .unwrap_or(access_level),
                );
                sources.push(match link_share {
                    LinkShare::Public => PermissionSubject::Public,
                    LinkShare::Team => PermissionSubject::Team("link".into()),
                });
            }
        }

        effective.map(|access_level| EffectiveAccess {
            access_level,
            sources,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityPermission {
    NoAccess,
    AccessLevel(AccessLevel),
    ChannelViewOnly,
    ChannelRole(String),
    TeamRole(String),
}

impl EntityPermission {
    pub fn can_view(&self) -> bool {
        !matches!(self, Self::NoAccess)
    }

    pub fn can_edit(&self) -> bool {
        match self {
            Self::AccessLevel(level) => level.can_edit(),
            Self::ChannelRole(role) => matches!(role.as_str(), "owner" | "admin" | "member"),
            Self::TeamRole(role) => matches!(role.as_str(), "owner" | "admin" | "member"),
            Self::NoAccess | Self::ChannelViewOnly => false,
        }
    }

    pub fn can_comment(&self) -> bool {
        match self {
            Self::AccessLevel(level) => level.can_comment(),
            Self::ChannelRole(role) => matches!(role.as_str(), "owner" | "admin" | "member"),
            Self::TeamRole(role) => matches!(role.as_str(), "owner" | "admin" | "member"),
            Self::NoAccess | Self::ChannelViewOnly => false,
        }
    }
}

pub fn parse_permission(value: &Value) -> Result<EntityPermission> {
    match value.get("status").and_then(Value::as_str) {
        Some("no_access") => Ok(EntityPermission::NoAccess),
        Some("access") => {
            let permission = value
                .get("permission")
                .context("permission response missing permission")?;
            match permission.get("type").and_then(Value::as_str) {
                Some("access_level") => {
                    let level = match permission.get("access_level").and_then(Value::as_str) {
                        Some("view") => AccessLevel::View,
                        Some("comment") => AccessLevel::Comment,
                        Some("edit") => AccessLevel::Edit,
                        Some("owner") => AccessLevel::Owner,
                        _ => anyhow::bail!("permission response had an unknown access level"),
                    };
                    Ok(EntityPermission::AccessLevel(level))
                }
                Some("channel_view_only") => Ok(EntityPermission::ChannelViewOnly),
                Some("channel_role") => Ok(EntityPermission::ChannelRole(
                    permission
                        .get("role")
                        .and_then(Value::as_str)
                        .context("channel role permission missing role")?
                        .to_owned(),
                )),
                Some("team_role") => Ok(EntityPermission::TeamRole(
                    permission
                        .get("role")
                        .and_then(Value::as_str)
                        .context("team role permission missing role")?
                        .to_owned(),
                )),
                _ => anyhow::bail!("permission response had an unknown permission type"),
            }
        }
        _ => anyhow::bail!("permission response had an unknown status"),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PropertyOption {
    pub id: String,
    pub label: String,
    pub color: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    Empty,
    Boolean(bool),
    Number(f64),
    String(String),
    Date(String),
    SelectOptions(Vec<String>),
    EntityReferences(Vec<EntityRef>),
    Links(Vec<String>),
    Unknown(Value),
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntityProperty {
    pub entity_property_id: String,
    pub definition_id: String,
    pub display_name: String,
    pub data_type: String,
    pub is_metadata: bool,
    pub is_system: bool,
    pub is_multi_select: bool,
    pub value: PropertyValue,
    pub options: Vec<PropertyOption>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntityProperties {
    pub entity_id: String,
    pub properties: Vec<EntityProperty>,
}

pub fn parse_entity_properties(value: &Value) -> Result<EntityProperties> {
    let entity_id = value
        .get("entity_id")
        .and_then(Value::as_str)
        .context("entity properties missing entity_id")?
        .to_owned();
    let properties = value
        .get("properties")
        .and_then(Value::as_array)
        .context("entity properties missing properties")?
        .iter()
        .map(parse_property)
        .collect::<Result<Vec<_>>>()?;
    Ok(EntityProperties {
        entity_id,
        properties,
    })
}

fn parse_property(value: &Value) -> Result<EntityProperty> {
    let definition = value
        .get("definition")
        .context("entity property missing definition")?;
    let property = value
        .get("property")
        .context("entity property missing assignment")?;
    let options = value
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|option| {
            let option_value = option.get("value")?;
            let label = option_value.get("value").map(|value| match value {
                Value::String(value) => value.clone(),
                other => other.to_string(),
            })?;
            Some(PropertyOption {
                id: option.get("id")?.as_str()?.to_owned(),
                label,
                color: option
                    .get("color")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
            })
        })
        .collect();
    Ok(EntityProperty {
        entity_property_id: property
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        definition_id: definition
            .get("id")
            .and_then(Value::as_str)
            .context("property definition missing id")?
            .to_owned(),
        display_name: definition
            .get("display_name")
            .and_then(Value::as_str)
            .unwrap_or("Property")
            .to_owned(),
        data_type: definition
            .get("data_type")
            .and_then(Value::as_str)
            .unwrap_or("STRING")
            .to_owned(),
        is_metadata: definition
            .get("is_metadata")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        is_system: definition
            .get("is_system")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        is_multi_select: definition
            .get("is_multi_select")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        value: parse_property_value(value.get("value")),
        options,
    })
}

fn parse_property_value(value: Option<&Value>) -> PropertyValue {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return PropertyValue::Empty;
    };
    let Some(kind) = value.get("type").and_then(Value::as_str) else {
        return PropertyValue::Unknown(value.clone());
    };
    let payload = value.get("value").unwrap_or(&Value::Null);
    match kind {
        "Boolean" => payload
            .as_bool()
            .map(PropertyValue::Boolean)
            .unwrap_or_else(|| PropertyValue::Unknown(value.clone())),
        "Number" => payload
            .as_f64()
            .map(PropertyValue::Number)
            .unwrap_or_else(|| PropertyValue::Unknown(value.clone())),
        "String" => payload
            .as_str()
            .map(|value| PropertyValue::String(value.to_owned()))
            .unwrap_or_else(|| PropertyValue::Unknown(value.clone())),
        "Date" => payload
            .as_str()
            .map(|value| PropertyValue::Date(value.to_owned()))
            .unwrap_or_else(|| PropertyValue::Unknown(value.clone())),
        "SelectOption" => PropertyValue::SelectOptions(string_array(payload)),
        "Link" => PropertyValue::Links(string_array(payload)),
        "EntityReference" => PropertyValue::EntityReferences(
            payload
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|reference| {
                    Some(EntityRef {
                        entity_type: entity_type_from_property_wire(
                            reference.get("entity_type")?.as_str()?,
                        ),
                        entity_id: reference.get("entity_id")?.as_str()?.to_owned(),
                        specific_message_id: reference
                            .get("specific_message_id")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned),
                    })
                })
                .collect(),
        ),
        _ => PropertyValue::Unknown(value.clone()),
    }
}

fn string_array(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

pub fn property_target_wire_type(entity_type: EntityType) -> Option<&'static str> {
    match entity_type {
        EntityType::Call | EntityType::CallRecord => Some("CALL_RECORD"),
        EntityType::Channel => Some("CHANNEL"),
        EntityType::Chat => Some("CHAT"),
        EntityType::CrmCompany => Some("COMPANY"),
        EntityType::Document | EntityType::Skill => Some("DOCUMENT"),
        EntityType::Project => Some("PROJECT"),
        // The properties service uses THREAD for both channel threads and
        // email threads. Macro's web client makes the same email -> THREAD
        // conversion for tags and key properties.
        EntityType::ChannelMessage | EntityType::EmailThread | EntityType::EmailMessage => {
            Some("THREAD")
        }
        EntityType::User => Some("USER"),
        _ => None,
    }
}

pub fn permission_target_wire_type(entity_type: EntityType) -> Option<&'static str> {
    match entity_type {
        EntityType::Document | EntityType::Skill => Some("document"),
        EntityType::Chat => Some("chat"),
        EntityType::Project => Some("project"),
        EntityType::ChannelMessage => Some("thread"),
        EntityType::EmailThread | EntityType::EmailMessage => Some("email_thread"),
        EntityType::Channel => Some("channel"),
        _ => None,
    }
}

pub fn favorite_target_wire_type(entity_type: EntityType) -> Option<&'static str> {
    match entity_type {
        EntityType::User => Some("user"),
        EntityType::Chat => Some("chat"),
        EntityType::Channel => Some("channel"),
        EntityType::ChannelMessage => Some("channel_message"),
        EntityType::Document => Some("document"),
        EntityType::Project => Some("project"),
        EntityType::EmailThread | EntityType::EmailMessage => Some("email_thread"),
        EntityType::CalendarEvent => Some("calendar_event"),
        EntityType::Team => Some("team"),
        EntityType::Call | EntityType::CallRecord => Some("call"),
        EntityType::ForeignEntity => Some("foreign_entity"),
        EntityType::StaticFile => Some("static_file"),
        EntityType::CrmCompany => Some("crm_company"),
        EntityType::CrmContact => Some("crm_contact"),
        EntityType::Reminder => Some("reminder"),
        EntityType::Skill => Some("skill"),
        EntityType::AgentSession => Some("agent_session"),
    }
}

pub fn reference_target_wire_type(entity_type: EntityType) -> Option<&'static str> {
    match entity_type {
        EntityType::Document | EntityType::Skill => Some("document"),
        EntityType::Chat | EntityType::AgentSession => Some("chat"),
        EntityType::Project => Some("project"),
        EntityType::Channel => Some("channel"),
        EntityType::ChannelMessage => Some("channel_message"),
        EntityType::EmailThread | EntityType::EmailMessage => Some("email"),
        EntityType::Call | EntityType::CallRecord => Some("call"),
        EntityType::CalendarEvent => Some("calendar_event"),
        EntityType::ForeignEntity => Some("foreign"),
        EntityType::CrmCompany => Some("crm_company"),
        EntityType::CrmContact => Some("crm_contact"),
        EntityType::User | EntityType::Team | EntityType::StaticFile | EntityType::Reminder => None,
    }
}

fn entity_type_from_favorite_wire(value: &str) -> EntityType {
    match value {
        "user" => EntityType::User,
        "chat" => EntityType::Chat,
        "channel" => EntityType::Channel,
        "channel_message" => EntityType::ChannelMessage,
        "document" => EntityType::Document,
        "project" => EntityType::Project,
        "email_thread" => EntityType::EmailThread,
        "calendar_event" => EntityType::CalendarEvent,
        "team" => EntityType::Team,
        "call" => EntityType::CallRecord,
        "static_file" => EntityType::StaticFile,
        "crm_company" => EntityType::CrmCompany,
        "crm_contact" => EntityType::CrmContact,
        "reminder" => EntityType::Reminder,
        "skill" => EntityType::Skill,
        "agent_session" => EntityType::AgentSession,
        _ => EntityType::ForeignEntity,
    }
}

fn entity_type_from_property_wire(value: &str) -> EntityType {
    match value {
        "CALL_RECORD" => EntityType::CallRecord,
        "CHANNEL" => EntityType::Channel,
        "CHAT" => EntityType::Chat,
        "COMPANY" => EntityType::CrmCompany,
        "DOCUMENT" | "TASK" => EntityType::Document,
        "PROJECT" => EntityType::Project,
        "THREAD" => EntityType::ChannelMessage,
        "USER" => EntityType::User,
        "CALENDAR_EVENT" => EntityType::CalendarEvent,
        _ => EntityType::ForeignEntity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn permissions_preserve_edit_semantics() {
        let edit = parse_permission(&json!({
            "status": "access",
            "permission": {"type": "access_level", "access_level": "edit"}
        }))
        .unwrap();
        assert!(edit.can_view());
        assert!(edit.can_edit());
        assert!(
            !parse_permission(&json!({"status": "no_access"}))
                .unwrap()
                .can_view()
        );
    }

    #[test]
    fn entity_graph_keeps_forward_and_backlink_indexes_consistent() {
        let source = EntityRef::new(EntityType::Document, "doc-1");
        let first_target = EntityRef::new(EntityType::Channel, "channel-1");
        let second_target = EntityRef::new(EntityType::CrmCompany, "company-1");
        let mut graph = EntityGraph::default();
        graph.upsert(EntityMention {
            id: "mention-1".into(),
            source: source.clone(),
            target: first_target.clone(),
            user_id: Some("user-1".into()),
            created_at: None,
        });
        assert!(graph.contains_edge(&source, &first_target));
        assert_eq!(graph.incoming(&first_target).len(), 1);

        graph.upsert(EntityMention {
            id: "mention-1".into(),
            source: source.clone(),
            target: second_target.clone(),
            user_id: Some("user-1".into()),
            created_at: None,
        });
        assert!(!graph.contains_edge(&source, &first_target));
        assert!(graph.incoming(&first_target).is_empty());
        assert!(graph.contains_edge(&source, &second_target));
        assert_eq!(graph.len(), 1);

        graph.remove("mention-1");
        assert!(graph.is_empty());
        assert!(graph.outgoing(&source).is_empty());
        assert!(graph.incoming(&second_target).is_empty());
    }

    #[test]
    fn permission_policy_combines_direct_and_derived_access_with_sources() {
        let teams = HashSet::from(["team-1".to_owned()]);
        let channels = HashSet::from(["channel-1".to_owned()]);
        let policy = SharePolicy {
            id: "share-1".into(),
            owner_id: "owner-1".into(),
            link_share: Some(LinkShare::Team),
            link_access_level: Some(AccessLevel::View),
            grants: vec![
                PermissionGrant {
                    subject: PermissionSubject::Channel("channel-1".into()),
                    access_level: AccessLevel::Comment,
                },
                PermissionGrant {
                    subject: PermissionSubject::User("user-1".into()),
                    access_level: AccessLevel::Edit,
                },
            ],
        };
        let effective = policy
            .effective_access(&PermissionContext {
                user_id: "user-1",
                team_ids: &teams,
                channel_ids: &channels,
            })
            .unwrap();
        assert_eq!(effective.access_level, AccessLevel::Edit);
        assert_eq!(effective.sources.len(), 3);

        let owner = policy
            .effective_access(&PermissionContext {
                user_id: "owner-1",
                team_ids: &HashSet::new(),
                channel_ids: &HashSet::new(),
            })
            .unwrap();
        assert_eq!(owner.access_level, AccessLevel::Owner);
    }

    #[test]
    fn entity_properties_keep_typed_values_and_options() {
        let parsed = parse_entity_properties(&json!({
            "entity_id": "doc-1",
            "properties": [{
                "property": {"id": "assignment-1"},
                "definition": {
                    "id": "status", "display_name": "Status", "data_type": "SELECT_STRING",
                    "is_metadata": false, "is_system": true, "is_multi_select": false
                },
                "value": {"type": "SelectOption", "value": ["open"]},
                "options": [{
                    "id": "open", "color": "blue", "value": {"type": "string", "value": "Open"}
                }]
            }]
        }))
        .unwrap();
        assert_eq!(parsed.properties[0].display_name, "Status");
        assert_eq!(parsed.properties[0].options[0].label, "Open");
        assert_eq!(
            parsed.properties[0].value,
            PropertyValue::SelectOptions(vec!["open".into()])
        );
    }

    #[test]
    fn wire_type_mappings_are_domain_specific() {
        assert_eq!(
            permission_target_wire_type(EntityType::EmailThread),
            Some("email_thread")
        );
        assert_eq!(
            property_target_wire_type(EntityType::EmailThread),
            Some("THREAD")
        );
        assert_eq!(
            favorite_target_wire_type(EntityType::EmailThread),
            Some("email_thread")
        );
        assert_eq!(
            reference_target_wire_type(EntityType::EmailThread),
            Some("email")
        );
        assert_eq!(
            reference_target_wire_type(EntityType::CrmCompany),
            Some("crm_company")
        );
    }

    #[test]
    fn favorites_preserve_manual_order_metadata() {
        let favorites = parse_favorites(&json!({
            "favorites": [{
                "entityId": "mail-1",
                "entityType": "email_thread",
                "sortOrder": 7
            }]
        }))
        .unwrap();
        assert_eq!(favorites[0].entity_type, EntityType::EmailThread);
        assert_eq!(favorites[0].sort_order, 7);
    }
}
