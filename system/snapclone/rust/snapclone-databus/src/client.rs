//! DataBusClient — connect to the bus from Rust code.

use crate::{
    point::MapPoint,
    protocol::{decode, encode, BusMessage, BusReply},
    server::socket_path,
};
use anyhow::{bail, Result};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    sync::mpsc,
};
use tracing::debug;

/// A connected client. Send commands via `publish()`, `subscribe()`, `list()`.
/// Receive pushed points from the `receiver()` channel.
pub struct DataBusClient {
    write:    tokio::net::unix::OwnedWriteHalf,
    read:     BufReader<tokio::net::unix::OwnedReadHalf>,
    push_tx:  mpsc::Sender<MapPoint>,
    push_rx:  mpsc::Receiver<MapPoint>,
}

impl DataBusClient {
    /// Connect to the SnapClone data bus.
    pub async fn connect() -> Result<Self> {
        let path = socket_path();
        let stream = UnixStream::connect(&path).await
            .map_err(|e| anyhow::anyhow!("Cannot connect to DataBus at {:?}: {}", path, e))?;
        Ok(Self::from_stream(stream))
    }

    fn from_stream(stream: UnixStream) -> Self {
        let (r, w) = stream.into_split();
        let (tx, rx) = mpsc::channel(128);
        Self { write: w, read: BufReader::new(r), push_tx: tx, push_rx: rx }
    }

    /// Publish a MapPoint to the bus. Returns the assigned point ID.
    pub async fn publish(&mut self, point: MapPoint) -> Result<String> {
        self.send(&BusMessage::Pub { point }).await?;
        match self.recv_reply().await? {
            BusReply::Ack { id } => Ok(id),
            BusReply::Err { message } => bail!("Publish failed: {}", message),
            other => bail!("Unexpected reply: {:?}", other),
        }
    }

    /// Subscribe to one or more layers. Pushed points arrive via `receiver()`.
    pub async fn subscribe(&mut self, layers: &[&str]) -> Result<()> {
        let layers = layers.iter().map(|s| s.to_string()).collect();
        self.send(&BusMessage::Sub { layers }).await?;
        match self.recv_reply().await? {
            BusReply::Ack { .. } => Ok(()),
            BusReply::Err { message } => bail!("Subscribe failed: {}", message),
            other => bail!("Unexpected reply: {:?}", other),
        }
    }

    /// Query a snapshot of in-TTL points for the given layers.
    pub async fn list(
        &mut self,
        layers: &[&str],
        bbox: Option<[f64; 4]>,
        limit: Option<usize>,
    ) -> Result<Vec<MapPoint>> {
        let layers = layers.iter().map(|s| s.to_string()).collect();
        self.send(&BusMessage::List { layers, bbox, limit }).await?;
        match self.recv_reply().await? {
            BusReply::Points { items, .. } => Ok(items),
            BusReply::Err { message } => bail!("List failed: {}", message),
            other => bail!("Unexpected reply: {:?}", other),
        }
    }

    /// Receive the next pushed MapPoint from the server.
    /// Call in a loop after subscribing.
    pub async fn next_pushed(&mut self) -> Option<MapPoint> {
        // Drain any buffered pushes then read the socket
        if let Ok(pt) = self.push_rx.try_recv() { return Some(pt); }
        loop {
            match self.recv_reply().await {
                Ok(BusReply::Push { point }) => return Some(point),
                Ok(BusReply::Pong) => continue,
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    }

    async fn send(&mut self, msg: &BusMessage) -> Result<()> {
        let mut s = serde_json::to_string(msg)?;
        s.push('\n');
        self.write.write_all(s.as_bytes()).await?;
        Ok(())
    }

    async fn recv_reply(&mut self) -> Result<BusReply> {
        let mut line = String::new();
        self.read.read_line(&mut line).await?;
        if line.is_empty() { bail!("DataBus connection closed"); }
        Ok(serde_json::from_str(line.trim())?)
    }
}
