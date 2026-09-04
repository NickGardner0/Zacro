use anyhow::{Context as _, Result, anyhow};
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::mpsc, task::JoinHandle, time::Instant};
use uuid::Uuid;

use crate::{
    InboundFrame, PeerMessage, RemoteMessage, StoredSnapshot, SyncPersistence, SyncServiceConfig,
    SyncTransport,
};

const INITIAL_SYNC_TIMEOUT: Duration = Duration::from_secs(10);
const ACK_TIMEOUT: Duration = Duration::from_secs(5);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_MISSED_HEARTBEATS: u8 = 2;

pub type TokenFuture<'a> = Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>>;

pub trait PermissionTokenProvider: Send + Sync + 'static {
    fn permission_token<'a>(&'a self, scope_id: &'a str) -> TokenFuture<'a>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncStatus {
    Disconnected,
    Connecting { attempt: u32 },
    AwaitingInitialSync,
    Connected,
    Reconnecting { attempt: u32, delay_ms: u64 },
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncEvent {
    Status(SyncStatus),
    InitialSync {
        snapshot: Vec<u8>,
        awareness: Vec<u8>,
    },
    Update(Vec<u8>),
    Awareness(Vec<u8>),
    Snapshot(Vec<u8>),
    UpdateAck(String),
    UpdateSince {
        update: Vec<u8>,
        version_vector: Vec<u8>,
    },
    UnknownMessage {
        discriminator: u8,
        payload: Vec<u8>,
    },
    RecoverableError(String),
}

enum SyncCommand {
    FlushPending,
    Awareness(Vec<u8>),
    RegisterPeerId(u64),
    RequestSnapshot,
    RequestSince(Vec<u8>),
    Stop,
}

#[derive(Clone)]
pub struct SyncSessionHandle {
    scope_id: Arc<str>,
    persistence: SyncPersistence,
    commands: mpsc::UnboundedSender<SyncCommand>,
}

pub struct SyncSessionEvents {
    events: mpsc::UnboundedReceiver<SyncEvent>,
    task: JoinHandle<()>,
}

impl SyncSessionHandle {
    pub fn spawn(
        scope_id: impl Into<Arc<str>>,
        config: SyncServiceConfig,
        persistence: SyncPersistence,
        token_provider: Arc<dyn PermissionTokenProvider>,
    ) -> (Self, SyncSessionEvents) {
        let scope_id = scope_id.into();
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (event_tx, events) = mpsc::unbounded_channel();
        let supervisor = SyncSupervisor {
            scope_id: scope_id.clone(),
            config,
            persistence: persistence.clone(),
            token_provider,
            commands: command_rx,
            events: event_tx,
        };
        let task = reqwest_client::runtime().spawn(supervisor.run());
        (
            Self {
                scope_id,
                persistence,
                commands,
            },
            SyncSessionEvents { events, task },
        )
    }

    /// Writes to disk before notifying the transport. Returning `Ok` means the
    /// edit survives a crash even if the socket is currently unavailable.
    pub async fn queue_update(&self, updates: Vec<Vec<u8>>) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        self.persistence
            .enqueue_update(&self.scope_id, &id, updates, unix_timestamp_ms())
            .await?;
        self.commands
            .send(SyncCommand::FlushPending)
            .map_err(|_| anyhow!("Macro sync session has stopped"))?;
        Ok(id)
    }

    pub fn push_awareness(&self, awareness: Vec<u8>) -> Result<()> {
        self.commands
            .send(SyncCommand::Awareness(awareness))
            .map_err(|_| anyhow!("Macro sync session has stopped"))
    }

    pub fn register_peer_id(&self, peer_id: u64) -> Result<()> {
        self.commands
            .send(SyncCommand::RegisterPeerId(peer_id))
            .map_err(|_| anyhow!("Macro sync session has stopped"))
    }

    pub fn request_snapshot(&self) -> Result<()> {
        self.commands
            .send(SyncCommand::RequestSnapshot)
            .map_err(|_| anyhow!("Macro sync session has stopped"))
    }

    pub fn request_updates_since(&self, version_vector: Vec<u8>) -> Result<()> {
        self.commands
            .send(SyncCommand::RequestSince(version_vector))
            .map_err(|_| anyhow!("Macro sync session has stopped"))
    }

    pub fn stop(&self) {
        let _ = self.commands.send(SyncCommand::Stop);
    }
}

impl SyncSessionEvents {
    pub async fn next_event(&mut self) -> Option<SyncEvent> {
        self.events.recv().await
    }
}

