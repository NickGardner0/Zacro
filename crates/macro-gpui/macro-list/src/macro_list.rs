use chrono::{DateTime, Local, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TagFilter {
    pub property_definition_id: String,
    pub option_id: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TagFilterMode {
    #[default]
    Any,
    All,
}

impl TagFilterMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Any => "Any",
            Self::All => "All",
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            Self::Any => Self::All,
            Self::All => Self::Any,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListSort {
    #[default]
    UpdatedDesc,
    UpdatedAsc,
    CreatedDesc,
    CreatedAsc,
}

impl ListSort {
    pub const ALL: [Self; 4] = [
        Self::UpdatedDesc,
        Self::UpdatedAsc,
        Self::CreatedDesc,
        Self::CreatedAsc,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::UpdatedDesc => "Updated ↓",
            Self::UpdatedAsc => "Updated ↑",
            Self::CreatedDesc => "Created ↓",
            Self::CreatedAsc => "Created ↑",
        }
    }

    pub fn api_parts(self) -> (&'static str, &'static str) {
        match self {
            Self::UpdatedDesc => ("updated_at", "desc"),
            Self::UpdatedAsc => ("updated_at", "asc"),
            Self::CreatedDesc => ("created_at", "desc"),
            Self::CreatedAsc => ("created_at", "asc"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListGrouping {
    #[default]
    None,
    Date,
    EntityType,
    Project,
    TaskStatus,
    TaskPriority,
    TaskAssignee,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchScope {
    #[default]
    All,
    Documents,
    Tasks,
    Email,
    Channels,
    Calls,
    Agents,
    Folders,
}

impl SearchScope {
    pub const ALL: [Self; 8] = [
        Self::All,
        Self::Documents,
        Self::Tasks,
        Self::Email,
        Self::Channels,
        Self::Calls,
        Self::Agents,
        Self::Folders,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Documents => "Documents",
            Self::Tasks => "Tasks",
            Self::Email => "Email",
            Self::Channels => "Channels",
            Self::Calls => "Calls",
            Self::Agents => "Agents",
            Self::Folders => "Folders",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadFilter {
    #[default]
    All,
    Unread,
    Read,
}

impl ReadFilter {
    pub const ALL: [Self; 3] = [Self::All, Self::Unread, Self::Read];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Unread => "Unread",
            Self::Read => "Read",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::All => Self::Unread,
            Self::Unread => Self::Read,
            Self::Read => Self::All,
        }
    }

    pub fn matches(self, unread: bool) -> bool {
        match self {
            Self::All => true,
            Self::Unread => unread,
            Self::Read => !unread,
        }
    }
}

impl ListGrouping {
    pub const GENERAL: [Self; 4] = [Self::None, Self::Date, Self::EntityType, Self::Project];
    pub const TASKS: [Self; 7] = [
        Self::None,
        Self::Date,
        Self::EntityType,
        Self::Project,
        Self::TaskStatus,
        Self::TaskPriority,
        Self::TaskAssignee,
    ];

    pub fn available_for(view_id: &str) -> &'static [Self] {
        if view_id == "tasks" {
            &Self::TASKS
        } else {
            &Self::GENERAL
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "No grouping",
            Self::Date => "Date",
            Self::EntityType => "Type",
            Self::Project => "Project",
            Self::TaskStatus => "Status",
            Self::TaskPriority => "Priority",
            Self::TaskAssignee => "Assignee",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListQueryState {
    #[serde(default)]
    pub tags: Vec<TagFilter>,
    #[serde(default)]
    pub tag_mode: TagFilterMode,
    #[serde(default)]
    pub sort: ListSort,
    #[serde(default)]
    pub grouping: ListGrouping,
    #[serde(default)]
    pub search_text: String,
    #[serde(default)]
    pub mentioned_people: Vec<String>,
    /// Selected Macro email-link IDs for per-inbox mail views. Empty means all
    /// connected inboxes, matching the web client's filter-store contract.
    #[serde(default)]
    pub email_link_ids: Vec<String>,
    #[serde(default)]
    pub search_scope: SearchScope,
    #[serde(default)]
    pub read_filter: ReadFilter,
}

impl ListQueryState {
    pub fn inbox_default() -> Self {
        Self {
            read_filter: ReadFilter::Unread,
            ..Self::default()
        }
    }

    pub fn toggle_tag(&mut self, tag: TagFilter) {
        if let Some(index) = self.tags.iter().position(|candidate| candidate == &tag) {
            self.tags.remove(index);
        } else {
            self.tags.push(tag);
        }
    }

    pub fn is_tag_selected(&self, definition_id: &str, option_id: &str) -> bool {
        self.tags
            .iter()
            .any(|tag| tag.property_definition_id == definition_id && tag.option_id == option_id)
    }

    /// A compact, versioned query value suitable for a `view=` deep-link
    /// parameter. JSON preserves unknown future fields when the state model is
    /// extended and percent encoding keeps the containing URL unambiguous.
    pub fn to_deep_link_value(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self).map(|json| urlencoding::encode(&json).into_owned())
    }

    pub fn from_deep_link_value(value: &str) -> Result<Self, serde_json::Error> {
        let decoded = urlencoding::decode(value).unwrap_or_else(|_| value.into());
        serde_json::from_str(&decoded)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InboxTab {
    #[default]
    Signal,
    Noise,
    All,
}

impl InboxTab {
    pub const ALL: [Self; 3] = [Self::Signal, Self::Noise, Self::All];

    pub fn label(self) -> &'static str {
        match self {
            Self::Signal => "Signal",
            Self::Noise => "Noise",
            Self::All => "All",
        }
    }

    pub fn matches(self, tag: &str, raw: &Value) -> bool {
        match self {
            Self::Signal => is_signal(tag, raw),
            Self::Noise => !is_signal(tag, raw),
            Self::All => true,
        }
    }
}

/// Mirrors Macro's client-side fallback classification. The server remains the
/// authority when the soup query includes the importance predicate.
pub fn is_signal(tag: &str, raw: &Value) -> bool {
    if tag != "emailThread" {
        return !matches!(tag, "crmCompany" | "crmContact" | "automation");
    }

    let data = raw.get("data").unwrap_or(raw);
    if bool_at(data, &["isDraft", "is_draft"]).unwrap_or(false) {
        return true;
    }
    let label_tokens = label_tokens(data);
    let priority = ["CATEGORY_PERSONAL", "SENT", "IMPORTANT"]
        .iter()
        .any(|label| label_tokens.contains(*label));
    let deprioritized = [
        "CATEGORY_UPDATES",
        "CATEGORY_PROMOTIONS",
        "CATEGORY_SOCIAL",
        "CATEGORY_FORUMS",
    ]
    .iter()
    .any(|label| label_tokens.contains(*label));
    priority || !deprioritized
}

fn label_tokens(data: &Value) -> HashSet<String> {
    let labels = data
        .get("labels")
        .or_else(|| data.get("thread").and_then(|thread| thread.get("labels")))
        .and_then(Value::as_array);
    let mut tokens = HashSet::new();
    for label in labels.into_iter().flatten() {
        if let Some(value) = label.as_str() {
            tokens.insert(value.to_uppercase());
        } else {
            for key in ["id", "providerLabelId", "provider_label_id", "name"] {
                if let Some(value) = label.get(key).and_then(Value::as_str) {
                    tokens.insert(value.to_uppercase());
                }
            }
        }
    }
    tokens
}

fn bool_at(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_bool))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateGroup {
    Today,
    Yesterday,
    LastSevenDays,
    Older,
    Unknown,
}

impl DateGroup {
    pub fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::Yesterday => "Yesterday",
            Self::LastSevenDays => "Last 7 days",
            Self::Older => "Older",
            Self::Unknown => "No date",
        }
    }
}

pub fn date_group(timestamp: &str, today: NaiveDate) -> DateGroup {
    let Ok(timestamp) = DateTime::parse_from_rfc3339(timestamp) else {
        return DateGroup::Unknown;
    };
    let date = timestamp.with_timezone(&Local).date_naive();
    let age = today.signed_duration_since(date).num_days();
    match age {
        ..=-1 => DateGroup::Today,
        0 => DateGroup::Today,
        1 => DateGroup::Yesterday,
        2..=7 => DateGroup::LastSevenDays,
        _ => DateGroup::Older,
    }
}

pub fn date_group_now(timestamp: &str) -> DateGroup {
    date_group(timestamp, Utc::now().with_timezone(&Local).date_naive())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListAction {
    Open(String),
    OpenSplit(String),
    Preview(String),
    MarkDone(String),
}

#[derive(Debug, Clone, Default)]
pub struct ListInteraction {
    focused: Option<usize>,
    selected: HashSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    pub id: String,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub id: String,
    pub parent_id: Option<String>,
    pub depth: usize,
    pub has_children: bool,
    pub expanded: bool,
    pub position_in_set: usize,
    pub size_of_set: usize,
}

/// Stable interaction state for hierarchical product lists such as Files.
/// Trees begin expanded to preserve Macro's existing folder visibility; only
/// explicit user collapses are retained.
#[derive(Debug, Clone, Default)]
pub struct TreeInteraction {
    focused_id: Option<String>,
    collapsed_ids: HashSet<String>,
}

impl TreeInteraction {
    pub fn focused_id(&self) -> Option<&str> {
        self.focused_id.as_deref()
    }

    pub fn is_collapsed(&self, id: &str) -> bool {
        self.collapsed_ids.contains(id)
    }

    pub fn set_focused(&mut self, id: impl Into<String>, nodes: &[TreeNode]) {
        let id = id.into();
        if self.visible_rows(nodes).iter().any(|row| row.id == id) {
            self.focused_id = Some(id);
        }
    }

    pub fn visible_rows(&self, nodes: &[TreeNode]) -> Vec<TreeRow> {
        let known_ids = nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<HashSet<_>>();
        let mut children = std::collections::HashMap::<Option<&str>, Vec<usize>>::new();
        for (index, node) in nodes.iter().enumerate() {
            let parent = node
                .parent_id
                .as_deref()
                .filter(|parent| *parent != node.id.as_str() && known_ids.contains(*parent));
            children.entry(parent).or_default().push(index);
        }

        fn visit(
            index: usize,
            depth: usize,
            position_in_set: usize,
            size_of_set: usize,
            nodes: &[TreeNode],
            children: &std::collections::HashMap<Option<&str>, Vec<usize>>,
            collapsed_ids: &HashSet<String>,
            visited: &mut HashSet<usize>,
            rows: &mut Vec<TreeRow>,
        ) {
            if !visited.insert(index) {
                return;
            }
            let node = &nodes[index];
            let child_indexes = children
                .get(&Some(node.id.as_str()))
                .map(Vec::as_slice)
                .unwrap_or_default();
            let has_children = !child_indexes.is_empty();
            let expanded = has_children && !collapsed_ids.contains(&node.id);
            let parent_id = node.parent_id.clone().filter(|parent| {
                parent != &node.id && nodes.iter().any(|candidate| &candidate.id == parent)
            });
            rows.push(TreeRow {
                id: node.id.clone(),
                parent_id,
                depth,
                has_children,
                expanded,
                position_in_set,
                size_of_set,
            });
            if expanded {
                for (child_position, child_index) in child_indexes.iter().enumerate() {
                    visit(
                        *child_index,
                        depth + 1,
                        child_position + 1,
                        child_indexes.len(),
                        nodes,
                        children,
                        collapsed_ids,
                        visited,
                        rows,
                    );
                }
            }
        }

        let mut rows = Vec::with_capacity(nodes.len());
        let mut visited = HashSet::with_capacity(nodes.len());
        let root_indexes = children.get(&None).cloned().unwrap_or_default();
        for (position, index) in root_indexes.iter().enumerate() {
            visit(
                *index,
                0,
                position + 1,
                root_indexes.len(),
                nodes,
                &children,
                &self.collapsed_ids,
                &mut visited,
                &mut rows,
            );
        }

        // Malformed cyclic server data must remain visible and must never loop.
        // Descendants of an intentionally collapsed, already visited ancestor
        // are not malformed leftovers and must stay hidden.
        let remaining = (0..nodes.len())
            .filter(|index| {
                if visited.contains(index) {
                    return false;
                }
                let mut parent_id = nodes[*index].parent_id.as_deref();
                let mut ancestors = HashSet::new();
                while let Some(parent) = parent_id {
                    if self.collapsed_ids.contains(parent) {
                        return false;
                    }
                    if !ancestors.insert(parent) {
                        break;
                    }
                    let Some(parent_index) = nodes.iter().position(|node| node.id == parent) else {
                        break;
                    };
                    if visited.contains(&parent_index) {
                        return false;
                    }
                    parent_id = nodes[parent_index].parent_id.as_deref();
                }
                true
            })
            .collect::<Vec<_>>();
        for (position, index) in remaining.iter().enumerate() {
            visit(
                *index,
                0,
                position + 1,
                remaining.len(),
                nodes,
                &children,
                &self.collapsed_ids,
                &mut visited,
                &mut rows,
            );
        }
        rows
    }

    pub fn reconcile(&mut self, nodes: &[TreeNode]) {
        let rows = self.visible_rows(nodes);
        if self
            .focused_id
            .as_deref()
            .is_some_and(|focused| rows.iter().any(|row| row.id.as_str() == focused))
        {
            return;
        }
        self.focused_id = rows.first().map(|row| row.id.clone());
        self.collapsed_ids
            .retain(|id| nodes.iter().any(|node| &node.id == id));
    }

    pub fn move_next(&mut self, nodes: &[TreeNode]) {
        self.move_by(nodes, 1);
    }

    pub fn move_previous(&mut self, nodes: &[TreeNode]) {
        self.move_by(nodes, -1);
    }

    fn move_by(&mut self, nodes: &[TreeNode], delta: isize) {
        let rows = self.visible_rows(nodes);
        if rows.is_empty() {
            self.focused_id = None;
            return;
        }
        let current = self
            .focused_id
            .as_deref()
            .and_then(|focused| rows.iter().position(|row| row.id == focused));
        let next = match (current, delta.is_negative()) {
            (None, _) => 0,
            (Some(index), true) => index.saturating_sub(delta.unsigned_abs()),
            (Some(index), false) => (index + delta as usize).min(rows.len() - 1),
        };
        self.focused_id = Some(rows[next].id.clone());
    }

    pub fn move_home(&mut self, nodes: &[TreeNode]) {
        self.focused_id = self.visible_rows(nodes).first().map(|row| row.id.clone());
    }

    pub fn move_end(&mut self, nodes: &[TreeNode]) {
        self.focused_id = self.visible_rows(nodes).last().map(|row| row.id.clone());
    }

    pub fn expand_or_descend(&mut self, nodes: &[TreeNode]) {
        let rows = self.visible_rows(nodes);
        let Some(index) = self
            .focused_id
            .as_deref()
            .and_then(|focused| rows.iter().position(|row| row.id == focused))
        else {
            self.focused_id = rows.first().map(|row| row.id.clone());
            return;
        };
        let row = &rows[index];
        if row.has_children && !row.expanded {
            self.collapsed_ids.remove(&row.id);
        } else if row.has_children
            && let Some(child) = rows.get(index + 1).filter(|child| child.depth > row.depth)
        {
            self.focused_id = Some(child.id.clone());
        }
    }

    pub fn collapse_or_ascend(&mut self, nodes: &[TreeNode]) {
        let rows = self.visible_rows(nodes);
        let Some(row) = self
            .focused_id
            .as_deref()
            .and_then(|focused| rows.iter().find(|row| row.id.as_str() == focused))
        else {
            self.focused_id = rows.first().map(|row| row.id.clone());
            return;
        };
        if row.has_children && row.expanded {
            self.collapsed_ids.insert(row.id.clone());
        } else if let Some(parent_id) = row.parent_id.clone() {
            self.focused_id = Some(parent_id);
        }
    }

    pub fn toggle_expanded(&mut self, id: &str, nodes: &[TreeNode]) {
        let Some(row) = self
            .visible_rows(nodes)
            .into_iter()
            .find(|row| row.id == id)
        else {
            return;
        };
        if !row.has_children {
            return;
        }
        if !self.collapsed_ids.remove(id) {
            self.collapsed_ids.insert(id.to_owned());
        }
        self.focused_id = Some(id.to_owned());
    }
}

impl ListInteraction {
    pub fn focused(&self) -> Option<usize> {
        self.focused
    }

    pub fn is_selected(&self, id: &str) -> bool {
        self.selected.contains(id)
    }

    pub fn selected_ids(&self) -> impl Iterator<Item = &str> {
        self.selected.iter().map(String::as_str)
    }

    pub fn selected_count(&self) -> usize {
        self.selected.len()
    }

    pub fn clear_selected(&mut self) {
        self.selected.clear();
    }

    /// Releases keyboard ownership without disturbing the current selection.
    /// Product surfaces with more than one list (for example active calls and
    /// completed recordings) use this to return navigation to the primary
    /// collection after a transient list has been focused.
    pub fn clear_focused(&mut self) {
        self.focused = None;
    }

    pub fn set_focused(&mut self, index: usize, len: usize) {
        self.focused = (len > 0).then(|| index.min(len - 1));
    }

    pub fn move_next(&mut self, len: usize) {
        self.focused = match (self.focused, len) {
            (_, 0) => None,
            (None, _) => Some(0),
            (Some(index), _) => Some((index + 1).min(len - 1)),
        };
    }

    pub fn move_previous(&mut self, len: usize) {
        self.focused = match (self.focused, len) {
            (_, 0) => None,
            (None, _) => Some(0),
            (Some(index), _) => Some(index.saturating_sub(1)),
        };
    }

    pub fn toggle_selected(&mut self, ids: &[String]) {
        let Some(id) = self.focused.and_then(|index| ids.get(index)) else {
            return;
        };
        if !self.selected.remove(id) {
            self.selected.insert(id.clone());
        }
    }

    pub fn toggle_selected_id(&mut self, id: &str) {
        if !self.selected.remove(id) {
            self.selected.insert(id.to_owned());
        }
    }

    pub fn action(&self, ids: &[String], key: &str, shift: bool) -> Option<ListAction> {
        let id = self.focused.and_then(|index| ids.get(index))?.clone();
        match key {
            "enter" if shift => Some(ListAction::OpenSplit(id)),
            "enter" => Some(ListAction::Open(id)),
            "space" => Some(ListAction::Preview(id)),
            "e" => Some(ListAction::MarkDone(id)),
            _ => None,
        }
    }
}

/// Mirrors Macro's `unreadFilterFn` contract for the raw Soup payload kept by
/// the native normalized store. Email read state is authoritative and does not
/// fall through to notification state; every other entity is unread when at
/// least one attached notification has not been viewed or completed.
pub fn is_unread(tag: &str, raw: &Value) -> bool {
    let data = raw.get("data").unwrap_or(raw);
    if tag == "emailThread" {
        if let Some(is_read) = bool_at(data, &["isRead", "is_read"]) {
            return !is_read;
        }
        return label_tokens(data).contains("UNREAD");
    }

    if let Some(unread) = bool_at(data, &["unread", "isUnread", "is_unread"]) {
        return unread;
    }

    data.get("notifications")
        .or_else(|| raw.get("notifications"))
        .and_then(Value::as_array)
        .is_some_and(|notifications| notifications.iter().any(notification_is_unread))
}

fn notification_is_unread(notification: &Value) -> bool {
    if bool_at(notification, &["done"]).unwrap_or(false)
        || ["viewed_at", "viewedAt"]
            .iter()
            .filter_map(|key| notification.get(*key))
            .any(json_truthy)
    {
        return false;
    }

    let entity_type = notification
        .get("entity_type")
        .or_else(|| notification.get("entityType"))
        .and_then(Value::as_str);
    if entity_type == Some("channel") {
        let event_type = notification
            .pointer("/notification_metadata/tag")
            .or_else(|| notification.pointer("/notificationMetadata/tag"))
            .or_else(|| notification.get("notification_event_type"))
            .or_else(|| notification.get("notificationEventType"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        return matches!(
            event_type,
            "channel_mention"
                | "channel_message_send"
                | "channel_message_reply"
                | "document_mention"
        );
    }

    true
}

fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_macro_email_labels_like_the_web_client() {
        assert!(is_signal(
            "emailThread",
            &json!({"data": {"labels": [{"id": "CATEGORY_PERSONAL"}]}})
        ));
        assert!(!is_signal(
            "emailThread",
            &json!({"data": {"labels": [{"providerLabelId": "CATEGORY_PROMOTIONS"}]}})
        ));
        assert!(is_signal(
            "emailThread",
            &json!({"data": {"isDraft": true, "labels": ["CATEGORY_PROMOTIONS"]}})
        ));
        assert!(is_signal("document", &json!({})));
        assert!(!is_signal("crmCompany", &json!({})));
    }

    #[test]
    fn keyboard_focus_clamps_and_produces_actions() {
        let ids = vec!["a".into(), "b".into()];
        let mut state = ListInteraction::default();
        state.move_next(ids.len());
        state.move_next(ids.len());
        state.move_next(ids.len());
        assert_eq!(state.focused(), Some(1));
        assert_eq!(
            state.action(&ids, "space", false),
            Some(ListAction::Preview("b".into()))
        );
        assert_eq!(
            state.action(&ids, "enter", false),
            Some(ListAction::Open("b".into()))
        );
        assert_eq!(
            state.action(&ids, "enter", true),
            Some(ListAction::OpenSplit("b".into()))
        );
        state.toggle_selected(&ids);
        assert!(state.is_selected("b"));
        assert_eq!(state.selected_count(), 1);
        state.clear_focused();
        assert_eq!(state.focused(), None);
        assert!(state.is_selected("b"));
        state.clear_selected();
        assert_eq!(state.selected_count(), 0);
        state.move_previous(ids.len());
        assert_eq!(state.focused(), Some(0));
    }

    #[test]
    fn tree_navigation_expands_collapses_and_moves_by_stable_ids() {
        let nodes = vec![
            TreeNode {
                id: "root".into(),
                parent_id: None,
            },
            TreeNode {
                id: "child".into(),
                parent_id: Some("root".into()),
            },
            TreeNode {
                id: "grandchild".into(),
                parent_id: Some("child".into()),
            },
            TreeNode {
                id: "sibling".into(),
                parent_id: None,
            },
        ];
        let mut tree = TreeInteraction::default();
        tree.reconcile(&nodes);
        assert_eq!(tree.focused_id(), Some("root"));
        assert_eq!(
            tree.visible_rows(&nodes)
                .iter()
                .map(|row| (row.id.as_str(), row.depth))
                .collect::<Vec<_>>(),
            vec![("root", 0), ("child", 1), ("grandchild", 2), ("sibling", 0)]
        );

        tree.collapse_or_ascend(&nodes);
        assert!(tree.is_collapsed("root"));
        assert_eq!(
            tree.visible_rows(&nodes)
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            vec!["root", "sibling"]
        );
        tree.expand_or_descend(&nodes);
        assert!(!tree.is_collapsed("root"));
        tree.expand_or_descend(&nodes);
        assert_eq!(tree.focused_id(), Some("child"));
        tree.expand_or_descend(&nodes);
        assert_eq!(tree.focused_id(), Some("grandchild"));
        tree.collapse_or_ascend(&nodes);
        assert_eq!(tree.focused_id(), Some("child"));
        tree.move_end(&nodes);
        assert_eq!(tree.focused_id(), Some("sibling"));
        tree.move_home(&nodes);
        assert_eq!(tree.focused_id(), Some("root"));
    }

    #[test]
    fn malformed_tree_nodes_remain_finite_visible_and_reconcilable() {
        let nodes = vec![
            TreeNode {
                id: "orphan".into(),
                parent_id: Some("missing".into()),
            },
            TreeNode {
                id: "cycle-a".into(),
                parent_id: Some("cycle-b".into()),
            },
            TreeNode {
                id: "cycle-b".into(),
                parent_id: Some("cycle-a".into()),
            },
            TreeNode {
                id: "self".into(),
                parent_id: Some("self".into()),
            },
        ];
        let mut tree = TreeInteraction::default();
        let rows = tree.visible_rows(&nodes);
        assert_eq!(rows.len(), nodes.len());
        assert_eq!(
            rows.iter()
                .map(|row| row.id.as_str())
                .collect::<HashSet<_>>(),
            nodes
                .iter()
                .map(|node| node.id.as_str())
                .collect::<HashSet<_>>()
        );
        tree.set_focused("cycle-b", &nodes);
        assert_eq!(tree.focused_id(), Some("cycle-b"));
        tree.reconcile(&nodes[..1]);
        assert_eq!(tree.focused_id(), Some("orphan"));
    }

    #[test]
    fn groups_dates_at_product_boundaries() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 27).unwrap();
        assert_eq!(date_group("2026-08-27T12:00:00Z", today), DateGroup::Today);
        assert_eq!(
            date_group("2026-08-26T12:00:00Z", today),
            DateGroup::Yesterday
        );
        assert_eq!(
            date_group("2026-08-22T12:00:00Z", today),
            DateGroup::LastSevenDays
        );
        assert_eq!(date_group("2026-07-01T12:00:00Z", today), DateGroup::Older);
    }

    #[test]
    fn unread_contract_matches_macro_email_and_notification_semantics() {
        assert!(is_unread(
            "emailThread",
            &json!({"data": {"isRead": false, "notifications": [{"viewed_at": 1}]}})
        ));
        assert!(!is_unread(
            "emailThread",
            &json!({"data": {"isRead": true, "labels": ["UNREAD"]}})
        ));
        assert!(is_unread(
            "emailThread",
            &json!({"data": {"labels": [{"providerLabelId": "UNREAD"}]}})
        ));
        assert!(is_unread(
            "document",
            &json!({"data": {"notifications": [{"viewed_at": null, "done": false}]}})
        ));
        assert!(!is_unread(
            "document",
            &json!({"data": {"notifications": [{"viewed_at": 123}, {"done": true}]}})
        ));
        assert!(is_unread(
            "channelThread",
            &json!({"data": {"notifications": [{
                "entity_type": "channel",
                "viewed_at": null,
                "notification_metadata": {"tag": "document_mention"}
            }]}})
        ));
        assert!(!is_unread(
            "channelThread",
            &json!({"data": {"notifications": [{
                "entity_type": "channel",
                "viewed_at": null,
                "notification_metadata": {"tag": "channel_invite"}
            }]}})
        ));
        assert!(is_unread("document", &json!({"data": {"unread": true}})));
    }

    #[test]
    fn tag_filters_toggle_and_preserve_any_all_semantics() {
        let mut state = ListQueryState::default();
        let first = TagFilter {
            property_definition_id: "personal-tags".into(),
            option_id: "important".into(),
        };
        let second = TagFilter {
            property_definition_id: "team-tags".into(),
            option_id: "customer".into(),
        };
        state.toggle_tag(first.clone());
        state.toggle_tag(second);
        state.tag_mode = TagFilterMode::All;
        assert!(state.is_tag_selected("personal-tags", "important"));
        assert_eq!(state.tags.len(), 2);
        state.toggle_tag(first);
        assert_eq!(state.tags.len(), 1);
        assert_eq!(state.tag_mode.label(), "All");
    }

    #[test]
    fn list_query_state_round_trips_through_deep_link_value() {
        let state = ListQueryState {
            tags: vec![TagFilter {
                property_definition_id: "tag def".into(),
                option_id: "customer/success".into(),
            }],
            tag_mode: TagFilterMode::All,
            sort: ListSort::CreatedAsc,
            grouping: ListGrouping::EntityType,
            search_text: "from:@jane roadmap".into(),
            mentioned_people: vec!["user-1".into()],
            email_link_ids: vec!["inbox-primary".into(), "inbox-shared".into()],
            search_scope: SearchScope::Tasks,
            read_filter: ReadFilter::Read,
        };
        let encoded = state.to_deep_link_value().unwrap();
        assert!(!encoded.contains(' '));
        assert_eq!(
            ListQueryState::from_deep_link_value(&encoded).unwrap(),
            state
        );
    }

    #[test]
    fn read_filter_defaults_are_migration_safe_and_inbox_specific() {
        assert_eq!(ListQueryState::default().read_filter, ReadFilter::All);
        assert_eq!(
            ListQueryState::inbox_default().read_filter,
            ReadFilter::Unread
        );
        assert!(ReadFilter::All.matches(true));
        assert!(ReadFilter::All.matches(false));
        assert!(ReadFilter::Unread.matches(true));
        assert!(!ReadFilter::Unread.matches(false));
        assert!(ReadFilter::Read.matches(false));
        assert_eq!(ReadFilter::Read.cycle(), ReadFilter::All);
        assert_eq!(ReadFilter::ALL.len(), 3);

        let migrated: ListQueryState = serde_json::from_value(json!({
            "search_text": "legacy"
        }))
        .unwrap();
        assert_eq!(migrated.read_filter, ReadFilter::All);
    }
}
