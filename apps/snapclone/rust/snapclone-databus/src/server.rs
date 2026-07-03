//! DataBusServer — Unix socket server that routes MapPoints between apps.
//!
//! Started by SnapClone at launch. Any local app connects to the socket and
//! can publish or subscribe. No root required.
//!
//! Socket path: $XDG_RUNTIME_DIR/snapclone-bus.sock
//!   → /run/user/1000/snapclone-bus.sock on a typical Linux system.

use crate::{
    point::MapPoint,
    protocol::{decode, encode, BusMessage, BusReply},
    store::PointStore,
};
use anyhow::Result;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{broadcast, Mutex},
};
use tracing::{debug, error, info, warn};

/// Socket path — XDG_RUNTIME_DIR preferred, fallback to /tmp.
pub fn socket_path() -> PathBuf {
    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| "/tmp".to_owned());
    PathBuf::from(runtime).join("snapclone-bus.sock")
}

/// Shared server state.
#[derive(Debug)]
struct ServerState {
    store:    PointStore,
    /// broadcast channel: any published point goes to all subscribers
    /// Subscribers filter by layer client-side (simpler than per-layer channels)
    tx:       broadcast::Sender<MapPoint>,
}

/// The data bus server. Call `run()` in a tokio task.
pub struct DataBusServer {
    state: Arc<Mutex<ServerState>>,
    path:  PathBuf,
}

impl DataBusServer {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self {
            state: Arc::new(Mutex::new(ServerState { store: PointStore::new(), tx })),
            path:  socket_path(),
        }
    }

    pub fn with_path(path: PathBuf) -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self {
            state: Arc::new(Mutex::new(ServerState { store: PointStore::new(), tx })),
            path,
        }
    }

    /// Start accepting connections. Runs until the server is dropped.
    pub async fn run(self) -> Result<()> {
        // Remove stale socket file
        let _ = std::fs::remove_file(&self.path);

        let listener = UnixListener::bind(&self.path)?;
        info!("DataBus listening on {:?}", self.path);

        // Prune expired points every 60 seconds
        {
            let state = Arc::clone(&self.state);
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
                loop {
                    ticker.tick().await;
                    state.lock().await.store.prune();
                }
            });
        }

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let state = Arc::clone(&self.state);
                    tokio::spawn(async move {
                        if let Err(e) = handle_client(stream, state).await {
                            debug!("DataBus client disconnected: {}", e);
                        }
                    });
                }
                Err(e) => error!("DataBus accept error: {}", e),
            }
        }
    }
}

impl Default for DataBusServer { fn default() -> Self { Self::new() } }

async fn handle_client(
    stream: UnixStream,
    state:  Arc<Mutex<ServerState>>,
) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();
    // Subscribed layers for this client connection
    let mut subscribed_layers: Vec<String> = Vec::new();
    // broadcast receiver (created lazily when client subscribes)
    let mut rx: Option<broadcast::Receiver<MapPoint>> = None;

    loop {
        // Read next client message OR receive a broadcast point for subscribers
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else { break };
                if line.trim().is_empty() { continue; }

                let reply = match decode(&line) {
                    Ok(msg) => handle_message(msg, &state, &mut subscribed_layers, &mut rx).await,
                    Err(e) => BusReply::Err { message: format!("Parse error: {}", e) },
                };
                write_half.write_all(encode(&reply)?.as_bytes()).await?;
            }
            // Push matching broadcast points to subscribed clients
            point = async {
                match rx.as_mut() {
                    Some(r) => r.recv().await.ok(),
                    None => { tokio::time::sleep(std::time::Duration::from_secs(3600)).await; None }
                }
            } => {
                if let Some(point) = point {
                    // Only push if this client is subscribed to that layer
                    let layer_str = point.layer.as_str().to_owned();
                    if subscribed_layers.is_empty() || subscribed_layers.contains(&layer_str) {
                        let reply = BusReply::Push { point };
                        if write_half.write_all(encode(&reply)?.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

async fn handle_message(
    msg:   BusMessage,
    state: &Arc<Mutex<ServerState>>,
    subs:  &mut Vec<String>,
    rx:    &mut Option<broadcast::Receiver<MapPoint>>,
) -> BusReply {
    match msg {
        BusMessage::Pub { mut point } => {
            if point.id.is_empty() { point.id = uuid::Uuid::new_v4().to_string(); }
            point.published_at = chrono::Utc::now();
            let id = point.id.clone();
            let mut guard = state.lock().await;
            // Broadcast to all live subscribers
            let _ = guard.tx.send(point.clone());
            // Persist in store
            guard.store.insert(point);
            info!("DataBus: published {}", id);
            BusReply::Ack { id }
        }

        BusMessage::Sub { layers } => {
            *subs = layers.clone();
            // Subscribe to the broadcast channel
            let guard = state.lock().await;
            *rx = Some(guard.tx.subscribe());
            drop(guard);
            debug!("DataBus: client subscribed to {:?}", layers);
            BusReply::Ack { id: "subscribed".to_owned() }
        }

        BusMessage::Unsub => {
            subs.clear();
            *rx = None;
            BusReply::Ack { id: "unsubscribed".to_owned() }
        }

        BusMessage::List { layers, bbox, limit } => {
            let limit = limit.unwrap_or(200);
            let guard = state.lock().await;
            let items = guard.store.query(&layers, bbox, limit);
            let total = items.len();
            debug!("DataBus: list {} layers → {} points", layers.len(), total);
            BusReply::Points { items, total }
        }

        BusMessage::Ping => BusReply::Pong,
    }
}