impl Drop for SyncSessionEvents {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct SyncSupervisor {
    scope_id: Arc<str>,
    config: SyncServiceConfig,
    persistence: SyncPersistence,
    token_provider: Arc<dyn PermissionTokenProvider>,
    commands: mpsc::UnboundedReceiver<SyncCommand>,
    events: mpsc::UnboundedSender<SyncEvent>,
}

impl SyncSupervisor {
    async fn run(mut self) {
        let mut attempt = 0u32;
        self.emit(SyncEvent::Status(SyncStatus::Disconnected));
        loop {
            attempt = attempt.saturating_add(1);
            self.emit(SyncEvent::Status(SyncStatus::Connecting { attempt }));
            let session_result = self.run_connection().await;
            match session_result {
                Ok(ConnectionEnd::Stop) => break,
                Ok(ConnectionEnd::Reconnect) => {}
                Err(error) => self.emit(SyncEvent::RecoverableError(format!("{error:#}"))),
            }

            let delay = reconnect_delay(attempt);
            self.emit(SyncEvent::Status(SyncStatus::Reconnecting {
                attempt,
                delay_ms: delay.as_millis() as u64,
            }));
            let sleep = tokio::time::sleep(delay);
            tokio::pin!(sleep);
            loop {
                tokio::select! {
                    _ = &mut sleep => break,
                    command = self.commands.recv() => match command {
                        Some(SyncCommand::Stop) | None => {
                            self.emit(SyncEvent::Status(SyncStatus::Stopped));
                            return;
                        }
                        // Updates are already durable. Ephemeral commands are
                        // intentionally dropped while there is no connection.
                        Some(_) => {}
                    }
                }
            }
        }
        self.emit(SyncEvent::Status(SyncStatus::Stopped));
    }

    async fn run_connection(&mut self) -> Result<ConnectionEnd> {
        let token = self
            .token_provider
            .permission_token(&self.scope_id)
            .await
            .context("failed to refresh document permission token")?;
        let url = self.config.connection_url(&self.scope_id, &token);
        let mut transport = SyncTransport::connect(url).await?;
        self.emit(SyncEvent::Status(SyncStatus::AwaitingInitialSync));

        let initial = tokio::time::timeout(INITIAL_SYNC_TIMEOUT, transport.recv())
            .await
            .context("timed out waiting for Macro initial sync")??;
        match initial {
            InboundFrame::Message(RemoteMessage::InitialSync {
                snapshot,
                awareness,
            }) => {
                self.persist_initial_snapshot(&snapshot, &awareness).await;
                self.emit(SyncEvent::InitialSync {
                    snapshot,
                    awareness,
                });
            }
            InboundFrame::Closed => return Ok(ConnectionEnd::Reconnect),
            other => return Err(anyhow!("expected initial sync frame, received {other:?}")),
        }

        self.emit(SyncEvent::Status(SyncStatus::Connected));
        let mut awaiting_ack = HashMap::<String, Instant>::new();
        self.flush_pending(&mut transport, &mut awaiting_ack)
            .await?;
        let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        heartbeat.tick().await;
        let mut ack_scan = tokio::time::interval(Duration::from_secs(1));
        ack_scan.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut pong_deadline: Option<Instant> = None;
        let mut missed_heartbeats = 0u8;

        loop {
            tokio::select! {
                command = self.commands.recv() => match command {
                    Some(SyncCommand::FlushPending) => {
                        self.flush_pending(&mut transport, &mut awaiting_ack).await?;
                    }
                    Some(SyncCommand::Awareness(awareness)) => {
                        transport.send(&PeerMessage::Awareness { awareness }).await?;
                    }
                    Some(SyncCommand::RegisterPeerId(peer_id)) => {
                        transport.send(&PeerMessage::RegisterPeerId { peer_id }).await?;
                    }
                    Some(SyncCommand::RequestSnapshot) => {
                        transport.send(&PeerMessage::RequestSnapshot).await?;
                    }
                    Some(SyncCommand::RequestSince(version_vector)) => {
                        transport.send(&PeerMessage::RequestSince { version_vector }).await?;
                    }
                    Some(SyncCommand::Stop) | None => {
                        let _ = transport.close().await;
                        return Ok(ConnectionEnd::Stop);
                    }
                },
                incoming = transport.recv() => match incoming? {
                    InboundFrame::Message(message) => {
                        self.handle_remote(message, &mut awaiting_ack).await;
                    }
                    InboundFrame::HeartbeatPong => {
                        pong_deadline = None;
                        missed_heartbeats = 0;
                    }
                    InboundFrame::Closed => return Ok(ConnectionEnd::Reconnect),
                },
                _ = heartbeat.tick() => {
                    if pong_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                        missed_heartbeats = missed_heartbeats.saturating_add(1);
                    }
                    if missed_heartbeats >= MAX_MISSED_HEARTBEATS {
                        return Ok(ConnectionEnd::Reconnect);
                    }
                    transport.heartbeat().await?;
                    pong_deadline = Some(Instant::now() + HEARTBEAT_TIMEOUT);
                }
                _ = ack_scan.tick() => {
                    let now = Instant::now();
                    if awaiting_ack.values().any(|deadline| now >= *deadline) {
                        self.flush_pending(&mut transport, &mut awaiting_ack).await?;
                    }
                }
            }
        }
    }

