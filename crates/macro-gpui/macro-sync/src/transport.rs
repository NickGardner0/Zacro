use anyhow::{Context as _, Result};
use async_tungstenite::{
    WebSocketStream,
    tokio::{ConnectStream, connect_async_with_tls_connector},
    tungstenite::Message,
};
use futures::StreamExt as _;
use std::sync::Arc;

use crate::{PeerMessage, RemoteMessage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncEnvironment {
    Local,
    Dev,
    Prod,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncServiceConfig {
    pub http_base_url: String,
    pub websocket_base_url: String,
    pub origin: String,
}

impl SyncServiceConfig {
    /// Hosts mirrored from Macro's `SYNC_SERVICE_HOSTS`. These worker suffixes
    /// are part of the production contract, not inferred from the storage API.
    pub fn for_environment(environment: SyncEnvironment) -> Self {
        match environment {
            SyncEnvironment::Local => Self {
                http_base_url: "http://localhost:8787".into(),
                websocket_base_url: "ws://localhost:8787".into(),
                origin: "http://localhost:3000".into(),
            },
            SyncEnvironment::Dev => Self {
                http_base_url: "https://sync-service-dev3.macroverse.workers.dev".into(),
                websocket_base_url: "wss://sync-service-dev3.macroverse.workers.dev".into(),
                origin: "https://dev.macro.com".into(),
            },
            SyncEnvironment::Prod => Self {
                http_base_url: "https://sync-service-prod2.macroverse.workers.dev".into(),
                websocket_base_url: "wss://sync-service-prod2.macroverse.workers.dev".into(),
                origin: "https://macro.com".into(),
            },
        }
    }

    pub fn connection_url(&self, scope_id: &str, token: &str) -> String {
        format!(
            "{}/document/{}/connect?token={}",
            self.websocket_base_url.trim_end_matches('/'),
            urlencoding::encode(scope_id),
            urlencoding::encode(token)
        )
    }

    pub fn document_url(&self, scope_id: &str, endpoint: &str) -> String {
        format!(
            "{}/document/{}/{}",
            self.http_base_url.trim_end_matches('/'),
            urlencoding::encode(scope_id),
            endpoint.trim_start_matches('/')
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundFrame {
    Message(RemoteMessage),
    HeartbeatPong,
    Closed,
}

pub struct SyncTransport {
    socket: WebSocketStream<ConnectStream>,
}

impl SyncTransport {
    pub async fn connect(url: String) -> Result<Self> {
        let (socket, _) = connect_async_with_tls_connector(
            url,
            Some(Arc::new(http_client_tls::tls_config()).into()),
        )
        .await
        .context("failed to connect to Macro sync service")?;
        Ok(Self { socket })
    }

    pub async fn send(&mut self, message: &PeerMessage) -> Result<()> {
        self.send_encoded(message.encode()?).await
    }

    pub async fn send_encoded(&mut self, frame: Vec<u8>) -> Result<()> {
        self.socket
            .send(Message::Binary(frame.into()))
            .await
            .context("failed to send Macro sync frame")
    }

    pub async fn heartbeat(&mut self) -> Result<()> {
        self.socket
            .send(Message::Text("ping".into()))
            .await
            .context("failed to send Macro sync heartbeat")
    }

    pub async fn recv(&mut self) -> Result<InboundFrame> {
        loop {
            let Some(frame) = self.socket.next().await else {
                return Ok(InboundFrame::Closed);
            };
            match frame.context("failed to receive Macro sync frame")? {
                Message::Binary(bytes) => {
                    return RemoteMessage::decode(bytes.as_ref())
                        .context("failed to decode Macro sync frame")
                        .map(InboundFrame::Message);
                }
                Message::Text(text) if text == "pong" => return Ok(InboundFrame::HeartbeatPong),
                Message::Text(text) if text == "ping" => {
                    self.socket
                        .send(Message::Text("pong".into()))
                        .await
                        .context("failed to answer Macro sync heartbeat")?;
                }
                Message::Close(_) => return Ok(InboundFrame::Closed),
                Message::Ping(bytes) => {
                    self.socket
                        .send(Message::Pong(bytes))
                        .await
                        .context("failed to answer websocket ping")?;
                }
                Message::Pong(_) => return Ok(InboundFrame::HeartbeatPong),
                Message::Text(_) | Message::Frame(_) => {}
            }
        }
    }

    pub async fn close(mut self) -> Result<()> {
        self.socket
            .close(None)
            .await
            .context("failed to close Macro sync socket")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_hosts_match_macro_contract() {
        let prod = SyncServiceConfig::for_environment(SyncEnvironment::Prod);
        assert_eq!(
            prod.websocket_base_url,
            "wss://sync-service-prod2.macroverse.workers.dev"
        );
        let dev = SyncServiceConfig::for_environment(SyncEnvironment::Dev);
        assert_eq!(
            dev.http_base_url,
            "https://sync-service-dev3.macroverse.workers.dev"
        );
        assert_eq!(
            dev.connection_url("doc/a", "one two"),
            "wss://sync-service-dev3.macroverse.workers.dev/document/doc%2Fa/connect?token=one%20two"
        );
    }
}
