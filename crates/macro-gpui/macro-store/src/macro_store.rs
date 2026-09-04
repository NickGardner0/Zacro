use anyhow::{Context as _, Result, bail};
use ring::{
    aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey},
    rand::{SecureRandom as _, SystemRandom},
};
use sqlez::{domain::Domain, thread_safe_connection::ThreadSafeConnection};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationDeliveryState {
    Queued,
    InFlight,
    Failed,
    Exhausted,
}

#[derive(Debug, Clone)]
pub struct PendingMutation<T> {
    pub operation_id: String,
    pub payload: T,
    pub state: MutationDeliveryState,
    pub attempt_count: u32,
    pub last_error: Option<String>,
}

/// Process-local optimistic mutation queue. Operation IDs are idempotency keys:
/// enqueueing the same ID twice never duplicates work, realtime/API acks remove
/// it once, and a disconnected in-flight operation is made retryable on the
/// next connection. Durable encrypted persistence can wrap this queue without
/// changing feature reducers or transport dispatch.
#[derive(Debug, Clone)]
pub struct MutationOutbox<T> {
    records: HashMap<String, PendingMutation<T>>,
    ordered_ids: Vec<String>,
}

impl<T> Default for MutationOutbox<T> {
    fn default() -> Self {
        Self {
            records: HashMap::new(),
            ordered_ids: Vec::new(),
        }
    }
}

impl<T: Clone> MutationOutbox<T> {
    pub fn enqueue(&mut self, operation_id: impl Into<String>, payload: T) -> bool {
        let operation_id = operation_id.into();
        if self.records.contains_key(&operation_id) {
            return false;
        }
        self.ordered_ids.push(operation_id.clone());
        self.records.insert(
            operation_id.clone(),
            PendingMutation {
                operation_id,
                payload,
                state: MutationDeliveryState::Queued,
                attempt_count: 0,
                last_error: None,
            },
        );
        true
    }

    pub fn begin_attempt(&mut self, operation_id: &str, max_attempts: u32) -> Option<T> {
        let mutation = self.records.get_mut(operation_id)?;
        if mutation.state == MutationDeliveryState::InFlight {
            return None;
        }
        if mutation.attempt_count >= max_attempts {
            mutation.state = MutationDeliveryState::Exhausted;
            return None;
        }
        mutation.attempt_count += 1;
        mutation.state = MutationDeliveryState::InFlight;
        mutation.last_error = None;
        Some(mutation.payload.clone())
    }

    pub fn fail(&mut self, operation_id: &str, error: impl Into<String>) -> bool {
        let Some(mutation) = self.records.get_mut(operation_id) else {
            return false;
        };
        mutation.state = MutationDeliveryState::Failed;
        mutation.last_error = Some(error.into());
        true
    }

    pub fn reset(&mut self, operation_id: &str) -> bool {
        let Some(mutation) = self.records.get_mut(operation_id) else {
            return false;
        };
        mutation.state = MutationDeliveryState::Queued;
        mutation.attempt_count = 0;
        mutation.last_error = None;
        true
    }

    pub fn requeue_in_flight(&mut self) {
        for mutation in self.records.values_mut() {
            if mutation.state == MutationDeliveryState::InFlight {
                mutation.state = MutationDeliveryState::Queued;
            }
        }
    }

    pub fn retryable(&self) -> Vec<(String, T)> {
        self.ordered_ids
            .iter()
            .filter_map(|operation_id| {
                let mutation = self.records.get(operation_id)?;
                matches!(
                    mutation.state,
                    MutationDeliveryState::Queued | MutationDeliveryState::Failed
                )
                .then(|| (operation_id.clone(), mutation.payload.clone()))
            })
            .collect()
    }

    pub fn acknowledge(&mut self, operation_id: &str) -> Option<PendingMutation<T>> {
        let removed = self.records.remove(operation_id)?;
        self.ordered_ids.retain(|id| id != operation_id);
        Some(removed)
    }