    async fn flush_pending(
        &self,
        transport: &mut SyncTransport,
        awaiting_ack: &mut HashMap<String, Instant>,
    ) -> Result<()> {
        for pending in self.persistence.pending_updates(&self.scope_id)? {
            if awaiting_ack
                .get(&pending.operation_id)
                .is_some_and(|deadline| Instant::now() < *deadline)
            {
                continue;
            }
            transport.send_encoded(pending.encoded_frame).await?;
            self.persistence
                .record_attempt(&pending.operation_id)
                .await?;
            awaiting_ack.insert(pending.operation_id, Instant::now() + ACK_TIMEOUT);
        }
        Ok(())
    }

    async fn handle_remote(
        &self,
        message: RemoteMessage,
        awaiting_ack: &mut HashMap<String, Instant>,
    ) {
        match message {
            RemoteMessage::InitialSync {
                snapshot,
                awareness,
            } => self.emit(SyncEvent::InitialSync {
                snapshot,
                awareness,
            }),
            RemoteMessage::Update { update } => self.emit(SyncEvent::Update(update)),
            RemoteMessage::Awareness { awareness } => self.emit(SyncEvent::Awareness(awareness)),
            RemoteMessage::Snapshot { snapshot } => self.emit(SyncEvent::Snapshot(snapshot)),
            RemoteMessage::UpdateAck { id } => {
                awaiting_ack.remove(&id);
                if let Err(error) = self.persistence.mark_acked(&id, unix_timestamp_ms()).await {
                    self.emit(SyncEvent::RecoverableError(format!(
                        "failed to persist sync acknowledgement: {error:#}"
                    )));
                }
                self.emit(SyncEvent::UpdateAck(id));
            }
            RemoteMessage::UpdateSince {
                update,
                version_vector,
            } => self.emit(SyncEvent::UpdateSince {
                update,
                version_vector,
            }),
            RemoteMessage::Unknown {
                discriminator,
                payload,
            } => self.emit(SyncEvent::UnknownMessage {
                discriminator,
                payload,
            }),
        }
    }

    async fn persist_initial_snapshot(&self, snapshot: &[u8], awareness: &[u8]) {
        let value = StoredSnapshot {
            scope_id: self.scope_id.to_string(),
            snapshot: snapshot.to_vec(),
            awareness: awareness.to_vec(),
            version_vector: Vec::new(),
            updated_at_ms: unix_timestamp_ms(),
        };
        if let Err(error) = self.persistence.save_snapshot(value).await {
            self.emit(SyncEvent::RecoverableError(format!(
                "failed to persist initial sync snapshot: {error:#}"
            )));
        }
    }

    fn emit(&self, event: SyncEvent) {
        let _ = self.events.send(event);
    }
}

enum ConnectionEnd {
    Stop,
    Reconnect,
}

fn reconnect_delay(attempt: u32) -> Duration {
    let exponent = attempt.clamp(1, 5);
    Duration::from_millis(250 * (1u64 << exponent))
}

