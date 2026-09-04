//! Loro-backed document state. The implementation lives here so editor views
//! never need to know about snapshots, version vectors, or peer identities.

use anyhow::{Context as _, Result, bail};
use loro::{
    Container, ContainerTrait, ExportMode, LocalUpdateCallback, LoroDoc, LoroList, LoroMap,
    LoroMovableList, LoroText, LoroValue, Subscription, ValueOrContainer, VersionVector,
    cursor::{Cursor, Side},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    /// A non-empty dependency range means the caller must request updates from
    /// the server using its current version vector before this change can be
    /// fully applied.
    pub has_missing_dependencies: bool,
}

/// The byte-compatible CRDT core shared by documents, tasks, snippets, agent
/// instructions, and rich messaging surfaces. It deliberately exposes the
/// complete deep value instead of projecting only nodes known to this client;
/// exporting after an import therefore preserves future/unknown node kinds.
#[derive(Debug, Clone)]
pub struct CollaborativeDocument {
    document: LoroDoc,
}

impl CollaborativeDocument {
    pub fn new(peer_id: Option<u64>) -> Result<Self> {
        let document = LoroDoc::new();
        document.set_record_timestamp(true);
        if let Some(peer_id) = peer_id {
            document
                .set_peer_id(peer_id)
                .context("failed to set Loro peer identity")?;
        }
        Ok(Self { document })
    }

    pub fn from_snapshot(snapshot: &[u8], peer_id: Option<u64>) -> Result<Self> {
        let this = Self::new(peer_id)?;
        this.import_remote(snapshot)?;
        Ok(this)
    }

    /// Restore the locally cached shallow snapshot and replay every durable
    /// WAL update over it before the websocket starts. Loro retains causally
    /// ahead operations, so an update whose dependency is only on the server
    /// is still safe to import here and will become visible after catch-up.
    pub fn from_cached_state(
        snapshot: Option<&[u8]>,
        wal_updates: &[Vec<u8>],
        peer_id: Option<u64>,
    ) -> Result<Self> {
        let this = Self::new(peer_id)?;
        if let Some(snapshot) = snapshot {
            this.import_remote(snapshot)
                .context("failed to import cached Macro collaboration snapshot")?;
        }
        if !wal_updates.is_empty() {
            this.import_remote_batch(wal_updates)
                .context("failed to replay Macro collaboration WAL")?;
        }
        Ok(this)
    }

    pub fn peer_id(&self) -> u64 {
        self.document.peer_id()
    }

    pub fn set_peer_id(&self, peer_id: u64) -> Result<()> {
        self.document
            .set_peer_id(peer_id)
            .context("failed to replace Loro peer identity")
    }

    pub fn import_remote(&self, update_or_snapshot: &[u8]) -> Result<ImportReport> {
        let status = self
            .document
            .import_with(update_or_snapshot, "macro-sync-remote")
            .context("failed to import Loro update or snapshot")?;
        Ok(ImportReport {
            has_missing_dependencies: status.pending.is_some(),
        })
    }

    pub fn import_remote_batch(&self, updates: &[Vec<u8>]) -> Result<ImportReport> {
        let status = self
            .document
            .import_batch(updates)
            .context("failed to import Loro update batch")?;
        Ok(ImportReport {
            has_missing_dependencies: status.pending.is_some(),
        })
    }

    pub fn export_snapshot(&self) -> Result<Vec<u8>> {
        self.document
            .export(ExportMode::Snapshot)
            .context("failed to export Loro snapshot")
    }

    /// Export the compact, state-bearing snapshot Macro writes to its local
    /// cache. The current frontiers become the shallow root, bounding cold
    /// start storage while the server remains the authoritative history store.
    pub fn export_shallow_snapshot(&self) -> Result<Vec<u8>> {
        let frontiers = self.document.oplog_frontiers();
        self.document
            .export(ExportMode::shallow_snapshot(&frontiers))
            .context("failed to export compact Loro snapshot")
    }

    pub fn export_all_updates(&self) -> Result<Vec<u8>> {
        self.document
            .export(ExportMode::all_updates())
            .context("failed to export complete Loro update history")
    }

    pub fn export_updates_since(&self, encoded_version_vector: &[u8]) -> Result<Vec<u8>> {
        let version_vector = VersionVector::decode(encoded_version_vector)
            .context("failed to decode Loro version vector")?;
        self.document
            .export(ExportMode::updates(&version_vector))
            .context("failed to export incremental Loro updates")
    }

    pub fn encoded_version_vector(&self) -> Vec<u8> {
        self.document.oplog_vv().encode()
    }

    pub fn deep_json(&self) -> Result<serde_json::Value> {
        serde_json::to_value(self.document.get_deep_value())
            .context("failed to serialize Loro document value")
    }

    /// Subscribe to locally committed update bytes. Remote imports do not flow
    /// through this callback, preventing echo loops.
    pub fn subscribe_local_updates(&self, callback: LocalUpdateCallback) -> Subscription {
        self.document.subscribe_local_update(callback)
    }

    pub fn commit(&self) {
        self.document.commit();
    }

    pub fn compact(&self) {
        self.document.compact_change_store();
    }

    pub fn loro(&self) -> &LoroDoc {
        &self.document
    }

    /// Create the byte representation used by Macro's Lexical awareness codec
    /// for a stable text-node ID and Unicode offset.
    pub fn encode_text_cursor(&self, node_id: &str, unicode_offset: usize) -> Result<Vec<u8>> {
        let text =
            find_text_container(&self.document.get_map("root"), node_id).with_context(|| {
                format!("Lexical text node {node_id} is not present in the Loro document")
            })?;
        let offset = unicode_offset.min(text.len_unicode());
        let cursor = text
            .get_cursor(offset, Side::Middle)
            .context("failed to create Loro text cursor")?;
        Ok(cursor.encode())
    }

    /// Decode a Macro awareness cursor and resolve its current Unicode offset.
    /// The cursor's container must still belong to the supplied Lexical node,
    /// preventing a malformed presence packet from highlighting unrelated text.
    pub fn resolve_text_cursor(&self, node_id: &str, encoded_cursor: &[u8]) -> Result<usize> {
        let cursor = Cursor::decode(encoded_cursor).context("failed to decode Loro text cursor")?;
        let text =
            find_text_container(&self.document.get_map("root"), node_id).with_context(|| {
                format!("Lexical text node {node_id} is not present in the Loro document")
            })?;
        if text.id() != cursor.container {
            bail!("awareness cursor container does not match Lexical node {node_id}");
        }
        self.document
            .get_cursor_pos(&cursor)
            .map(|position| position.current.pos.min(text.len_unicode()))
            .context("failed to resolve Loro text cursor")
    }

    /// Validate that a snapshot carries Macro's recursive markdown root. This
    /// is intentionally non-destructive: unknown attributes and node types are
    /// legal, while absence of the root map indicates a different collab
    /// surface schema and must not be edited as a document.
    pub fn validate_markdown_schema(&self) -> Result<()> {
        let value = self.deep_json()?;
        let Some(root) = value.get("root").and_then(serde_json::Value::as_object) else {
            bail!("Loro snapshot does not contain Macro's markdown root map");
        };
        if !root.contains_key("$") && !root.contains_key("children") {
            bail!("Macro markdown root is missing metadata and children");
        }
        Ok(())
    }

