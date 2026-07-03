//! outbound.rs — One-way outbound streaming socket.
//!
//! Connects to `$XDG_RUNTIME_DIR/snapclone-data.sock`.
//! SnapClone writes newline-delimited JSON to every connected client.
//! Clients are read-only — for inbound data use D-Bus or the XDG inbox.
//!
//! # Usage (any language, no library needed)
//!
//! ```bash
//! nc -U /run/user/1000/snapclone-data.sock
//! # → {"event":"location","lat":45.50,"lon":-73.56,"accuracy_m":15,"ts":"..."}
//! # → {"event":"snap_published","snap_id":"...","sender":"sc-aabbccdd","ts":"..."}
//! # → {"event":"weather","temp_c":21.4,"humidity":62,"ts":"..."}
//! ```

use crate::point::{Layer, MapPoint};
use anyhow::Result;
use chrono::Utc;
use serde_json::{json, Value};
use std::{
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    net::{UnixListener, UnixStream},
};
use tracing::{debug, info, warn};

/// Path for the outbound-only stream socket.
pub fn stream_socket_path() -> PathBuf {
    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| "/tmp".to_owned());
    PathBuf::from(runtime).join("snapclone-data.sock")
}

/// A connected streaming client.
struct Client {
    stream: UnixStream,
    addr:   String,
}

/// Outbound stream server. Clients connect and receive a JSON event stream.
pub struct OutboundStream {
    clients: Arc<Mutex<Vec<Client>>>,
    path:    PathBuf,
}

impl OutboundStream {
    pub fn new() -> Self {
        Self {
            clients: Arc::new(Mutex::new(Vec::new())),
            path:    stream_socket_path(),
        }
    }

    /// Accept new clients in a background task.
    pub async fn start(&self) -> Result<()> {
        let _ = std::fs::remove_file(&self.path);
        let listener = UnixListener::bind(&self.path)?;

        // Mode 0600 — only the owning user can connect
        std::fs::set_permissions(&self.path,
            std::fs::Permissions::from_mode(0o600))?;
        info!("Outbound stream at {:?}", self.path);

        let clients = Arc::clone(&self.clients);
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let addr = format!("client-{}", Utc::now().timestamp_millis());
                        info!("Stream: new client {}", addr);

                        // Send hello frame
                        let hello = json!({
                            "event": "hello",
                            "version": "1",
                            "events": ["location","snap_published","weather",
                                       "map_point","ping"],
                            "ts": Utc::now().to_rfc3339(),
                        });
                        let mut c = Client { stream, addr };
                        let _ = write_line(&mut c.stream, &hello).await;
                        clients.lock().unwrap().push(c);
                    }
                    Err(e) => warn!("Stream accept error: {}", e),
                }
            }
        });
        Ok(())
    }

    /// Broadcast a map point to all connected clients.
    pub async fn broadcast_point(&self, point: &MapPoint) {
        let event = json!({
            "event": "map_point",
            "layer": point.layer.as_str(),
            "id":    point.id,
            "lat":   point.lat,
            "lon":   point.lon,
            "source_app": point.source_app,
            "payload": point.payload,
            "ts": Utc::now().to_rfc3339(),
        });
        self.broadcast(&event).await;
    }

    /// Broadcast a location fix (SnapClone's own GPS/IP location).
    pub async fn broadcast_location(&self, lat: f64, lon: f64, accuracy_m: f32, ssid: &str) {
        let event = json!({
            "event": "location",
            "lat": lat,
            "lon": lon,
            "accuracy_m": accuracy_m,
            "ssid": ssid,
            "ts": Utc::now().to_rfc3339(),
        });
        self.broadcast(&event).await;
    }

    /// Broadcast a snap publication event.
    pub async fn broadcast_snap(&self, snap_id: &str, sender_id: &str, layer: &str) {
        let event = json!({
            "event": "snap_published",
            "snap_id":   snap_id,
            "sender_id": sender_id,
            "layer":     layer,
            "ts": Utc::now().to_rfc3339(),
        });
        self.broadcast(&event).await;
    }

    async fn broadcast(&self, event: &Value) {
        let mut dead = Vec::new();
        let mut guard = self.clients.lock().unwrap();
        for (i, client) in guard.iter_mut().enumerate() {
            match tokio::time::timeout(
                Duration::from_millis(250),
                write_line(&mut client.stream, event),
            ).await {
                Ok(Ok(())) => {}
                _ => {
                    debug!("Stream: dropping slow/dead client {}", client.addr);
                    dead.push(i);
                }
            }
        }
        for i in dead.into_iter().rev() { guard.remove(i); }
    }
}

impl Default for OutboundStream { fn default() -> Self { Self::new() } }

async fn write_line(stream: &mut UnixStream, value: &Value) -> Result<()> {
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    stream.write_all(line.as_bytes()).await?;
    Ok(())
}
