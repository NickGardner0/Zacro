use anyhow::{Context as _, Result, bail};
use sqlez::{domain::Domain, thread_safe_connection::ThreadSafeConnection};
use std::path::Path;

use crate::PeerMessage;

/// Macro retains undelivered document edits for one week before treating them
/// as irrecoverably stale.
pub const WAL_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1_000;

pub enum MacroSyncDomain {}

impl Domain for MacroSyncDomain {
    const NAME: &str = "macro_sync";
    const MIGRATIONS: &[&str] = &[r#"
            CREATE TABLE macro_sync_wal (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                scope_id TEXT NOT NULL,
                operation_id TEXT NOT NULL UNIQUE,
                encoded_frame BLOB NOT NULL,
                created_at_ms INTEGER NOT NULL,
                acked_at_ms INTEGER,
                attempt_count INTEGER NOT NULL DEFAULT 0
            ) STRICT;
            CREATE INDEX macro_sync_wal_pending
                ON macro_sync_wal(scope_id, acked_at_ms, sequence);

            CREATE TABLE macro_sync_snapshots (
                scope_id TEXT PRIMARY KEY,
                snapshot BLOB NOT NULL,
                awareness BLOB NOT NULL,
                version_vector BLOB NOT NULL,
                updated_at_ms INTEGER NOT NULL
            ) STRICT;
        "#];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingUpdate {
    pub sequence: i64,
    pub scope_id: String,
    pub operation_id: String,
    pub encoded_frame: Vec<u8>,
    pub created_at_ms: i64,
    pub attempt_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSnapshot {
    pub scope_id: String,
    pub snapshot: Vec<u8>,
    pub awareness: Vec<u8>,
    pub version_vector: Vec<u8>,
    pub updated_at_ms: i64,
}

/// Cached CRDT state used to make a document available before the network is
/// ready. Pending update bytes are replayed in WAL sequence order over the
/// snapshot, matching Macro's `loadCachedState` contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredScope {
    pub snapshot: Option<StoredSnapshot>,
    pub pending_updates: Vec<Vec<u8>>,
    pub pending_operation_ids: Vec<String>,
}

#[derive(Clone)]
pub struct SyncPersistence {
    connection: ThreadSafeConnection,
}

impl SyncPersistence {
    pub async fn open(path: &Path) -> Result<Self> {
        let path = path
            .to_str()
            .context("Macro sync database path is not valid UTF-8")?;
        let connection = ThreadSafeConnection::builder::<MacroSyncDomain>(path, true)
            .with_db_initialization_query("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .with_connection_initialize_query("PRAGMA busy_timeout=5000;")
            .build()
            .await
            .context("failed to initialize Macro sync persistence")?;
        Ok(Self { connection })
    }

    #[cfg(test)]
    async fn open_memory(name: &str) -> Result<Self> {
        let connection = ThreadSafeConnection::builder::<MacroSyncDomain>(name, false)
            .with_connection_initialize_query("PRAGMA busy_timeout=5000;")
            .build()
            .await?;
        Ok(Self { connection })
    }

    /// Durably records an outbound update before it may be sent. Reusing an
    /// operation ID is idempotent, which makes retrying the UI-to-sync handoff
    /// safe after an uncertain process boundary.
    pub async fn enqueue_update(
        &self,
        scope_id: &str,
        operation_id: &str,
        updates: Vec<Vec<u8>>,
        created_at_ms: i64,
    ) -> Result<()> {
        let frame = PeerMessage::Update {
            updates,
            id: operation_id.to_owned(),
        }
        .encode()
        .context("failed to encode outbound sync update")?;
        let scope_id = scope_id.to_owned();
        let operation_id = operation_id.to_owned();
        self.connection
            .write(move |connection| {
                connection.exec_bound::<(String, String, Vec<u8>, i64)>(
                    r#"
                    INSERT OR IGNORE INTO macro_sync_wal
                        (scope_id, operation_id, encoded_frame, created_at_ms)
                    VALUES (?, ?, ?, ?)
                    "#,
                )?((scope_id, operation_id, frame, created_at_ms))?;
                anyhow::Ok(())
            })
            .await
            .context("failed to append Macro sync WAL entry")
    }

    pub fn pending_updates(&self, scope_id: &str) -> Result<Vec<PendingUpdate>> {
        let rows = self
            .connection
            .select_bound::<&str, (i64, String, String, Vec<u8>, i64, i64)>(
                r#"
                SELECT sequence, scope_id, operation_id, encoded_frame,
                       created_at_ms, attempt_count
                FROM macro_sync_wal
                WHERE scope_id = ? AND acked_at_ms IS NULL
                ORDER BY sequence
                "#,
            )?(scope_id)?;
        Ok(rows
            .into_iter()
            .map(
                |(
                    sequence,
                    scope_id,
                    operation_id,
                    encoded_frame,
                    created_at_ms,
                    attempt_count,
                )| PendingUpdate {
                    sequence,
                    scope_id,
                    operation_id,
                    encoded_frame,
                    created_at_ms,
                    attempt_count,
                },
            )
            .collect())
    }

    pub async fn record_attempt(&self, operation_id: &str) -> Result<()> {
        let operation_id = operation_id.to_owned();
        self.connection
            .write(move |connection| {
                connection.exec_bound::<String>(
                    "UPDATE macro_sync_wal SET attempt_count = attempt_count + 1 WHERE operation_id = ? AND acked_at_ms IS NULL",
                )?(operation_id)?;
                anyhow::Ok(())
            })
            .await
            .context("failed to record Macro sync delivery attempt")
    }

    pub async fn mark_acked(&self, operation_id: &str, acked_at_ms: i64) -> Result<()> {
        let operation_id = operation_id.to_owned();
        self.connection
            .write(move |connection| {
                connection.exec_bound::<(i64, String)>(
                    "UPDATE macro_sync_wal SET acked_at_ms = COALESCE(acked_at_ms, ?) WHERE operation_id = ?",
                )?((acked_at_ms, operation_id))?;
                anyhow::Ok(())
            })
            .await
            .context("failed to acknowledge Macro sync WAL entry")
    }

    pub async fn prune_acked_before(&self, timestamp_ms: i64) -> Result<usize> {
        self.connection
            .write(move |connection| {
                connection.exec_bound::<i64>(
                    "DELETE FROM macro_sync_wal WHERE acked_at_ms IS NOT NULL AND acked_at_ms < ?",
                )?(timestamp_ms)?;
                let changed =
                    connection.select_row::<i64>("SELECT changes()")?()?.unwrap_or_default();
                anyhow::Ok(changed.max(0) as usize)
            })
            .await
            .context("failed to prune acknowledged Macro sync WAL entries")
    }

    pub async fn save_snapshot(&self, snapshot: StoredSnapshot) -> Result<()> {
        self.connection
            .write(move |connection| {
                connection.exec_bound::<(String, Vec<u8>, Vec<u8>, Vec<u8>, i64)>(
                    r#"
                    INSERT INTO macro_sync_snapshots
                        (scope_id, snapshot, awareness, version_vector, updated_at_ms)
                    VALUES (?, ?, ?, ?, ?)
                    ON CONFLICT(scope_id) DO UPDATE SET
                        snapshot = excluded.snapshot,
                        awareness = excluded.awareness,
                        version_vector = excluded.version_vector,
                        updated_at_ms = excluded.updated_at_ms
                    "#,
                )?((
                    snapshot.scope_id,
                    snapshot.snapshot,
                    snapshot.awareness,
                    snapshot.version_vector,
                    snapshot.updated_at_ms,
                ))?;
                anyhow::Ok(())
            })
            .await
            .context("failed to save Macro sync snapshot")
    }

    pub fn load_snapshot(&self, scope_id: &str) -> Result<Option<StoredSnapshot>> {
        self.connection
            .select_row_bound::<&str, (String, Vec<u8>, Vec<u8>, Vec<u8>, i64)>(
                r#"
                SELECT scope_id, snapshot, awareness, version_vector, updated_at_ms
                FROM macro_sync_snapshots WHERE scope_id = ?
                "#,
            )?(scope_id)
        .map(|row| {
            row.map(
                |(scope_id, snapshot, awareness, version_vector, updated_at_ms)| StoredSnapshot {
                    scope_id,
                    snapshot,
                    awareness,
                    version_vector,
                    updated_at_ms,
                },
            )
        })
    }

    /// Load the last durable snapshot and decode all unacknowledged outbound
    /// Loro updates. The encoded WAL frame remains untouched, so subsequent
    /// delivery still uses the original operation ID and is idempotent.
    pub fn recover_scope(&self, scope_id: &str) -> Result<RecoveredScope> {
        let snapshot = self.load_snapshot(scope_id)?;
        let pending = self.pending_updates(scope_id)?;
        let mut pending_updates = Vec::new();
        let mut pending_operation_ids = Vec::with_capacity(pending.len());
        for entry in pending {
            let decoded = PeerMessage::decode(&entry.encoded_frame).with_context(|| {
                format!("failed to decode WAL operation {}", entry.operation_id)
            })?;
            let PeerMessage::Update { updates, id } = decoded else {
                bail!("Macro sync WAL contains a non-update frame");
            };
            if id != entry.operation_id {
                bail!(
                    "Macro sync WAL operation ID mismatch: row {} encoded {}",
                    entry.operation_id,
                    id
                );
            }
            pending_operation_ids.push(id);
            pending_updates.extend(updates);
        }
        Ok(RecoveredScope {
            snapshot,
            pending_updates,
            pending_operation_ids,
        })
    }

    /// Drop edits that have remained undelivered past Macro's one-week WAL
    /// retention window. Acknowledged rows are pruned only after a newer
    /// snapshot has been saved, so this function never discards recoverable
    /// delivered state.
    pub async fn prune_expired_undelivered_before(&self, timestamp_ms: i64) -> Result<usize> {
        self.connection
            .write(move |connection| {
                connection.exec_bound::<i64>(
                    "DELETE FROM macro_sync_wal WHERE acked_at_ms IS NULL AND created_at_ms < ?",
                )?(timestamp_ms)?;
                let changed =
                    connection.select_row::<i64>("SELECT changes()")?()?.unwrap_or_default();
                anyhow::Ok(changed.max(0) as usize)
            })
            .await
            .context("failed to prune expired Macro sync WAL entries")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DB: AtomicU64 = AtomicU64::new(1);

    fn run<T>(future: impl Future<Output = T>) -> T {
        futures::executor::block_on(future)
    }

    fn db_name(test: &str) -> String {
        format!(
            "macro-sync-{test}-{}",
            NEXT_DB.fetch_add(1, Ordering::Relaxed)
        )
    }

    #[test]
    fn wal_is_ordered_idempotent_and_acknowledged() {
        run(async {
            let db = SyncPersistence::open_memory(&db_name("wal")).await.unwrap();
            db.enqueue_update("doc-a", "op-1", vec![vec![1]], 10)
                .await
                .unwrap();
            db.enqueue_update("doc-a", "op-1", vec![vec![99]], 11)
                .await
                .unwrap();
            db.enqueue_update("doc-a", "op-2", vec![vec![2]], 12)
                .await
                .unwrap();
            db.enqueue_update("doc-b", "op-3", vec![vec![3]], 13)
                .await
                .unwrap();

            let pending = db.pending_updates("doc-a").unwrap();
            assert_eq!(
                pending
                    .iter()
                    .map(|entry| entry.operation_id.as_str())
                    .collect::<Vec<_>>(),
                ["op-1", "op-2"]
            );
            db.record_attempt("op-1").await.unwrap();
            assert_eq!(db.pending_updates("doc-a").unwrap()[0].attempt_count, 1);
            db.mark_acked("op-1", 20).await.unwrap();
            assert_eq!(db.pending_updates("doc-a").unwrap().len(), 1);
            assert_eq!(db.prune_acked_before(21).await.unwrap(), 1);
        });
    }

    #[test]
    fn snapshot_round_trips_and_is_replaced_atomically() {
        run(async {
            let db = SyncPersistence::open_memory(&db_name("snapshot"))
                .await
                .unwrap();
            let first = StoredSnapshot {
                scope_id: "doc-a".into(),
                snapshot: vec![1],
                awareness: vec![2],
                version_vector: vec![3],
                updated_at_ms: 10,
            };
            db.save_snapshot(first.clone()).await.unwrap();
            assert_eq!(db.load_snapshot("doc-a").unwrap(), Some(first));

            let replacement = StoredSnapshot {
                scope_id: "doc-a".into(),
                snapshot: vec![4],
                awareness: vec![],
                version_vector: vec![5],
                updated_at_ms: 20,
            };
            db.save_snapshot(replacement.clone()).await.unwrap();
            assert_eq!(db.load_snapshot("doc-a").unwrap(), Some(replacement));
        });
    }

    #[test]
    fn cold_recovery_replays_pending_updates_in_order_without_changing_ids() {
        run(async {
            let db = SyncPersistence::open_memory(&db_name("recover"))
                .await
                .unwrap();
            let snapshot = StoredSnapshot {
                scope_id: "doc-a".into(),
                snapshot: vec![7],
                awareness: vec![8],
                version_vector: vec![9],
                updated_at_ms: 10,
            };
            db.save_snapshot(snapshot.clone()).await.unwrap();
            db.enqueue_update("doc-a", "op-1", vec![vec![1], vec![2]], 11)
                .await
                .unwrap();
            db.enqueue_update("doc-a", "op-2", vec![vec![3]], 12)
                .await
                .unwrap();

            let recovered = db.recover_scope("doc-a").unwrap();
            assert_eq!(recovered.snapshot, Some(snapshot));
            assert_eq!(recovered.pending_updates, [vec![1], vec![2], vec![3]]);
            assert_eq!(recovered.pending_operation_ids, ["op-1", "op-2"]);
            assert_eq!(db.pending_updates("doc-a").unwrap().len(), 2);
        });
    }

    #[test]
    fn expired_undelivered_rows_are_pruned_but_delivered_rows_remain_for_snapshot() {
        run(async {
            let db = SyncPersistence::open_memory(&db_name("retention"))
                .await
                .unwrap();
            db.enqueue_update("doc-a", "expired", vec![vec![1]], 10)
                .await
                .unwrap();
            db.enqueue_update("doc-a", "delivered", vec![vec![2]], 11)
                .await
                .unwrap();
            db.mark_acked("delivered", 12).await.unwrap();

            assert_eq!(db.prune_expired_undelivered_before(20).await.unwrap(), 1);
            assert!(db.pending_updates("doc-a").unwrap().is_empty());
            assert_eq!(db.prune_acked_before(13).await.unwrap(), 1);
        });
    }
}