    /// Apply a serialized Macro Lexical tree as local Loro operations and
    /// return the incremental update that must be queued durably. Containers
    /// for node kinds unknown to this client are never removed or replaced.
    pub fn replace_markdown_state(&self, state: &serde_json::Value) -> Result<Vec<u8>> {
        self.replace_markdown_state_with_policy(state, false)
    }

    /// Apply Macro's exact `comment-mark` wrapper through a narrowly scoped
    /// annotation path. Ordinary Markdown projection does not authorize this
    /// node, keeping unknown-node losslessness checks intact.
    pub fn replace_annotation_state(&self, state: &serde_json::Value) -> Result<Vec<u8>> {
        self.replace_markdown_state_with_policy(state, true)
    }

    fn replace_markdown_state_with_policy(
        &self,
        state: &serde_json::Value,
        allow_comment_marks: bool,
    ) -> Result<Vec<u8>> {
        self.validate_markdown_schema()?;
        let incoming = state
            .get("root")
            .and_then(|root| root.get("children"))
            .and_then(serde_json::Value::as_array)
            .context("serialized Lexical state is missing root.children")?;
        let root = self.document.get_map("root");
        let children = root
            .get("children")
            .and_then(into_movable_list)
            .context("Macro markdown root.children is not a Loro movable list")?;
        let version_before = self.encoded_version_vector();
        let existing = movable_list_nodes(&children, allow_comment_marks)?;
        // Full preflight happens before root metadata or any list operation is
        // touched. Protected containers are matched by stable Lexical node ID
        // (or exact value for old snapshots), and omitted protected nodes are
        // woven back in for compatibility with older projections.
        let target = prepare_markdown_target(&existing, incoming, allow_comment_marks)?;
        apply_supported_root_state(&root, state.get("root").unwrap_or(state))?;
        for (target_index, node) in target.iter().enumerate() {
            if let Some(current_index) = find_current_node(&children, node)? {
                if current_index != target_index {
                    children
                        .mov(current_index, target_index)
                        .context("failed to reorder Macro Lexical block")?;
                }
                let current = children
                    .get(target_index)
                    .and_then(into_map)
                    .map(|map| {
                        let value = serde_json::to_value(map.get_deep_value())?;
                        Ok::<_, anyhow::Error>((map, value))
                    })
                    .transpose()?
                    .context("Macro Lexical child is not a map")?;
                if &current.1 != node {
                    let current_kind = current
                        .1
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("unknown");
                    let target_kind = node
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("unknown");
                    if current_kind == target_kind
                        && is_known_lexical_node(target_kind)
                        && !contains_unsupported_descendant(&current.1, allow_comment_marks)
                    {
                        reconcile_rewritable_node(&current.0, node, allow_comment_marks)?;
                    } else {
                        children
                            .set_container(target_index, build_loro_map(node)?)
                            .context("failed to replace Macro Lexical block")?;
                    }
                }
            } else {
                children
                    .insert_container(target_index, build_loro_map(node)?)
                    .context("failed to insert Macro Lexical block")?;
            }
        }
        while children.len() > target.len() {
            let slot = children.len() - 1;
            children
                .delete(slot, 1)
                .context("failed to remove obsolete Macro Lexical block")?;
        }
        self.commit();
        self.export_updates_since(&version_before)
    }
}

fn find_text_container(node: &LoroMap, node_id: &str) -> Option<LoroText> {
    let matches_id = node
        .get("$")
        .and_then(into_map)
        .and_then(|metadata| metadata.get("id"))
        .and_then(|id| match id {
            ValueOrContainer::Value(LoroValue::String(id)) => Some(id.to_string()),
            _ => None,
        })
        .is_some_and(|id| id == node_id);
    if matches_id && let Some(text) = node.get("text").and_then(into_text) {
        return Some(text);
    }
    match node.get("children")? {
        ValueOrContainer::Container(Container::List(children)) => {
            (0..children.len()).find_map(|index| {
                children
                    .get(index)
                    .and_then(into_map)
                    .and_then(|child| find_text_container(&child, node_id))
            })
        }
        ValueOrContainer::Container(Container::MovableList(children)) => (0..children.len())
            .find_map(|index| {
                children
                    .get(index)
                    .and_then(into_map)
                    .and_then(|child| find_text_container(&child, node_id))
            }),
        _ => None,
    }
}

fn apply_supported_root_state(root: &LoroMap, incoming_root: &serde_json::Value) -> Result<()> {
    let Some(incoming_metadata) = incoming_root
        .get("$")
        .and_then(serde_json::Value::as_object)
    else {
        return Ok(());
    };
    let Some(pinned_property_ids) = incoming_metadata.get("pinnedPropertyIds") else {
        return Ok(());
    };
    let metadata = match root.get("$").and_then(into_map) {
        Some(metadata) => metadata,
        None => root
            .insert_container("$", LoroMap::new())
            .context("failed to create Macro root node-state metadata")?,
    };
    metadata
        .insert(
            "pinnedPropertyIds",
            LoroValue::from(pinned_property_ids.clone()),
        )
        .context("failed to update Macro pinned property node state")?;
    Ok(())
}

fn into_map(value: ValueOrContainer) -> Option<LoroMap> {
    match value {
        ValueOrContainer::Container(Container::Map(map)) => Some(map),
        _ => None,
    }
}

fn into_movable_list(value: ValueOrContainer) -> Option<LoroMovableList> {
    match value {
        ValueOrContainer::Container(Container::MovableList(list)) => Some(list),
        _ => None,
    }
}

fn into_text(value: ValueOrContainer) -> Option<LoroText> {
    match value {
        ValueOrContainer::Container(Container::Text(text)) => Some(text),
        _ => None,
    }
}