    /// Restores a durable record without resetting its bounded retry count.
    /// A process cannot still own an in-flight request after restart, so that
    /// state is safely made queued and retried with the original idempotency
    /// key.
    pub fn restore(&mut self, mut mutation: PendingMutation<T>) -> bool {
        if self.records.contains_key(&mutation.operation_id) {
            return false;
        }
        if mutation.state == MutationDeliveryState::InFlight {
            mutation.state = MutationDeliveryState::Queued;
        }
        self.ordered_ids.push(mutation.operation_id.clone());
        self.records.insert(mutation.operation_id.clone(), mutation);
        true
    }

    pub fn get(&self, operation_id: &str) -> Option<&PendingMutation<T>> {
        self.records.get(operation_id)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

pub enum MutationPersistenceDomain {}

impl Domain for MutationPersistenceDomain {
    const NAME: &str = "macro_mutation_outbox";
    const MIGRATIONS: &[&str] = &[r#"
        CREATE TABLE macro_mutation_outbox (
            sequence INTEGER PRIMARY KEY AUTOINCREMENT,
            account_id TEXT NOT NULL,
            operation_id TEXT NOT NULL,
            nonce BLOB NOT NULL,
            ciphertext BLOB NOT NULL,
            state TEXT NOT NULL,
            attempt_count INTEGER NOT NULL DEFAULT 0,
            last_error TEXT,
            created_at_ms INTEGER NOT NULL,
            UNIQUE(account_id, operation_id)
        ) STRICT;
        CREATE INDEX macro_mutation_outbox_pending
            ON macro_mutation_outbox(account_id, sequence);
    "#];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMutation {
    pub operation_id: String,
    pub payload: Vec<u8>,
    pub state: MutationDeliveryState,
    pub attempt_count: u32,
    pub last_error: Option<String>,
    pub created_at_ms: i64,
}

/// SQLite-backed encrypted mutation persistence. Only routing/retry metadata is
/// readable in the database; payloads are AES-256-GCM ciphertext authenticated
/// to the account and operation IDs. The encryption key belongs in the OS
/// credential store and is never written beside this database.
#[derive(Clone)]
pub struct EncryptedMutationPersistence {
    connection: ThreadSafeConnection,
    key: Arc<LessSafeKey>,
}

pub fn generate_mutation_encryption_key() -> Result<[u8; 32]> {
    let mut key = [0_u8; 32];
    SystemRandom::new()
        .fill(&mut key)
        .map_err(|_| anyhow::anyhow!("failed to generate mutation encryption key"))?;
    Ok(key)
}

impl EncryptedMutationPersistence {
    pub async fn open(path: &Path, key: [u8; 32]) -> Result<Self> {
        let path = path
            .to_str()
            .context("Macro mutation database path is not valid UTF-8")?;
        let connection = ThreadSafeConnection::builder::<MutationPersistenceDomain>(path, true)
            .with_db_initialization_query("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
            .with_connection_initialize_query("PRAGMA busy_timeout=5000;")
            .build()
            .await
            .context("failed to initialize encrypted mutation persistence")?;
        Self::new(connection, key)
    }

    #[cfg(test)]
    async fn open_memory(name: &str, key: [u8; 32]) -> Result<Self> {
        let connection = ThreadSafeConnection::builder::<MutationPersistenceDomain>(name, false)
            .with_connection_initialize_query("PRAGMA busy_timeout=5000;")
            .build()
            .await?;
        Self::new(connection, key)
    }

    fn new(connection: ThreadSafeConnection, key: [u8; 32]) -> Result<Self> {
        let key = UnboundKey::new(&AES_256_GCM, &key)
            .map_err(|_| anyhow::anyhow!("invalid mutation encryption key"))?;
        Ok(Self {
            connection,
            key: Arc::new(LessSafeKey::new(key)),
        })
    }

    pub async fn enqueue(
        &self,
        account_id: &str,
        operation_id: &str,
        payload: Vec<u8>,
        created_at_ms: i64,
    ) -> Result<()> {
        let (nonce, ciphertext) = self.seal(account_id, operation_id, payload)?;
        let account_id = account_id.to_owned();
        let operation_id = operation_id.to_owned();
        self.connection
            .write(move |connection| {
                connection.exec_bound::<(String, String, Vec<u8>, Vec<u8>, i64)>(
                    r#"
                    INSERT OR IGNORE INTO macro_mutation_outbox
                        (account_id, operation_id, nonce, ciphertext, state, created_at_ms)
                    VALUES (?, ?, ?, ?, 'queued', ?)
                    "#,
                )?((account_id, operation_id, nonce, ciphertext, created_at_ms))?;
                anyhow::Ok(())
            })
            .await
            .context("failed to persist encrypted mutation")
    }

    pub async fn record_attempt(&self, account_id: &str, operation_id: &str) -> Result<()> {
        let account_id = account_id.to_owned();
        let operation_id = operation_id.to_owned();
        self.connection
            .write(move |connection| {
                connection.exec_bound::<(String, String)>(
                    r#"
                    UPDATE macro_mutation_outbox
                    SET state = 'in_flight', attempt_count = attempt_count + 1,
                        last_error = NULL
                    WHERE account_id = ? AND operation_id = ?
                    "#,
                )?((account_id, operation_id))?;
                anyhow::Ok(())
            })
            .await
            .context("failed to persist mutation delivery attempt")
    }

    pub async fn mark_failed(
        &self,
        account_id: &str,
        operation_id: &str,
        error: &str,
    ) -> Result<()> {
        let account_id = account_id.to_owned();
        let operation_id = operation_id.to_owned();
        let error = error.chars().take(2_048).collect::<String>();
        self.connection
            .write(move |connection| {
                connection.exec_bound::<(String, String, String)>(
                    r#"
                    UPDATE macro_mutation_outbox
                    SET state = 'failed', last_error = ?
                    WHERE account_id = ? AND operation_id = ?
                    "#,
                )?((error, account_id, operation_id))?;
                anyhow::Ok(())
            })
            .await
            .context("failed to persist mutation failure")
    }

    pub async fn mark_exhausted(
        &self,
        account_id: &str,
        operation_id: &str,
        error: Option<&str>,
    ) -> Result<()> {
        let account_id = account_id.to_owned();
        let operation_id = operation_id.to_owned();
        let error = error.map(|error| error.chars().take(2_048).collect::<String>());
        self.connection
            .write(move |connection| {
                connection.exec_bound::<(Option<String>, String, String)>(
                    r#"
                    UPDATE macro_mutation_outbox
                    SET state = 'exhausted', last_error = COALESCE(?, last_error)
                    WHERE account_id = ? AND operation_id = ?
                    "#,
                )?((error, account_id, operation_id))?;
                anyhow::Ok(())
            })
            .await
            .context("failed to persist exhausted mutation")
    }

    pub async fn reset(&self, account_id: &str, operation_id: &str) -> Result<()> {
        let account_id = account_id.to_owned();
        let operation_id = operation_id.to_owned();
        self.connection
            .write(move |connection| {
                connection.exec_bound::<(String, String)>(
                    r#"
                    UPDATE macro_mutation_outbox
                    SET state = 'queued', attempt_count = 0, last_error = NULL
                    WHERE account_id = ? AND operation_id = ?
                    "#,
                )?((account_id, operation_id))?;
                anyhow::Ok(())
            })
            .await
            .context("failed to reset persisted mutation retry state")
    }

    pub async fn acknowledge(&self, account_id: &str, operation_id: &str) -> Result<()> {
        let account_id = account_id.to_owned();
        let operation_id = operation_id.to_owned();
        self.connection
            .write(move |connection| {
                connection.exec_bound::<(String, String)>(
                    "DELETE FROM macro_mutation_outbox WHERE account_id = ? AND operation_id = ?",
                )?((account_id, operation_id))?;
                anyhow::Ok(())
            })
            .await
            .context("failed to acknowledge persisted mutation")
    }

    pub async fn clear_account(&self, account_id: &str) -> Result<()> {
        let account_id = account_id.to_owned();
        self.connection
            .write(move |connection| {
                connection.exec_bound::<String>(
                    "DELETE FROM macro_mutation_outbox WHERE account_id = ?",
                )?(account_id)?;
                anyhow::Ok(())
            })
            .await
            .context("failed to clear persisted account mutations")
    }

    pub fn pending(&self, account_id: &str) -> Result<Vec<StoredMutation>> {
        let rows = self
            .connection
            .select_bound::<&str, (String, Vec<u8>, Vec<u8>, String, i64, Option<String>, i64)>(
                r#"
            SELECT operation_id, nonce, ciphertext, state, attempt_count,
                   last_error, created_at_ms
            FROM macro_mutation_outbox
            WHERE account_id = ?
            ORDER BY sequence
            "#,
            )?(account_id)?;
        rows.into_iter()
            .map(
                |(
                    operation_id,
                    nonce,
                    ciphertext,
                    state,
                    attempt_count,
                    last_error,
                    created_at_ms,
                )| {
                    let payload =
                        self.open_payload(account_id, &operation_id, nonce, ciphertext)?;
                    Ok(StoredMutation {
                        operation_id,
                        payload,
                        state: delivery_state(&state)?,
                        attempt_count: attempt_count.max(0) as u32,
                        last_error,
                        created_at_ms,
                    })
                },
            )
            .collect()
    }

    fn seal(
        &self,
        account_id: &str,
        operation_id: &str,
        mut payload: Vec<u8>,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        let mut nonce_bytes = [0_u8; NONCE_LEN];
        SystemRandom::new()
            .fill(&mut nonce_bytes)
            .map_err(|_| anyhow::anyhow!("failed to generate mutation nonce"))?;
        let aad = mutation_aad(account_id, operation_id);
        self.key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::from(aad.as_bytes()),
                &mut payload,
            )
            .map_err(|_| anyhow::anyhow!("failed to encrypt mutation payload"))?;
        Ok((nonce_bytes.to_vec(), payload))
    }

    fn open_payload(
        &self,
        account_id: &str,
        operation_id: &str,
        nonce: Vec<u8>,
        mut ciphertext: Vec<u8>,
    ) -> Result<Vec<u8>> {
        let nonce: [u8; NONCE_LEN] = nonce
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid persisted mutation nonce"))?;
        let aad = mutation_aad(account_id, operation_id);
        let plaintext = self
            .key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad.as_bytes()),
                &mut ciphertext,
            )
            .map_err(|_| anyhow::anyhow!("persisted mutation authentication failed"))?;
        Ok(plaintext.to_vec())
    }
}

fn mutation_aad(account_id: &str, operation_id: &str) -> String {
    format!("macro-gpui-outbox-v1\0{account_id}\0{operation_id}")
}

fn delivery_state(value: &str) -> Result<MutationDeliveryState> {
    match value {
        "queued" => Ok(MutationDeliveryState::Queued),
        "in_flight" => Ok(MutationDeliveryState::InFlight),
        "failed" => Ok(MutationDeliveryState::Failed),
        "exhausted" => Ok(MutationDeliveryState::Exhausted),
        other => bail!("unknown persisted mutation state {other:?}"),
    }
}

pub trait Identified {
    fn id(&self) -> &str;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadStatus {
    Idle,
    Loading,
    Ready,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct CollectionSnapshot<T> {
    pub status: LoadStatus,
    pub items: Vec<Arc<T>>,
    pub next_cursor: Option<String>,
    pub previous_cursor: Option<String>,
    pub refreshing: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NormalizedCollection<T> {
    records: HashMap<String, Arc<T>>,
    ordered_ids: Vec<String>,
    next_cursor: Option<String>,
    previous_cursor: Option<String>,
    status: LoadStatus,
    refreshing: bool,
    last_error: Option<String>,
}

impl<T> Default for NormalizedCollection<T> {
    fn default() -> Self {
        Self {
            records: HashMap::new(),
            ordered_ids: Vec::new(),
            next_cursor: None,
            previous_cursor: None,
            status: LoadStatus::Idle,
            refreshing: false,
            last_error: None,
        }
    }
}

impl<T> NormalizedCollection<T>
where
    T: Identified + PartialEq,
{
    pub fn status(&self) -> &LoadStatus {
        &self.status
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.status, LoadStatus::Ready)
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.status, LoadStatus::Loading) || self.refreshing
    }

    pub fn is_empty(&self) -> bool {
        self.ordered_ids.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ordered_ids.len()
    }

    pub fn next_cursor(&self) -> Option<&str> {
        self.next_cursor.as_deref()
    }

    pub fn previous_cursor(&self) -> Option<&str> {
        self.previous_cursor.as_deref()
    }

    pub fn begin_load(&mut self, preserve_ready_data: bool) {
        self.last_error = None;
        if preserve_ready_data && self.is_ready() {
            self.refreshing = true;
        } else {
            self.status = LoadStatus::Loading;
            self.refreshing = false;
        }
    }

    pub fn replace(&mut self, values: Vec<T>, next_cursor: Option<String>) {
        self.replace_window(values, next_cursor, None);
    }

    /// Replaces a centered or ordinary page while retaining cursors in both
    /// directions. Ordinary one-directional collections use `replace`.
    pub fn replace_window(
        &mut self,
        values: Vec<T>,
        next_cursor: Option<String>,
        previous_cursor: Option<String>,
    ) {
        let mut records = HashMap::with_capacity(values.len());
        let mut ordered_ids = Vec::with_capacity(values.len());
        for value in values {
            let id = value.id().to_owned();
            if records.contains_key(&id) {
                continue;
            }
            let value = match self.records.get(&id) {
                Some(existing) if existing.as_ref() == &value => Arc::clone(existing),
                _ => Arc::new(value),
            };
            ordered_ids.push(id.clone());
            records.insert(id, value);
        }
        self.records = records;
        self.ordered_ids = ordered_ids;
        self.next_cursor = next_cursor;
        self.previous_cursor = previous_cursor;
        self.status = LoadStatus::Ready;
        self.refreshing = false;
        self.last_error = None;
    }

    pub fn append(&mut self, values: Vec<T>, next_cursor: Option<String>) {
        let mut known: HashSet<String> = self.ordered_ids.iter().cloned().collect();
        for value in values {
            let id = value.id().to_owned();
            let record = match self.records.get(&id) {
                Some(existing) if existing.as_ref() == &value => Arc::clone(existing),
                _ => Arc::new(value),
            };
            self.records.insert(id.clone(), record);
            if known.insert(id.clone()) {
                self.ordered_ids.push(id);
            }
        }
        self.next_cursor = next_cursor;
        self.status = LoadStatus::Ready;
        self.refreshing = false;
        self.last_error = None;
    }

    /// Prepends a newer page to a newest-first collection. Existing IDs keep
    /// their current position, and the server-owned older-page cursor remains
    /// unchanged.
    pub fn prepend(&mut self, values: Vec<T>, previous_cursor: Option<String>) {
        let mut incoming_ids = Vec::with_capacity(values.len());
        let mut incoming_set = HashSet::with_capacity(values.len());
        for value in values {
            let id = value.id().to_owned();
            let record = match self.records.get(&id) {
                Some(existing) if existing.as_ref() == &value => Arc::clone(existing),
                _ => Arc::new(value),
            };
            self.records.insert(id.clone(), record);
            if incoming_set.insert(id.clone()) && !self.ordered_ids.contains(&id) {
                incoming_ids.push(id);
            }
        }
        incoming_ids.append(&mut self.ordered_ids);
        self.ordered_ids = incoming_ids;
        self.previous_cursor = previous_cursor;
        self.status = LoadStatus::Ready;
        self.refreshing = false;
        self.last_error = None;
    }

    pub fn fail(&mut self, error: impl Into<String>) {
        let error = error.into();
        if self.is_ready() && !self.ordered_ids.is_empty() {
            self.refreshing = false;
            self.last_error = Some(error);
        } else {
            self.status = LoadStatus::Failed(error);
            self.refreshing = false;
        }
    }

    pub fn get(&self, id: &str) -> Option<Arc<T>> {
        self.records.get(id).cloned()
    }

    /// Replaces one normalized record without disturbing query order. New IDs
    /// are appended because this store cannot infer a server-owned sort key.
    pub fn upsert(&mut self, value: T) {
        let id = value.id().to_owned();
        let value = match self.records.get(&id) {
            Some(existing) if existing.as_ref() == &value => Arc::clone(existing),
            _ => Arc::new(value),
        };
        if !self.records.contains_key(&id) {
            self.ordered_ids.push(id.clone());
        }
        self.records.insert(id, value);
    }

    /// Replaces one normalized record and places newly observed IDs at the
    /// front of the server-owned order. This is useful for newest-first live
    /// feeds, where an insert arriving over realtime has a known position.
    pub fn upsert_front(&mut self, value: T) {
        let id = value.id().to_owned();
        let value = match self.records.get(&id) {
            Some(existing) if existing.as_ref() == &value => Arc::clone(existing),
            _ => Arc::new(value),
        };
        if !self.records.contains_key(&id) {
            self.ordered_ids.insert(0, id.clone());
        }
        self.records.insert(id, value);
    }

    /// Applies a typed record patch in place while preserving the collection's
    /// server-owned ordering and pagination metadata.
    pub fn update(&mut self, id: &str, patch: impl FnOnce(&T) -> T) -> bool {
        let Some(existing) = self.records.get(id) else {
            return false;
        };
        let updated = patch(existing.as_ref());
        let record = if existing.as_ref() == &updated {
            Arc::clone(existing)
        } else {
            Arc::new(updated)
        };
        self.records.insert(id.to_owned(), record);
        true
    }

    pub fn remove(&mut self, id: &str) -> Option<Arc<T>> {
        let removed = self.records.remove(id)?;
        self.ordered_ids.retain(|ordered_id| ordered_id != id);
        Some(removed)
    }

    pub fn snapshot(&self) -> CollectionSnapshot<T> {
        CollectionSnapshot {
            status: self.status.clone(),
            items: self
                .ordered_ids
                .iter()
                .filter_map(|id| self.records.get(id).cloned())
                .collect(),
            next_cursor: self.next_cursor.clone(),
            previous_cursor: self.previous_cursor.clone(),
            refreshing: self.refreshing,
            last_error: self.last_error.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    struct Item {
        id: String,
        value: u32,
    }

    impl Identified for Item {
        fn id(&self) -> &str {
            &self.id
        }
    }

    fn item(id: &str, value: u32) -> Item {
        Item {
            id: id.into(),
            value,
        }
    }

    #[test]
    fn replacement_deduplicates_ids_and_preserves_unchanged_arcs() {
        let mut store = NormalizedCollection::default();
        store.replace(vec![item("a", 1), item("b", 2)], Some("next".into()));
        let original = store.get("a").unwrap();

        store.begin_load(true);
        store.replace(vec![item("a", 1), item("a", 99), item("c", 3)], None);

        assert!(Arc::ptr_eq(&original, &store.get("a").unwrap()));
        assert_eq!(store.snapshot().items.len(), 2);
        assert!(store.get("b").is_none());
    }

    #[test]
    fn pagination_updates_existing_records_without_duplicating_order() {
        let mut store = NormalizedCollection::default();
        store.replace(vec![item("a", 1)], Some("one".into()));
        store.append(vec![item("a", 2), item("b", 3)], Some("two".into()));

        let snapshot = store.snapshot();
        assert_eq!(snapshot.items.len(), 2);
        assert_eq!(snapshot.items[0].value, 2);
        assert_eq!(snapshot.next_cursor.as_deref(), Some("two"));
    }

    #[test]
    fn realtime_front_insert_preserves_existing_order_and_cursor() {
        let mut store = NormalizedCollection::default();
        store.replace(
            vec![item("newer", 2), item("older", 1)],
            Some("next".into()),
        );

        store.upsert_front(item("newest", 3));
        store.upsert_front(item("older", 4));

        let snapshot = store.snapshot();
        assert_eq!(
            snapshot
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["newest", "newer", "older"]
        );
        assert_eq!(snapshot.items[2].value, 4);
        assert_eq!(snapshot.next_cursor.as_deref(), Some("next"));
    }

    #[test]
    fn centered_windows_page_in_both_directions_without_duplicates() {
        let mut store = NormalizedCollection::default();
        store.replace_window(
            vec![item("middle-new", 3), item("middle-old", 2)],
            Some("older".into()),
            Some("newer".into()),
        );
        store.prepend(vec![item("newest", 4), item("middle-new", 30)], None);
        store.append(vec![item("middle-old", 20), item("oldest", 1)], None);

        let snapshot = store.snapshot();
        assert_eq!(
            snapshot
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["newest", "middle-new", "middle-old", "oldest"]
        );
        assert_eq!(snapshot.items[1].value, 30);
        assert_eq!(snapshot.items[2].value, 20);
        assert!(snapshot.next_cursor.is_none());
        assert!(snapshot.previous_cursor.is_none());
    }

    #[test]
    fn background_refresh_failure_keeps_renderable_data() {
        let mut store = NormalizedCollection::default();
        store.replace(vec![item("a", 1)], None);
        store.begin_load(true);
        store.fail("offline");

        assert_eq!(store.status(), &LoadStatus::Ready);
        assert_eq!(store.snapshot().last_error.as_deref(), Some("offline"));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn targeted_updates_and_removals_preserve_query_metadata() {
        let mut store = NormalizedCollection::default();
        store.replace(vec![item("a", 1), item("b", 2)], Some("next".into()));
        assert!(store.update("a", |_| item("a", 9)));
        assert!(!store.update("missing", |_| item("missing", 1)));
        assert_eq!(store.get("a").unwrap().value, 9);
        assert_eq!(store.snapshot().next_cursor.as_deref(), Some("next"));
        assert_eq!(store.remove("a").unwrap().value, 9);
        assert_eq!(
            store
                .snapshot()
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["b"]
        );
    }

    #[test]
    fn mutation_outbox_is_idempotent_retryable_and_acknowledged_once() {
        let mut outbox = MutationOutbox::default();
        assert!(outbox.enqueue("nonce-1", "payload".to_owned()));
        assert!(!outbox.enqueue("nonce-1", "duplicate".to_owned()));
        assert_eq!(outbox.len(), 1);

        assert_eq!(
            outbox.begin_attempt("nonce-1", 2).as_deref(),
            Some("payload")
        );
        assert!(outbox.begin_attempt("nonce-1", 2).is_none());
        assert!(outbox.fail("nonce-1", "offline"));
        assert_eq!(
            outbox.retryable(),
            vec![("nonce-1".into(), "payload".into())]
        );
        assert_eq!(
            outbox.begin_attempt("nonce-1", 2).as_deref(),
            Some("payload")
        );
        assert!(outbox.fail("nonce-1", "offline again"));
        assert!(outbox.begin_attempt("nonce-1", 2).is_none());
        assert_eq!(
            outbox.get("nonce-1").map(|entry| entry.state),
            Some(MutationDeliveryState::Exhausted)
        );

        assert!(outbox.reset("nonce-1"));
        assert_eq!(
            outbox.begin_attempt("nonce-1", 2).as_deref(),
            Some("payload")
        );
        assert!(outbox.acknowledge("nonce-1").is_some());
        assert!(outbox.acknowledge("nonce-1").is_none());
        assert!(outbox.is_empty());
    }

    #[test]
    fn disconnected_in_flight_mutations_requeue_in_original_order() {
        let mut outbox = MutationOutbox::default();
        outbox.enqueue("one", 1_u8);
        outbox.enqueue("two", 2_u8);
        assert_eq!(outbox.begin_attempt("one", 3), Some(1));
        assert_eq!(outbox.begin_attempt("two", 3), Some(2));
        outbox.requeue_in_flight();
        assert_eq!(
            outbox.retryable(),
            vec![("one".into(), 1), ("two".into(), 2)]
        );
    }

    #[test]
    fn restored_in_flight_mutations_keep_attempts_and_become_retryable() {
        let mut outbox = MutationOutbox::default();
        assert!(outbox.restore(PendingMutation {
            operation_id: "op-1".into(),
            payload: "message".to_owned(),
            state: MutationDeliveryState::InFlight,
            attempt_count: 2,
            last_error: None,
        }));
        assert_eq!(outbox.retryable(), vec![("op-1".into(), "message".into())]);
        assert_eq!(outbox.get("op-1").unwrap().attempt_count, 2);
        assert_eq!(outbox.begin_attempt("op-1", 5).as_deref(), Some("message"));
        assert_eq!(outbox.get("op-1").unwrap().attempt_count, 3);
    }

    #[test]
    fn encrypted_persistence_round_trips_without_plaintext_at_rest() {
        futures::executor::block_on(async {
            let key = [7_u8; 32];
            let persistence =
                EncryptedMutationPersistence::open_memory("macro-store-encrypted-outbox", key)
                    .await
                    .unwrap();
            let plaintext = br#"{"content":"private channel message"}"#.to_vec();
            persistence
                .enqueue("account-a", "op-1", plaintext.clone(), 10)
                .await
                .unwrap();

            let raw = persistence
                .connection
                .select_row::<(Vec<u8>, Vec<u8>)>(
                    "SELECT nonce, ciphertext FROM macro_mutation_outbox WHERE operation_id = 'op-1'",
                )
                .unwrap()()
                .unwrap()
                .unwrap();
            assert_eq!(raw.0.len(), NONCE_LEN);
            assert_ne!(raw.1, plaintext);
            assert!(!String::from_utf8_lossy(&raw.1).contains("private channel message"));

            persistence
                .record_attempt("account-a", "op-1")
                .await
                .unwrap();
            persistence
                .mark_failed("account-a", "op-1", "offline")
                .await
                .unwrap();
            let pending = persistence.pending("account-a").unwrap();
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].payload, plaintext);
            assert_eq!(pending[0].state, MutationDeliveryState::Failed);
            assert_eq!(pending[0].attempt_count, 1);
            assert_eq!(pending[0].last_error.as_deref(), Some("offline"));

            persistence.acknowledge("account-a", "op-1").await.unwrap();
            assert!(persistence.pending("account-a").unwrap().is_empty());
        });
    }

    #[test]
    fn encrypted_payload_is_bound_to_account_and_operation_identity() {
        futures::executor::block_on(async {
            let persistence =
                EncryptedMutationPersistence::open_memory("macro-store-encrypted-aad", [9_u8; 32])
                    .await
                    .unwrap();
            persistence
                .enqueue("account-a", "op-1", b"payload".to_vec(), 10)
                .await
                .unwrap();
            persistence
                .connection
                .write(|connection| {
                    connection.exec_bound::<String>(
                        "UPDATE macro_mutation_outbox SET operation_id = ? WHERE operation_id = 'op-1'",
                    )?("op-tampered".into())?;
                    anyhow::Ok(())
                })
                .await
                .unwrap();
            assert!(persistence.pending("account-a").is_err());
        });
    }

    #[test]
    fn exhausted_delivery_state_survives_restart_and_explicit_reset() {
        futures::executor::block_on(async {
            let persistence = EncryptedMutationPersistence::open_memory(
                "macro-store-exhausted-delivery",
                [11_u8; 32],
            )
            .await
            .unwrap();
            persistence
                .enqueue("account-a", "op-1", b"payload".to_vec(), 10)
                .await
                .unwrap();
            persistence
                .record_attempt("account-a", "op-1")
                .await
                .unwrap();
            persistence
                .mark_exhausted("account-a", "op-1", Some("retry limit reached"))
                .await
                .unwrap();

            let pending = persistence.pending("account-a").unwrap();
            assert_eq!(pending[0].state, MutationDeliveryState::Exhausted);
            assert_eq!(pending[0].attempt_count, 1);
            assert_eq!(
                pending[0].last_error.as_deref(),
                Some("retry limit reached")
            );

            persistence.reset("account-a", "op-1").await.unwrap();
            let reset = persistence.pending("account-a").unwrap();
            assert_eq!(reset[0].state, MutationDeliveryState::Queued);
            assert_eq!(reset[0].attempt_count, 0);
            assert!(reset[0].last_error.is_none());
        });
    }
}
