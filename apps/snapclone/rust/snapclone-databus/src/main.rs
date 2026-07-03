//! snapclone-bus daemon — starts all three IPC channels.
//!
//! Usually started by SnapClone automatically at launch.
//! Can also run standalone: snapclone-bus &

use snapclone_databus::{
    server::DataBusServer,
    outbound::OutboundStream,
    inbox::InboxWatcher,
};
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env()
            .add_directive("snapclone_databus=info".parse()?))
        .init();

    tracing::info!("snapclone-bus starting");

    // Channel A+C: pub/sub server (also feeds Channel C)
    let server = DataBusServer::new();

    // Channel B: XDG inbox watcher
    let inbox_store = Arc::new(Mutex::new(snapclone_databus::store::PointStore::new()));
    let watcher = InboxWatcher::new(Arc::clone(&inbox_store));
    watcher.start().await?;

    // Channel C: one-way outbound stream
    let stream = OutboundStream::new();
    stream.start().await
        .unwrap_or_else(|e| tracing::warn!("Outbound stream failed (continuing): {}", e));

    tracing::info!("Three IPC channels active:");
    tracing::info!("  A: pub/sub at {:?}", snapclone_databus::server::socket_path());
    tracing::info!("  B: inbox at  {:?}", snapclone_databus::inbox::inbox_path());
    tracing::info!("  C: stream at {:?}", snapclone_databus::outbound::stream_socket_path());

    server.run().await
}
