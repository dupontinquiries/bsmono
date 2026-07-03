//! Async relay transport — ported from MagicWormholeRelayTransport.cpp.

use crate::{
    backoff::ExponentialBackoff,
    protocol::{decode_relay_msg, encode_relay_msg, RelayMessage},
};
use anyhow::Result;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Events emitted by the relay transport to the caller.
#[derive(Debug, Clone)]
pub enum RelayEvent {
    /// WebSocket connection established and Bind sent.
    Connected,
    /// Relay confirmed the peer pair is ready — bidirectional data can flow.
    PeerReady,
    /// Data payload received from the relay (already decoded from base64).
    DataReceived(Vec<u8>),
    /// Transport disconnected. Will reconnect automatically if attempts remain.
    Disconnected,
    /// All reconnect attempts exhausted — caller must take action.
    Failed { reason: String },
}

/// Relay transport handle. Created by `RelayTransport::connect()`.
pub struct RelayTransport {
    event_rx: mpsc::Receiver<RelayEvent>,
    data_tx:  mpsc::Sender<Vec<u8>>,
}

impl RelayTransport {
    /// Connect to a relay server and return a handle.
    ///
    /// Spawns a background tokio task that manages the WebSocket lifecycle,
    /// including exponential-backoff reconnection.
    pub async fn connect(
        relay_url: String,
        self_id:   String,
        peer_id:   String,
    ) -> Result<Self> {
        let (event_tx, event_rx) = mpsc::channel::<RelayEvent>(32);
        let (data_tx, mut data_rx) = mpsc::channel::<Vec<u8>>(64);

        tokio::spawn(async move {
            let mut backoff = ExponentialBackoff::default_relay();
            loop {
                info!("Relay: connecting to {}", relay_url);
                match run_session(&relay_url, &self_id, &peer_id,
                                  &event_tx, &mut data_rx).await {
                    Ok(()) => {
                        // Clean disconnect — reset backoff
                        backoff.reset();
                        let _ = event_tx.send(RelayEvent::Disconnected).await;
                    }
                    Err(e) => {
                        warn!("Relay session error: {}", e);
                        let _ = event_tx.send(RelayEvent::Disconnected).await;
                    }
                }
                match backoff.next() {
                    Some(delay) => {
                        debug!("Relay: retry in {:?}", delay);
                        tokio::time::sleep(delay).await;
                    }
                    None => {
                        let _ = event_tx.send(RelayEvent::Failed {
                            reason: "Max reconnect attempts exceeded".to_owned()
                        }).await;
                        break;
                    }
                }
            }
        });

        Ok(Self { event_rx, data_tx })
    }

    /// Send raw bytes through the relay.
    pub async fn send(&self, data: Vec<u8>) -> Result<()> {
        self.data_tx.send(data).await
            .map_err(|e| anyhow::anyhow!("Relay send channel closed: {}", e))
    }

    /// Receive the next relay event (None = transport dropped).
    pub async fn recv(&mut self) -> Option<RelayEvent> {
        self.event_rx.recv().await
    }
}

async fn run_session(
    relay_url: &str,
    self_id:   &str,
    peer_id:   &str,
    event_tx:  &mpsc::Sender<RelayEvent>,
    data_rx:   &mut mpsc::Receiver<Vec<u8>>,
) -> Result<()> {
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    use futures_util::{SinkExt, StreamExt};
    use base64::Engine;

    let (ws_stream, _) = connect_async(relay_url).await?;
    let (mut write, mut read) = ws_stream.split();

    // Send bind
    let bind = encode_relay_msg(&RelayMessage::Bind {
        id: self_id.to_owned(), peer: peer_id.to_owned()
    })?;
    write.send(Message::Text(bind)).await?;
    let _ = event_tx.send(RelayEvent::Connected).await;
    info!("Relay: bound {} ↔ {}", self_id, peer_id);

    loop {
        tokio::select! {
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(txt))) => {
                        match decode_relay_msg(&txt) {
                            Ok(RelayMessage::Ready) => {
                                let _ = event_tx.send(RelayEvent::PeerReady).await;
                            }
                            Ok(RelayMessage::Data { payload }) => {
                                match base64::engine::general_purpose::STANDARD.decode(&payload) {
                                    Ok(bytes) => { let _ = event_tx.send(RelayEvent::DataReceived(bytes)).await; }
                                    Err(e) => warn!("Relay: bad base64: {}", e),
                                }
                            }
                            Ok(RelayMessage::Ping) => {
                                write.send(Message::Text(encode_relay_msg(&RelayMessage::Pong)?)).await?;
                            }
                            Ok(RelayMessage::Error { message }) => {
                                warn!("Relay server error: {}", message);
                            }
                            Ok(_) => {}
                            Err(e) => warn!("Relay: parse error: {}", e),
                        }
                    }
                    Some(Ok(Message::Ping(d))) => { write.send(Message::Pong(d)).await?; }
                    Some(Ok(Message::Close(_))) | None => return Ok(()),
                    Some(Err(e)) => return Err(e.into()),
                    _ => {}
                }
            }
            data = data_rx.recv() => {
                match data {
                    Some(bytes) => {
                        use base64::Engine;
                        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
                        write.send(Message::Text(
                            encode_relay_msg(&RelayMessage::Data { payload: encoded })?
                        )).await?;
                    }
                    None => return Ok(()),
                }
            }
        }
    }
}