fn unix_timestamp_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_tungstenite::{tokio::accept_async, tungstenite::Message};
    use futures::StreamExt as _;
    use tempfile::tempdir;
    use tokio::{net::TcpListener, sync::oneshot};

    struct StaticTokenProvider;

    impl PermissionTokenProvider for StaticTokenProvider {
        fn permission_token<'a>(&'a self, _scope_id: &'a str) -> TokenFuture<'a> {
            Box::pin(async { Ok("loopback-token".to_owned()) })
        }
    }

    async fn next_matching_event(
        events: &mut SyncSessionEvents,
        predicate: impl Fn(&SyncEvent) -> bool,
    ) -> SyncEvent {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let event = events
                    .next_event()
                    .await
                    .expect("sync event stream ended unexpectedly");
                if predicate(&event) {
                    break event;
                }
            }
        })
        .await
        .expect("timed out waiting for sync event")
    }

    #[test]
    fn backoff_matches_macro_socket_contract() {
        assert_eq!(reconnect_delay(1), Duration::from_millis(500));
        assert_eq!(reconnect_delay(2), Duration::from_secs(1));
        assert_eq!(reconnect_delay(5), Duration::from_secs(8));
        assert_eq!(reconnect_delay(100), Duration::from_secs(8));
    }

    #[tokio::test]
    async fn unacknowledged_wal_update_replays_exactly_after_websocket_reconnect() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (operation_tx, operation_rx) = oneshot::channel::<String>();
        let server = tokio::spawn(async move {
            let mut operation_rx = Some(operation_rx);
            let mut first_update = None::<Vec<u8>>;
            let mut operation_id = None::<String>;
            for connection_index in 0..2 {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = accept_async(stream).await.unwrap();
                let initial = RemoteMessage::InitialSync {
                    snapshot: vec![1, 2, 3],
                    awareness: vec![4, 5],
                }
                .encode_for_fixture()
                .unwrap();
                socket.send(Message::Binary(initial.into())).await.unwrap();

                let update = tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        match socket.next().await.unwrap().unwrap() {
                            Message::Binary(bytes) => break bytes.to_vec(),
                            Message::Text(text) if text == "ping" => {
                                socket.send(Message::Text("pong".into())).await.unwrap();
                            }
                            _ => {}
                        }
                    }
                })
                .await
                .expect("client did not flush its WAL update");

                if connection_index == 0 {
                    first_update = Some(update);
                    operation_id = Some(operation_rx.take().unwrap().await.unwrap());
                    socket.close(None).await.unwrap();
                    continue;
                }

                assert_eq!(Some(&update), first_update.as_ref());
                let id = operation_id.take().unwrap();
                socket
                    .send(Message::Binary(
                        RemoteMessage::UpdateAck { id }
                            .encode_for_fixture()
                            .unwrap()
                            .into(),
                    ))
                    .await
                    .unwrap();
                socket
                    .send(Message::Binary(
                        RemoteMessage::Update {
                            update: vec![9, 10],
                        }
                        .encode_for_fixture()
                        .unwrap()
                        .into(),
                    ))
                    .await
                    .unwrap();
                socket
                    .send(Message::Binary(
                        RemoteMessage::Awareness {
                            awareness: vec![11, 12],
                        }
                        .encode_for_fixture()
                        .unwrap()
                        .into(),
                    ))
                    .await
                    .unwrap();
                let _ = tokio::time::timeout(Duration::from_secs(2), socket.next()).await;
            }
        });

        let directory = tempdir().unwrap();
        let persistence = SyncPersistence::open(&directory.path().join("sync.sqlite"))
            .await
            .unwrap();
        let config = SyncServiceConfig {
            http_base_url: format!("http://{address}"),
            websocket_base_url: format!("ws://{address}"),
            origin: "http://localhost".into(),
        };
        let (handle, mut events) = SyncSessionHandle::spawn(
            "document-loopback",
            config,
            persistence.clone(),
            Arc::new(StaticTokenProvider),
        );

        next_matching_event(&mut events, |event| {
            matches!(event, SyncEvent::Status(SyncStatus::Connected))
        })
        .await;
        let operation_id = handle.queue_update(vec![vec![21, 22, 23]]).await.unwrap();
        operation_tx.send(operation_id.clone()).unwrap();

        next_matching_event(&mut events, |event| {
            matches!(event, SyncEvent::Status(SyncStatus::Reconnecting { .. }))
        })
        .await;
        next_matching_event(&mut events, |event| {
            matches!(event, SyncEvent::Status(SyncStatus::Connected))
        })
        .await;
        assert_eq!(
            next_matching_event(&mut events, |event| {
                matches!(event, SyncEvent::UpdateAck(_))
            })
            .await,
            SyncEvent::UpdateAck(operation_id)
        );
        assert_eq!(
            next_matching_event(&mut events, |event| matches!(event, SyncEvent::Update(_))).await,
            SyncEvent::Update(vec![9, 10])
        );
        assert_eq!(
            next_matching_event(&mut events, |event| matches!(
                event,
                SyncEvent::Awareness(_)
            ))
            .await,
            SyncEvent::Awareness(vec![11, 12])
        );
        assert!(
            persistence
                .pending_updates("document-loopback")
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            persistence
                .load_snapshot("document-loopback")
                .unwrap()
                .unwrap()
                .snapshot,
            vec![1, 2, 3]
        );

        handle.stop();
        next_matching_event(&mut events, |event| {
            matches!(event, SyncEvent::Status(SyncStatus::Stopped))
        })
        .await;
        server.await.unwrap();
    }
}