fn map_node_type(map: &LoroMap) -> Option<String> {
    match map.get("type")? {
        ValueOrContainer::Value(LoroValue::String(value)) => Some(value.to_string()),
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct ExistingMarkdownNode {
    index: usize,
    value: serde_json::Value,
    id: Option<String>,
    protected: bool,
}

fn movable_list_nodes(
    children: &LoroMovableList,
    allow_comment_marks: bool,
) -> Result<Vec<ExistingMarkdownNode>> {
    (0..children.len())
        .map(|index| {
            let map = children
                .get(index)
                .and_then(into_map)
                .with_context(|| format!("Macro Lexical child {} is not a map", index + 1))?;
            let value = serde_json::to_value(map.get_deep_value())?;
            let kind = map_node_type(&map).unwrap_or_else(|| "unknown".into());
            Ok(ExistingMarkdownNode {
                index,
                id: json_node_id(&value).map(ToOwned::to_owned),
                protected: !is_known_lexical_node(&kind)
                    || contains_unsupported_descendant(&value, allow_comment_marks),
                value,
            })
        })
        .collect()
}

fn json_node_id(node: &serde_json::Value) -> Option<&str> {
    node.pointer("/$/id").and_then(serde_json::Value::as_str)
}

fn prepare_markdown_target(
    existing: &[ExistingMarkdownNode],
    incoming: &[serde_json::Value],
    allow_comment_marks: bool,
) -> Result<Vec<serde_json::Value>> {
    let mut target = incoming.to_vec();
    ensure_unique_node_ids(existing.iter().filter_map(|node| node.id.as_deref()))?;
    ensure_unique_node_ids(target.iter().filter_map(json_node_id))?;

    for node in existing.iter().filter(|node| node.protected) {
        let positions = target
            .iter()
            .enumerate()
            .filter_map(|(index, incoming)| {
                let matches = node.id.as_deref().map_or_else(
                    || incoming == &node.value,
                    |id| json_node_id(incoming) == Some(id),
                );
                matches.then_some(index)
            })
            .collect::<Vec<_>>();
        match positions.as_slice() {
            [position] if target[*position] == node.value => {}
            [position] => bail!(
                "Macro block {} ({}) contains data this native editor cannot yet rewrite losslessly",
                node.index + 1,
                target[*position]
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown")
            ),
            [] if target.len() < existing.len() => {
                target.insert(node.index.min(target.len()), node.value.clone())
            }
            [] => bail!(
                "Macro block {} contains data this native editor cannot yet rewrite losslessly",
                node.index + 1
            ),
            _ => bail!("protected Macro block has an ambiguous duplicate node id"),
        }
    }
    preserve_legacy_editable_ids(existing, &mut target, allow_comment_marks);
    ensure_unique_node_ids(target.iter().filter_map(json_node_id))?;
    validate_preserved_atomic_inline_nodes(existing, &target)?;

    for node in &target {
        let kind = node
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        if is_known_lexical_node(kind)
            && !contains_unsupported_descendant(node, allow_comment_marks)
        {
            continue;
        }
        let matches_protected =
            existing
                .iter()
                .filter(|existing| existing.protected)
                .any(|existing| {
                    existing.id.as_deref().map_or_else(
                        || &existing.value == node,
                        |id| json_node_id(node) == Some(id) && &existing.value == node,
                    )
                });
        if !matches_protected {
            bail!("native edit introduced an unsupported Macro Lexical node ({kind})");
        }
    }
    Ok(target)
}

/// Mention decorators are editable only as atomic, lossless values. The native
/// Markdown surface may change text around them, while adding, removing,
/// reordering, or mutating their identity payload requires a typed Macro UI.
/// This preflight runs before any Loro operation so a bad projection cannot
/// partially update the document.
fn validate_preserved_atomic_inline_nodes(
    existing: &[ExistingMarkdownNode],
    target: &[serde_json::Value],
) -> Result<()> {
    let mut current_atoms = Vec::new();
    for node in existing {
        collect_atomic_inline_nodes(&node.value, &mut current_atoms);
    }
    let mut target_atoms = Vec::new();
    for node in target {
        collect_atomic_inline_nodes(node, &mut target_atoms);
    }
    if current_atoms != target_atoms {
        bail!("native edit attempted to add, remove, reorder, or mutate an atomic Macro mention");
    }
    Ok(())
}

fn collect_atomic_inline_nodes(node: &serde_json::Value, nodes: &mut Vec<serde_json::Value>) {
    let kind = node
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if is_atomic_inline_node(kind) {
        nodes.push(node.clone());
        return;
    }
    if let Some(children) = node.get("children").and_then(serde_json::Value::as_array) {
        for child in children {
            collect_atomic_inline_nodes(child, nodes);
        }
    }
}

fn preserve_legacy_editable_ids(
    existing: &[ExistingMarkdownNode],
    target: &mut [serde_json::Value],
    allow_comment_marks: bool,
) {
    let existing_editable = existing
        .iter()
        .filter(|node| !node.protected)
        .collect::<Vec<_>>();
    let target_editable = target
        .iter()
        .enumerate()
        .filter(|(_, node)| {
            let kind = node
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            is_known_lexical_node(kind)
                && !contains_unsupported_descendant(node, allow_comment_marks)
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if existing_editable.len() != target_editable.len() {
        return;
    }
    let existing_ids = existing
        .iter()
        .filter_map(|node| node.id.as_deref())
        .collect::<std::collections::HashSet<_>>();
    let claimed_ids = target
        .iter()
        .filter_map(json_node_id)
        .filter(|id| existing_ids.contains(id))
        .map(ToOwned::to_owned)
        .collect::<std::collections::HashSet<_>>();
    for (existing, target_index) in existing_editable.into_iter().zip(target_editable) {
        let Some(existing_id) = existing.id.as_deref() else {
            continue;
        };
        let target_id = json_node_id(&target[target_index]);
        if claimed_ids.contains(existing_id)
            || target_id.is_some_and(|target_id| existing_ids.contains(target_id))
        {
            continue;
        }
        set_json_node_id(&mut target[target_index], existing_id);
    }
}

fn set_json_node_id(node: &mut serde_json::Value, id: &str) {
    if let Some(node) = node.as_object_mut() {
        let metadata = node.entry("$").or_insert_with(|| serde_json::json!({}));
        if let Some(metadata) = metadata.as_object_mut() {
            metadata.insert("id".into(), id.into());
        }
    }
}

fn ensure_unique_node_ids<'a>(ids: impl IntoIterator<Item = &'a str>) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for id in ids {
        if !seen.insert(id) {
            bail!("duplicate Macro Lexical node id {id}");
        }
    }
    Ok(())
}

fn find_current_node(
    children: &LoroMovableList,
    target: &serde_json::Value,
) -> Result<Option<usize>> {
    let target_id = json_node_id(target);
    for index in 0..children.len() {
        let Some(map) = children.get(index).and_then(into_map) else {
            continue;
        };
        let value = serde_json::to_value(map.get_deep_value())?;
        if target_id.map_or_else(|| &value == target, |id| json_node_id(&value) == Some(id)) {
            return Ok(Some(index));
        }
    }
    Ok(None)
}

fn reconcile_rewritable_node(
    map: &LoroMap,
    target: &serde_json::Value,
    allow_comment_marks: bool,
) -> Result<()> {
    let target_object = target
        .as_object()
        .context("Macro Lexical node must be an object")?;
    for (key, value) in target_object {
        match (key.as_str(), value) {
            ("children", serde_json::Value::Array(target_children)) => {
                let children = match map.get(key).and_then(into_movable_list) {
                    Some(children) => children,
                    None => map
                        .insert_container(key, LoroMovableList::new())
                        .context("failed to create Lexical children list")?,
                };
                reconcile_rewritable_children(&children, target_children, allow_comment_marks)?;
            }
            ("text", serde_json::Value::String(target_text)) => {
                let text = match map.get(key).and_then(into_text) {
                    Some(text) => text,
                    None => map
                        .insert_container(key, LoroText::new())
                        .context("failed to create Lexical text")?,
                };
                update_loro_text(&text, target_text)?;
            }
            ("$", serde_json::Value::Object(metadata)) => {
                let metadata_map = match map.get(key).and_then(into_map) {
                    Some(metadata_map) => metadata_map,
                    None => map
                        .insert_container(key, LoroMap::new())
                        .context("failed to create Lexical metadata map")?,
                };
                reconcile_plain_map(&metadata_map, metadata)?;
            }
            _ => map
                .insert(key, LoroValue::from(value.clone()))
                .with_context(|| format!("failed to update Lexical field {key}"))?,
        }
    }
    Ok(())
}

fn reconcile_plain_map(
    map: &LoroMap,
    target: &serde_json::Map<String, serde_json::Value>,
) -> Result<()> {
    for (key, value) in target {
        if let serde_json::Value::Object(object) = value {
            let nested = match map.get(key).and_then(into_map) {
                Some(nested) => nested,
                None => map
                    .insert_container(key, LoroMap::new())
                    .with_context(|| format!("failed to create Lexical metadata field {key}"))?,
            };
            reconcile_plain_map(&nested, object)?;
        } else {
            map.insert(key, LoroValue::from(value.clone()))
                .with_context(|| format!("failed to update Lexical metadata field {key}"))?;
        }
    }
    Ok(())
}

fn reconcile_rewritable_children(
    children: &LoroMovableList,
    target: &[serde_json::Value],
    allow_comment_marks: bool,
) -> Result<()> {
    for node in target {
        let kind = node
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        if !is_rewritable_lexical_node(kind, allow_comment_marks)
            || contains_unsupported_descendant(node, allow_comment_marks)
        {
            bail!("cannot reconcile unsupported inline Macro Lexical node ({kind})");
        }
    }
    ensure_unique_node_ids(target.iter().filter_map(json_node_id))?;
    for (target_index, node) in target.iter().enumerate() {
        if let Some(current_index) = find_current_node(children, node)? {
            if current_index != target_index {
                children
                    .mov(current_index, target_index)
                    .context("failed to reorder inline Macro Lexical node")?;
            }
            let map = children
                .get(target_index)
                .and_then(into_map)
                .context("Macro Lexical child is not a map")?;
            let current_kind = map_node_type(&map).unwrap_or_else(|| "unknown".into());
            let target_kind = node
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            if current_kind == target_kind {
                reconcile_rewritable_node(&map, node, allow_comment_marks)?;
            } else {
                children
                    .set_container(target_index, build_loro_map(node)?)
                    .context("failed to replace inline Macro Lexical node")?;
            }
        } else {
            children
                .insert_container(target_index, build_loro_map(node)?)
                .context("failed to insert inline Macro Lexical node")?;
        }
    }
    while children.len() > target.len() {
        children
            .delete(children.len() - 1, 1)
            .context("failed to remove inline Macro Lexical node")?;
    }
    Ok(())
}

fn update_loro_text(text: &LoroText, target: &str) -> Result<()> {
    let current = text.to_string();
    if current == target {
        return Ok(());
    }
    let current_chars = current.chars().collect::<Vec<_>>();
    let target_chars = target.chars().collect::<Vec<_>>();
    let prefix = current_chars
        .iter()
        .zip(&target_chars)
        .take_while(|(left, right)| left == right)
        .count();
    let suffix = current_chars[prefix..]
        .iter()
        .rev()
        .zip(target_chars[prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    let delete_len = current_chars.len() - prefix - suffix;
    let insert = target_chars[prefix..target_chars.len() - suffix]
        .iter()
        .collect::<String>();
    text.splice(prefix, delete_len, &insert)
        .context("failed to update Macro Lexical text")?;
    Ok(())
}

fn is_known_lexical_node(kind: &str) -> bool {
    matches!(
        kind,
        "paragraph"
            | "heading"
            | "quote"
            | "code"
            | "custom-code"
            | "horizontalrule"
            | "list"
            | "table"
            | "image"
            | "video"
            | "equation"
    )
}

fn is_rewritable_lexical_node(kind: &str, allow_comment_marks: bool) -> bool {
    (allow_comment_marks && kind == "comment-mark")
        || is_atomic_inline_node(kind)
        || matches!(
            kind,
            "root"
                | "paragraph"
                | "heading"
                | "quote"
                | "code"
                | "custom-code"
                | "horizontalrule"
                | "list"
                | "listitem"
                | "table"
                | "tablerow"
                | "tablecell"
                | "image"
                | "video"
                | "equation"
                | "link"
                | "autolink"
                | "text"
                | "code-highlight"
                | "unlinked-text"
                | "inline-search"
                | "linebreak"
        )
}

fn is_atomic_inline_node(kind: &str) -> bool {
    kind == "mention" || kind.ends_with("-mention") || matches!(kind, "snapshot" | "watermark")
}

fn contains_unsupported_descendant(node: &serde_json::Value, allow_comment_marks: bool) -> bool {
    node.get("children")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|children| {
            children.iter().any(|child| {
                child
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|kind| !is_rewritable_lexical_node(kind, allow_comment_marks))
                    || contains_unsupported_descendant(child, allow_comment_marks)
            })
        })
}

fn build_loro_map(value: &serde_json::Value) -> Result<LoroMap> {
    let object = value
        .as_object()
        .context("Macro Lexical node must be an object")?;
    let map = LoroMap::new();
    for (key, value) in object {
        match (key.as_str(), value) {
            ("children", serde_json::Value::Array(children)) => {
                let list = map
                    .insert_container(key, LoroMovableList::new())
                    .context("failed to create Lexical children list")?;
                for child in children {
                    list.push_container(build_loro_map(child)?)
                        .context("failed to create Lexical child")?;
                }
            }
            ("text", serde_json::Value::String(text)) => {
                let loro_text = map
                    .insert_container(key, LoroText::new())
                    .context("failed to create Lexical text")?;
                loro_text
                    .insert(0, text)
                    .context("failed to insert Lexical text")?;
            }
            ("ids", serde_json::Value::Array(ids)) => {
                let list = map
                    .insert_container(key, LoroList::new())
                    .context("failed to create Lexical id list")?;
                for id in ids {
                    list.push(LoroValue::from(id.clone()))
                        .context("failed to insert Lexical id")?;
                }
            }
            ("$", serde_json::Value::Object(_)) => {
                map.insert_container(key, build_loro_map(value)?)
                    .context("failed to create Lexical metadata map")?;
            }
            _ => map
                .insert(key, LoroValue::from(value.clone()))
                .with_context(|| format!("failed to insert Lexical field {key}"))?,
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use loro::{LoroMap, LoroMovableList, LoroText};
    use parking_lot::Mutex;
    use std::sync::Arc;

    const TYPESCRIPT_MARKDOWN_GOLDEN: &str =
        include_str!("../fixtures/markdown_golden_typescript.json");
    const TYPESCRIPT_TWO_CLIENT_FIXTURE: &str =
        include_str!("../fixtures/markdown_two_client_typescript.json");

    fn decode_hex_fixture(hex: &str) -> Vec<u8> {
        assert_eq!(hex.len() % 2, 0, "fixture hex must contain whole bytes");
        hex.as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let text = std::str::from_utf8(pair).expect("fixture hex must be UTF-8");
                u8::from_str_radix(text, 16).expect("fixture must contain hexadecimal bytes")
            })
            .collect()
    }

    #[test]
    fn imports_and_edits_macro_typescript_markdown_golden() {
        let fixture: serde_json::Value = serde_json::from_str(TYPESCRIPT_MARKDOWN_GOLDEN).unwrap();
        assert_eq!(fixture["schema"], "macro.markdown-loro.typescript.v1");
        assert_eq!(
            fixture["macro_revision"],
            "7324c6adaee90070e64bcabf3c0d77600c96db71"
        );
        let snapshot = decode_hex_fixture(fixture["snapshot_hex"].as_str().unwrap());
        assert_eq!(snapshot.len(), 784);

        let native = CollaborativeDocument::from_snapshot(&snapshot, Some(4_201)).unwrap();
        native.validate_markdown_schema().unwrap();
        let imported = native.deep_json().unwrap();
        assert_eq!(imported["root"]["type"], "root");
        assert_eq!(imported["root"]["$"]["id"], "root");
        assert_eq!(imported["root"]["children"][0]["type"], "paragraph");
        let paragraph_id = imported["root"]["children"][0]["$"]["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let round_tripped =
            CollaborativeDocument::from_snapshot(&native.export_snapshot().unwrap(), Some(4_202))
                .unwrap();
        assert_eq!(round_tripped.deep_json().unwrap(), imported);

        let edited = serde_json::json!({"root":{"children":[{
            "$":{"id": paragraph_id},
            "type":"paragraph",
            "version":1,
            "direction":null,
            "format":"",
            "indent":0,
            "textFormat":0,
            "textStyle":"",
            "children":[{
                "$":{"id":"native-text"},
                "type":"text",
                "version":1,
                "text":"Edited in GPUI",
                "format":0,
                "detail":0,
                "mode":"normal",
                "style":""
            }]
        }]}});
        let update = native.replace_markdown_state(&edited).unwrap();
        assert!(!update.is_empty());
        round_tripped.import_remote(&update).unwrap();
        let converged = round_tripped.deep_json().unwrap();
        assert_eq!(converged, native.deep_json().unwrap());
        assert_eq!(
            converged["root"]["children"][0]["children"][0]["text"],
            "Edited in GPUI"
        );
        assert_eq!(
            converged["root"]["children"][0]["$"]["id"],
            edited["root"]["children"][0]["$"]["id"]
        );
    }

    #[test]
    fn cached_snapshot_and_unacknowledged_wal_restore_exact_local_state() {
        let source = CollaborativeDocument::new(Some(31)).unwrap();
        let root = source.loro().get_map("root");
        root.insert("type", "root").unwrap();
        let children = root
            .insert_container("children", LoroMovableList::new())
            .unwrap();
        let paragraph = LoroMap::new();
        paragraph.insert("type", "paragraph").unwrap();
        let text = paragraph.insert_container("text", LoroText::new()).unwrap();
        text.insert(0, "cached").unwrap();
        children.push_container(paragraph).unwrap();
        source.commit();

        let cached = source.export_shallow_snapshot().unwrap();
        let cached_version = source.encoded_version_vector();
        text.insert(text.len_unicode(), " + offline").unwrap();
        source.commit();
        let pending = source.export_updates_since(&cached_version).unwrap();

        let restored =
            CollaborativeDocument::from_cached_state(Some(&cached), &[pending], Some(32)).unwrap();
        assert_eq!(restored.deep_json().unwrap(), source.deep_json().unwrap());
    }

    #[test]
    fn native_and_typescript_markdown_updates_converge_in_both_orders() {
        let fixture: serde_json::Value =
            serde_json::from_str(TYPESCRIPT_TWO_CLIENT_FIXTURE).unwrap();
        assert_eq!(fixture["schema"], "macro.markdown-loro.two-client.v1");
        assert_eq!(
            fixture["coverage"],
            serde_json::json!(["paragraph", "heading", "checklist", "unknown_opaque_block"])
        );
        let seed = decode_hex_fixture(fixture["seed_snapshot_hex"].as_str().unwrap());
        let browser_update = decode_hex_fixture(fixture["browser_update_hex"].as_str().unwrap());
        let recorded_native_update =
            decode_hex_fixture(fixture["native_update_hex"].as_str().unwrap());

        let browser_only = CollaborativeDocument::from_snapshot(&seed, Some(7_011)).unwrap();
        assert!(
            !browser_only
                .import_remote(&browser_update)
                .unwrap()
                .has_missing_dependencies
        );
        assert_eq!(
            browser_only.deep_json().unwrap(),
            fixture["expected_after_browser_update"]
        );

        // Recreate the native branch from exactly the same JavaScript seed.
        // Export bytes contain recorded wall-clock metadata, so semantic
        // convergence—not byte-for-byte reproduction—is the stable assertion.
        let native = CollaborativeDocument::from_snapshot(&seed, Some(702)).unwrap();
        let baseline = native.encoded_version_vector();
        let mut native_state = native.deep_json().unwrap();
        native_state["root"]["children"][1]["children"][0]["text"] =
            "Browser heading from native".into();
        native.replace_markdown_state(&native_state).unwrap();
        let generated_native_update = native.export_updates_since(&baseline).unwrap();
        assert!(!generated_native_update.is_empty());
        native.import_remote(&browser_update).unwrap();
        let native_converged = native.deep_json().unwrap();
        assert_eq!(native_converged, fixture["expected_converged"]);
        assert_eq!(native_converged["root"]["children"][2]["type"], "list");
        assert_eq!(
            native_converged["root"]["children"][2]["children"][0]["checked"],
            false
        );
        assert_eq!(
            native_converged["root"]["children"][3],
            fixture["expected_after_browser_update"]["root"]["children"][3]
        );

        // This branch models the browser importing freshly generated Rust
        // bytes, with the browser edit arriving first.
        let browser_receives_native =
            CollaborativeDocument::from_snapshot(&seed, Some(7_012)).unwrap();
        browser_receives_native
            .import_remote(&browser_update)
            .unwrap();
        browser_receives_native
            .import_remote(&generated_native_update)
            .unwrap();
        assert_eq!(
            browser_receives_native.deep_json().unwrap(),
            fixture["expected_converged"]
        );

        // The recorded Rust update was independently imported by the actual
        // JavaScript/WASM runtime to produce expected_converged. Rust must
        // accept that same pair in reverse order and under duplicate delivery.
        let native_receives_browser =
            CollaborativeDocument::from_snapshot(&seed, Some(7_013)).unwrap();
        native_receives_browser
            .import_remote(&recorded_native_update)
            .unwrap();
        native_receives_browser
            .import_remote_batch(&[browser_update.clone(), browser_update])
            .unwrap();
        assert_eq!(
            native_receives_browser.deep_json().unwrap(),
            fixture["expected_converged"]
        );
    }

    #[test]
    fn two_documents_converge_through_incremental_updates() {
        let a = CollaborativeDocument::new(Some(1)).unwrap();
        let b = CollaborativeDocument::new(Some(2)).unwrap();
        a.loro().get_text("body").insert(0, "hello").unwrap();
        a.commit();
        b.import_remote(&a.export_all_updates().unwrap()).unwrap();
        b.loro().get_text("body").insert(5, " world").unwrap();
        b.commit();
        let since = a.encoded_version_vector();
        a.import_remote(&b.export_updates_since(&since).unwrap())
            .unwrap();
        assert_eq!(a.deep_json().unwrap(), b.deep_json().unwrap());
    }

    #[test]
    fn partitioned_clients_converge_after_duplicate_and_reordered_delivery() {
        let a = CollaborativeDocument::new(Some(101)).unwrap();
        a.loro().get_text("body").insert(0, "seed").unwrap();
        a.commit();
        let b =
            CollaborativeDocument::from_snapshot(&a.export_snapshot().unwrap(), Some(202)).unwrap();
        let common_version = a.encoded_version_vector();

        a.loro().get_text("body").insert(0, "A:").unwrap();
        a.commit();
        b.loro().get_text("body").insert(4, ":B").unwrap();
        b.commit();
        let update_a = a.export_updates_since(&common_version).unwrap();
        let update_b = b.export_updates_since(&common_version).unwrap();

        assert!(!a.import_remote(&update_b).unwrap().has_missing_dependencies);
        assert!(!a.import_remote(&update_b).unwrap().has_missing_dependencies);
        assert!(
            !b.import_remote_batch(&[update_a.clone(), update_a])
                .unwrap()
                .has_missing_dependencies
        );
        assert_eq!(a.deep_json().unwrap(), b.deep_json().unwrap());
    }

    #[test]
    fn missing_dependency_update_is_buffered_until_its_predecessor_arrives() {
        let source = CollaborativeDocument::new(Some(303)).unwrap();
        let empty_version = source.encoded_version_vector();
        source.loro().get_text("body").insert(0, "first").unwrap();
        source.commit();
        let first_update = source.export_updates_since(&empty_version).unwrap();
        let first_version = source.encoded_version_vector();
        source.loro().get_text("body").insert(5, " second").unwrap();
        source.commit();
        let second_update = source.export_updates_since(&first_version).unwrap();

        let target = CollaborativeDocument::new(Some(404)).unwrap();
        assert!(
            target
                .import_remote(&second_update)
                .unwrap()
                .has_missing_dependencies
        );
        assert!(
            !target
                .import_remote(&first_update)
                .unwrap()
                .has_missing_dependencies
        );
        assert_eq!(target.deep_json().unwrap(), source.deep_json().unwrap());
    }

    #[test]
    fn unknown_nodes_survive_snapshot_round_trip() {
        let doc = CollaborativeDocument::new(Some(7)).unwrap();
        let root = doc.loro().get_map("root");
        root.insert("futureScalar", "keep-me").unwrap();
        let future = root.insert_container("futureNode", LoroMap::new()).unwrap();
        future.insert("type", "node-from-a-newer-client").unwrap();
        future
            .insert_container("text", LoroText::new())
            .unwrap()
            .insert(0, "lossless")
            .unwrap();
        doc.commit();

        let restored =
            CollaborativeDocument::from_snapshot(&doc.export_snapshot().unwrap(), Some(8)).unwrap();
        assert_eq!(
            restored.deep_json().unwrap()["root"]["futureNode"]["text"],
            "lossless"
        );
        assert_eq!(
            restored.deep_json().unwrap()["root"]["futureScalar"],
            "keep-me"
        );
    }

    #[test]
    fn text_awareness_cursors_resolve_after_concurrent_edits() {
        let document = CollaborativeDocument::new(Some(71)).unwrap();
        let root = document.loro().get_map("root");
        root.insert("type", "root").unwrap();
        root.insert_container("$", LoroMap::new())
            .unwrap()
            .insert("id", "root")
            .unwrap();
        let children = root
            .insert_container("children", LoroMovableList::new())
            .unwrap();
        children
            .push_container(
                build_loro_map(&serde_json::json!({
                    "$": {"id": "paragraph-1"},
                    "type": "paragraph",
                    "children": [{
                        "$": {"id": "text-1"},
                        "type": "text",
                        "text": "héllo"
                    }]
                }))
                .unwrap(),
            )
            .unwrap();
        document.commit();

        let cursor = document.encode_text_cursor("text-1", 2).unwrap();
        assert_eq!(document.resolve_text_cursor("text-1", &cursor).unwrap(), 2);
        let text = find_text_container(&root, "text-1").unwrap();
        text.insert(0, "go ").unwrap();
        document.commit();
        assert_eq!(document.resolve_text_cursor("text-1", &cursor).unwrap(), 5);
        assert!(document.resolve_text_cursor("missing", &cursor).is_err());
    }

    #[test]
    fn local_subscription_does_not_echo_remote_imports() {
        let source = CollaborativeDocument::new(Some(9)).unwrap();
        source.loro().get_text("body").insert(0, "remote").unwrap();
        source.commit();

        let target = CollaborativeDocument::new(Some(10)).unwrap();
        let local = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
        let captured = local.clone();
        let _subscription = target.subscribe_local_updates(Box::new(move |update| {
            captured.lock().push(update.clone());
            true
        }));
        target
            .import_remote(&source.export_all_updates().unwrap())
            .unwrap();
        assert!(local.lock().is_empty());
        target.loro().get_text("body").insert(6, " edit").unwrap();
        target.commit();
        assert_eq!(local.lock().len(), 1);
    }

    #[test]
    fn replacing_markdown_preserves_unknown_blocks_and_exports_an_update() {
        let document = CollaborativeDocument::new(Some(11)).unwrap();
        let root = document.loro().get_map("root");
        root.insert("type", "root").unwrap();
        let metadata = root.insert_container("$", LoroMap::new()).unwrap();
        metadata.insert("id", "root").unwrap();
        let children = root
            .insert_container("children", LoroMovableList::new())
            .unwrap();
        let paragraph = build_loro_map(&serde_json::json!({
            "$": {"id": "paragraph-1"},
            "type": "paragraph",
            "version": 1,
            "children": []
        }))
        .unwrap();
        children.push_container(paragraph).unwrap();
        let future = build_loro_map(&serde_json::json!({
            "$": {"id": "future-1"},
            "type": "future-widget",
            "version": 1,
            "payload": {"meaning": 42}
        }))
        .unwrap();
        children.push_container(future).unwrap();
        document.commit();

        let update = document
            .replace_markdown_state(&serde_json::json!({"root":{"children":[{
                "$": {"id": "replacement"},
                "type": "heading",
                "tag": "h2",
                "version": 1,
                "children": [{
                    "$": {"id": "text-1"},
                    "type": "text",
                    "text": "Plan",
                    "version": 1
                }]
            }]}}))
            .unwrap();
        let value = document.deep_json().unwrap();
        assert!(!update.is_empty());
        assert_eq!(value["root"]["children"][0]["type"], "heading");
        assert_eq!(value["root"]["children"][0]["$"]["id"], "paragraph-1");
        assert_eq!(value["root"]["children"][1]["type"], "future-widget");
        assert_eq!(value["root"]["children"][1]["payload"]["meaning"], 42);
    }

    #[test]
    fn native_custom_code_and_video_edits_reconcile_without_flattening_macro_fields() {
        let document = CollaborativeDocument::new(Some(111)).unwrap();
        let root = document.loro().get_map("root");
        root.insert("type", "root").unwrap();
        root.insert_container("$", LoroMap::new())
            .unwrap()
            .insert("id", "root")
            .unwrap();
        let children = root
            .insert_container("children", LoroMovableList::new())
            .unwrap();
        let code = serde_json::json!({
            "$":{"id":"code-1"},
            "type":"custom-code",
            "version":1,
            "language":"rust",
            "futureCodeField":"preserve",
            "children":[{
                "$":{"id":"highlight-1"},
                "type":"code-highlight",
                "version":1,
                "text":"fn old() {}",
                "highlightType":"function"
            }]
        });
        let video = serde_json::json!({
            "$":{"id":"video-1"},
            "type":"video",
            "version":1,
            "url":"https://cdn.example/old.mp4",
            "controls":true,
            "width":1280,
            "height":720
        });
        children
            .push_container(build_loro_map(&code).unwrap())
            .unwrap();
        children
            .push_container(build_loro_map(&video).unwrap())
            .unwrap();
        document.commit();

        let mut edited_code = code.clone();
        edited_code["language"] = "typescript".into();
        edited_code["children"][0]["text"] = "const ready = true;".into();
        let mut edited_video = video.clone();
        edited_video["url"] = "https://cdn.example/new.mp4".into();
        let update = document
            .replace_markdown_state(&serde_json::json!({"root":{"children":[
                edited_code,
                edited_video
            ]}}))
            .unwrap();
        assert!(!update.is_empty());

        let state = document.deep_json().unwrap();
        assert_eq!(state["root"]["children"][0]["type"], "custom-code");
        assert_eq!(
            state["root"]["children"][0]["children"][0]["text"],
            "const ready = true;"
        );
        assert_eq!(state["root"]["children"][0]["futureCodeField"], "preserve");
        assert_eq!(
            state["root"]["children"][1]["url"],
            "https://cdn.example/new.mp4"
        );
        assert_eq!(state["root"]["children"][1]["width"], 1280);
    }

    #[test]
    fn replacing_markdown_refuses_to_destroy_unknown_inline_nodes() {
        let document = CollaborativeDocument::new(Some(12)).unwrap();
        let root = document.loro().get_map("root");
        root.insert("type", "root").unwrap();
        root.insert_container("$", LoroMap::new())
            .unwrap()
            .insert("id", "root")
            .unwrap();
        let children = root
            .insert_container("children", LoroMovableList::new())
            .unwrap();
        children
            .push_container(
                build_loro_map(&serde_json::json!({
                    "$": {"id": "paragraph-1"},
                    "type": "paragraph",
                    "children": [{
                        "$": {"id": "comment-1"},
                        "type": "comment-mark",
                        "ids": ["DISCUSSION:1"],
                        "threadId": 9,
                        "children": [{
                            "$": {"id": "text-1"},
                            "type": "text",
                            "text": "protected"
                        }]
                    }]
                }))
                .unwrap(),
            )
            .unwrap();
        document.commit();
        let before = document.deep_json().unwrap();
        let error = document
            .replace_markdown_state(&serde_json::json!({"root":{"children":[{
                "$": {"id": "new"},
                "type": "paragraph",
                "children": [{"type":"text", "text":"changed"}]
            }]}}))
            .unwrap_err();
        assert!(error.to_string().contains("cannot yet rewrite losslessly"));
        assert_eq!(document.deep_json().unwrap(), before);
    }

    #[test]
    fn comment_marks_require_the_annotation_policy_and_remain_rewritable() {
        let document = CollaborativeDocument::new(Some(121)).unwrap();
        let root = document.loro().get_map("root");
        root.insert("type", "root").unwrap();
        root.insert_container("$", LoroMap::new())
            .unwrap()
            .insert("id", "root")
            .unwrap();
        let children = root
            .insert_container("children", LoroMovableList::new())
            .unwrap();
        children
            .push_container(
                build_loro_map(&serde_json::json!({
                    "$": {"id": "paragraph-1"},
                    "type": "paragraph",
                    "children": [{
                        "$": {"id": "text-1"},
                        "type": "text",
                        "text": "selected"
                    }]
                }))
                .unwrap(),
            )
            .unwrap();
        document.commit();
        let annotated = serde_json::json!({"root":{"children":[{
            "$": {"id": "paragraph-1"},
            "type": "paragraph",
            "children": [{
                "$": {"id": "comment-1"},
                "type": "comment-mark",
                "ids": ["DISCUSSION:abc"],
                "threadId": 42,
                "isDraft": false,
                "children": [{
                    "$": {"id": "text-1"},
                    "type": "text",
                    "text": "selected"
                }]
            }]
        }]}});
        let before = document.deep_json().unwrap();
        assert!(document.replace_markdown_state(&annotated).is_err());
        assert_eq!(document.deep_json().unwrap(), before);
        assert!(
            !document
                .replace_annotation_state(&annotated)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            document.deep_json().unwrap()["root"]["children"][0]["children"][0]["threadId"],
            42
        );
        let edited = serde_json::json!({"root":{"children":[{
            "$": {"id": "paragraph-1"},
            "type": "paragraph",
            "children": [
                {"$":{"id":"before-1"},"type":"text","text":"before "},
                {
                    "$": {"id": "comment-1"},
                    "type": "comment-mark",
                    "ids": ["DISCUSSION:abc"],
                    "threadId": 42,
                    "isDraft": false,
                    "children": [{
                        "$": {"id": "text-1"},
                        "type": "text",
                        "text": "edited"
                    }]
                },
                {"$":{"id":"after-1"},"type":"text","text":" after"}
            ]
        }]}});
        assert!(
            !document
                .replace_annotation_state(&edited)
                .unwrap()
                .is_empty()
        );
        let current = document.deep_json().unwrap();
        assert_eq!(
            current["root"]["children"][0]["children"][1]["children"][0]["text"],
            "edited"
        );
        assert_eq!(
            current["root"]["children"][0]["children"][1]["ids"][0],
            "DISCUSSION:abc"
        );
    }

    #[test]
    fn unchanged_opaque_block_does_not_block_other_edits_or_root_state() {
        let document = CollaborativeDocument::new(Some(13)).unwrap();
        let root = document.loro().get_map("root");
        root.insert("type", "root").unwrap();
        root.insert("futureRootField", "keep-me").unwrap();
        let metadata = root.insert_container("$", LoroMap::new()).unwrap();
        metadata.insert("id", "root").unwrap();
        metadata
            .insert(
                "pinnedPropertyIds",
                LoroValue::from(serde_json::json!(["priority", "owner"])),
            )
            .unwrap();
        let children = root
            .insert_container("children", LoroMovableList::new())
            .unwrap();
        let opaque = serde_json::json!({
            "$": {"id": "paragraph-opaque"},
            "type": "paragraph",
            "version": 1,
            "children": [{
                "$": {"id": "mention-1"},
                "type": "user-mention",
                "version": 1,
                "userId": "user-1",
                "email": "person@example.com",
                "displayName": "Person"
            }]
        });
        children
            .push_container(build_loro_map(&opaque).unwrap())
            .unwrap();
        children
            .push_container(
                build_loro_map(&serde_json::json!({
                    "$": {"id": "paragraph-editable"},
                    "type": "paragraph",
                    "version": 1,
                    "children": [{
                        "$": {"id": "text-1"},
                        "type": "text",
                        "version": 1,
                        "text": "Before"
                    }]
                }))
                .unwrap(),
            )
            .unwrap();
        document.commit();

        let update = document
            .replace_markdown_state(&serde_json::json!({"root":{
                "$":{"pinnedPropertyIds":["status"]},
                "children":[
                opaque,
                {
                    "$": {"id": "replacement"},
                    "type": "paragraph",
                    "version": 1,
                    "children": [{
                        "$": {"id": "replacement-text"},
                        "type": "text",
                        "version": 1,
                        "text": "After"
                    }]
                }
            ]}}))
            .unwrap();
        let value = document.deep_json().unwrap();
        assert!(!update.is_empty());
        assert_eq!(value["root"]["children"][0]["type"], "paragraph");
        assert_eq!(
            value["root"]["children"][0]["children"][0]["type"],
            "user-mention"
        );
        assert_eq!(value["root"]["children"][1]["children"][0]["text"], "After");
        assert_eq!(value["root"]["futureRootField"], "keep-me");
        assert_eq!(
            value["root"]["$"]["pinnedPropertyIds"],
            serde_json::json!(["status"])
        );
    }

    #[test]
    fn surrounding_mention_text_reconciles_but_atomic_payload_mutations_are_rejected() {
        let document = CollaborativeDocument::new(Some(131)).unwrap();
        let root = document.loro().get_map("root");
        root.insert("type", "root").unwrap();
        root.insert_container("$", LoroMap::new())
            .unwrap()
            .insert("id", "root")
            .unwrap();
        let children = root
            .insert_container("children", LoroMovableList::new())
            .unwrap();
        let mention = serde_json::json!({
            "$":{"id":"mention-1","future":"preserve"},
            "type":"user-mention",
            "version":1,
            "userId":"user-1",
            "email":"person@example.com",
            "displayName":"Person"
        });
        children
            .push_container(
                build_loro_map(&serde_json::json!({
                    "$":{"id":"paragraph-1"},
                    "type":"paragraph",
                    "children":[
                        {"$":{"id":"text-1"},"type":"text","text":"Hello "},
                        mention,
                        {"$":{"id":"text-2"},"type":"text","text":"!"}
                    ]
                }))
                .unwrap(),
            )
            .unwrap();
        document.commit();

        let edited = serde_json::json!({"root":{"children":[{
            "$":{"id":"paragraph-1"},
            "type":"paragraph",
            "children":[
                {"$":{"id":"text-1"},"type":"text","text":"Welcome "},
                mention,
                {"$":{"id":"text-2"},"type":"text","text":" — today"}
            ]
        }]}});
        assert!(!document.replace_markdown_state(&edited).unwrap().is_empty());
        let current = document.deep_json().unwrap();
        assert_eq!(
            current["root"]["children"][0]["children"][0]["text"],
            "Welcome "
        );
        assert_eq!(
            current["root"]["children"][0]["children"][1],
            edited["root"]["children"][0]["children"][1]
        );

        let mut corrupted = edited;
        corrupted["root"]["children"][0]["children"][1]["userId"] = "attacker".into();
        let before = document.deep_json().unwrap();
        let error = document.replace_markdown_state(&corrupted).unwrap_err();
        assert!(error.to_string().contains("atomic Macro mention"));
        assert_eq!(document.deep_json().unwrap(), before);
    }

    #[test]
    fn reordering_editable_blocks_moves_containers_around_opaque_anchors() {
        let document = CollaborativeDocument::new(Some(14)).unwrap();
        let root = document.loro().get_map("root");
        root.insert("type", "root").unwrap();
        root.insert_container("$", LoroMap::new())
            .unwrap()
            .insert("id", "root")
            .unwrap();
        let children = root
            .insert_container("children", LoroMovableList::new())
            .unwrap();
        let paragraph = |id: &str, text: &str| {
            serde_json::json!({
                "$":{"id":id},
                "type":"paragraph",
                "version":1,
                "children":[{"$":{"id":format!("{id}-text")},"type":"text","text":text}]
            })
        };
        let first = paragraph("first", "First");
        let second = paragraph("second", "Second");
        let opaque = serde_json::json!({
            "$":{"id":"card"},
            "type":"document-card",
            "version":1,
            "documentId":"document-1",
            "documentName":"Reference"
        });
        for node in [&first, &opaque, &second] {
            children
                .push_container(build_loro_map(node).unwrap())
                .unwrap();
        }
        document.commit();

        let mut moved_second = second.clone();
        moved_second["children"][0]["text"] = "Second updated".into();
        let mut moved_first = first.clone();
        moved_first["children"][0]["text"] = "First updated".into();
        document
            .replace_markdown_state(&serde_json::json!({"root":{"children":[
                moved_second,
                opaque.clone(),
                moved_first
            ]}}))
            .unwrap();

        let value = document.deep_json().unwrap();
        assert_eq!(value["root"]["children"][0]["$"]["id"], "second");
        assert_eq!(
            value["root"]["children"][0]["children"][0]["text"],
            "Second updated"
        );
        assert_eq!(value["root"]["children"][1], opaque);
        assert_eq!(value["root"]["children"][2]["$"]["id"], "first");
        assert_eq!(
            value["root"]["children"][2]["children"][0]["text"],
            "First updated"
        );
    }

    #[test]
    fn native_text_edits_merge_with_concurrent_remote_text_operations() {
        let seed = CollaborativeDocument::new(Some(20)).unwrap();
        let root = seed.loro().get_map("root");
        root.insert("type", "root").unwrap();
        root.insert_container("$", LoroMap::new())
            .unwrap()
            .insert("id", "root")
            .unwrap();
        root.insert_container("children", LoroMovableList::new())
            .unwrap()
            .push_container(
                build_loro_map(&serde_json::json!({
                    "$":{"id":"paragraph"},
                    "type":"paragraph",
                    "version":1,
                    "children":[{
                        "$":{"id":"text"},
                        "type":"text",
                        "version":1,
                        "text":"hello",
                        "format":0
                    }]
                }))
                .unwrap(),
            )
            .unwrap();
        seed.commit();

        let snapshot = seed.export_snapshot().unwrap();
        let local = CollaborativeDocument::from_snapshot(&snapshot, Some(21)).unwrap();
        let remote = CollaborativeDocument::from_snapshot(&snapshot, Some(22)).unwrap();
        let baseline = local.encoded_version_vector();

        local
            .replace_markdown_state(&serde_json::json!({"root":{"children":[{
                "$":{"id":"paragraph"},
                "type":"paragraph",
                "version":1,
                "children":[{
                    "$":{"id":"text"},
                    "type":"text",
                    "version":1,
                    "text":"hello local",
                    "format":0
                }]
            }]}}))
            .unwrap();
        let remote_text = remote
            .loro()
            .get_map("root")
            .get("children")
            .and_then(into_movable_list)
            .unwrap()
            .get(0)
            .and_then(into_map)
            .unwrap()
            .get("children")
            .and_then(into_movable_list)
            .unwrap()
            .get(0)
            .and_then(into_map)
            .unwrap()
            .get("text")
            .and_then(into_text)
            .unwrap();
        remote_text.insert(5, " remote").unwrap();
        remote.commit();

        let local_update = local.export_updates_since(&baseline).unwrap();
        let remote_update = remote.export_updates_since(&baseline).unwrap();
        local.import_remote(&remote_update).unwrap();
        remote.import_remote(&local_update).unwrap();
        assert_eq!(local.deep_json().unwrap(), remote.deep_json().unwrap());
        let merged = local.deep_json().unwrap()["root"]["children"][0]["children"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(merged.contains("local"), "local edit was lost: {merged}");
        assert!(merged.contains("remote"), "remote edit was lost: {merged}");
    }
}
