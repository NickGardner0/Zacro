use anyhow::{Result, anyhow};
use loro::{LoroValue, ToJson, awareness::EphemeralStore};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

/// Macro's Lexical collaboration provider expires presence after five seconds.
pub const AWARENESS_TIMEOUT_MS: i64 = 5_000;

const COLLABORATOR_COLORS: [&str; 11] = [
    "accent-30",
    "accent-60",
    "accent-90",
    "accent-120",
    "accent-150",
    "accent-180",
    "accent-210",
    "accent-240",
    "accent-270",
    "accent-300",
    "accent-330",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AwarenessPoint {
    #[serde(rename = "nodeId")]
    pub node_id: String,
    pub cursor: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AwarenessSelection {
    pub anchor: AwarenessPoint,
    pub focus: AwarenessPoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwarenessPeer {
    pub peer_id: String,
    pub user_id: Option<String>,
    pub color: String,
    pub selection: Option<AwarenessSelection>,
}

/// Byte-compatible wrapper around Loro's ephemeral store used by Macro's
/// SolidJS collaboration client.
#[derive(Clone)]
pub struct DocumentAwareness {
    store: EphemeralStore,
    local_peer_id: String,
    local_user_id: Option<String>,
    local_color: String,
    /// Loro 1.13.7 stamps ephemeral writes with wall-clock milliseconds and
    /// rejects an imported record whose timestamp equals the existing one.
    /// Cursor events commonly share a millisecond, so outgoing timestamps must
    /// be monotonic or the browser/native peer can silently lose a selection.
    local_timestamp: Arc<AtomicI64>,
}

#[derive(Serialize)]
struct EncodedAwarenessState<'a> {
    key: &'a str,
    value: Option<LoroValue>,
    timestamp: i64,
}

impl DocumentAwareness {
    pub fn new(peer_id: u64, user_id: Option<String>) -> Self {
        Self::with_timeout(peer_id, user_id, AWARENESS_TIMEOUT_MS)
    }

    pub fn with_timeout(peer_id: u64, user_id: Option<String>, timeout_ms: i64) -> Self {
        let local_peer_id = peer_id.to_string();
        let local_color =
            COLLABORATOR_COLORS[(peer_id as usize) % COLLABORATOR_COLORS.len()].to_owned();
        let awareness = Self {
            store: EphemeralStore::new(timeout_ms),
            local_peer_id,
            local_user_id: user_id,
            local_color,
            local_timestamp: Arc::new(AtomicI64::new(0)),
        };
        awareness.update_local(None);
        awareness
    }

    pub fn local_peer_id(&self) -> &str {
        &self.local_peer_id
    }

    /// Refreshes the local timestamp and returns exactly the payload consumed
    /// by Macro's `EphemeralStore.apply` call.
    pub fn encoded_local(&self) -> Vec<u8> {
        self.update_local(self.local_selection());
        self.encode_local_monotonic()
    }

    pub fn set_local_selection(&self, selection: Option<AwarenessSelection>) -> Vec<u8> {
        self.update_local(selection);
        self.encode_local_monotonic()
    }

    pub fn apply_remote(&self, update: &[u8]) -> Result<()> {
        if update.is_empty() {
            return Ok(());
        }
        self.store
            .apply(update)
            .map_err(|error| anyhow!(error.to_string()))
    }

    pub fn remove_outdated(&self) {
        self.store.remove_outdated();
    }

    pub fn remote_peers(&self) -> Vec<AwarenessPeer> {
        self.remove_outdated();
        let mut peers = self
            .store
            .get_all_states()
            .into_iter()
            .filter(|(peer_id, _)| peer_id != &self.local_peer_id)
            .filter_map(|(peer_id, value)| decode_peer(peer_id, value))
            .collect::<Vec<_>>();
        peers.sort_by(|left, right| left.peer_id.cmp(&right.peer_id));
        peers
    }

    fn local_selection(&self) -> Option<AwarenessSelection> {
        self.store
            .get(&self.local_peer_id)
            .and_then(|value| decode_peer(self.local_peer_id.clone(), value))
            .and_then(|peer| peer.selection)
    }

    fn update_local(&self, selection: Option<AwarenessSelection>) {
        let mut user = HashMap::<String, LoroValue>::new();
        if let Some(user_id) = self.local_user_id.as_ref() {
            user.insert("userId".to_owned(), user_id.clone().into());
        }
        user.insert("color".to_owned(), self.local_color.clone().into());
        user.insert("peerId".to_owned(), self.local_peer_id.clone().into());

        let mut value = HashMap::<String, LoroValue>::new();
        value.insert("user".to_owned(), LoroValue::from(user));
        if let Some(selection) = selection {
            value.insert("selection".to_owned(), selection_value(selection));
        }
        self.store.set(&self.local_peer_id, LoroValue::from(value));
    }

    fn encode_local_monotonic(&self) -> Vec<u8> {
        let wall_clock = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64;
        let mut previous = self.local_timestamp.load(Ordering::Relaxed);
        let timestamp = loop {
            let next = wall_clock.max(previous.saturating_add(1));
            match self.local_timestamp.compare_exchange_weak(
                previous,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break next,
                Err(actual) => previous = actual,
            }
        };
        postcard::to_allocvec(&vec![EncodedAwarenessState {
            key: &self.local_peer_id,
            value: self.store.get(&self.local_peer_id),
            timestamp,
        }])
        .expect("Macro awareness values must remain postcard encodable")
    }
}

fn selection_value(selection: AwarenessSelection) -> LoroValue {
    fn point_value(point: AwarenessPoint) -> LoroValue {
        LoroValue::from(HashMap::from([
            ("nodeId".to_owned(), LoroValue::from(point.node_id)),
            ("cursor".to_owned(), LoroValue::Binary(point.cursor.into())),
        ]))
    }

    LoroValue::from(HashMap::from([
        ("anchor".to_owned(), point_value(selection.anchor)),
        ("focus".to_owned(), point_value(selection.focus)),
    ]))
}

fn decode_peer(peer_id: String, value: LoroValue) -> Option<AwarenessPeer> {
    let value = value.to_json_value();
    let user = value.get("user")?;
    let declared_peer_id = user
        .get("peerId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&peer_id)
        .to_owned();
    let user_id = user
        .get("userId")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned);
    let color = user
        .get("color")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("accent-180")
        .to_owned();
    let selection = value
        .get("selection")
        .filter(|selection| !selection.is_null())
        .and_then(|selection| serde_json::from_value(selection.clone()).ok());
    Some(AwarenessPeer {
        peer_id: declared_peer_id,
        user_id,
        color,
        selection,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn awareness_payloads_interoperate_and_preserve_selection() {
        let alice = DocumentAwareness::new(7, Some("alice".to_owned()));
        let bob = DocumentAwareness::new(11, Some("bob".to_owned()));
        bob.apply_remote(&alice.encoded_local()).unwrap();
        assert_eq!(bob.remote_peers()[0].user_id.as_deref(), Some("alice"));

        let selection = AwarenessSelection {
            anchor: AwarenessPoint {
                node_id: "node-a".to_owned(),
                cursor: vec![1, 2, 3],
            },
            focus: AwarenessPoint {
                node_id: "node-b".to_owned(),
                cursor: vec![4, 5, 6],
            },
        };
        bob.apply_remote(&alice.set_local_selection(Some(selection.clone())))
            .unwrap();
        assert_eq!(bob.remote_peers()[0].selection, Some(selection));
    }

    #[test]
    fn malformed_remote_payload_is_reported() {
        let awareness = DocumentAwareness::new(3, None);
        assert!(awareness.apply_remote(&[1, 2, 3]).is_err());
    }

    #[test]
    fn rapid_local_updates_have_strictly_newer_wire_timestamps() {
        let alice = DocumentAwareness::new(17, Some("alice".to_owned()));
        let bob = DocumentAwareness::new(19, Some("bob".to_owned()));
        for index in 0..100_u8 {
            let selection = AwarenessSelection {
                anchor: AwarenessPoint {
                    node_id: "node".to_owned(),
                    cursor: vec![index],
                },
                focus: AwarenessPoint {
                    node_id: "node".to_owned(),
                    cursor: vec![index],
                },
            };
            bob.apply_remote(&alice.set_local_selection(Some(selection.clone())))
                .unwrap();
            assert_eq!(bob.remote_peers()[0].selection, Some(selection));
        }
    }
}
