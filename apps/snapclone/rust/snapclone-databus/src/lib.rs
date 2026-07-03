//! snapclone-databus — local geographic data hub.
//!
//! # Architecture: SnapClone as a local data hub
//!
//! SnapClone never makes outbound network requests for data.
//! The internet-aware sibling app does all networking.
//! SnapClone provides three LOCAL IPC channels for data exchange:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │  weather app / photo syncer / map app / any network app     │
//! │  (does its own HTTP, TLS, DNS — we don't touch that)        │
//! └──────────────────────┬──────────────────────────────────────┘
//!                        │  Three local IPC channels:
//!          ┌─────────────┼─────────────────────────┐
//!          │             │                         │
//!    ┌─────▼──────┐ ┌────▼────────┐ ┌─────────────▼──────┐
//!    │  D-Bus     │ │ XDG inbox   │ │ Unix stream socket  │
//!    │ Channel A  │ │ Channel B   │ │ Channel C           │
//!    │            │ │             │ │                     │
//!    │ Typed RPC  │ │ Drop a .json│ │ nc -U sock →        │
//!    │ Signals    │ │ file → done │ │ live JSON stream    │
//!    │ Discovery  │ │ No libs     │ │ One-way outbound    │
//!    └────────────┘ └─────────────┘ └─────────────────────┘
//!          │             │                         │
//!          └─────────────┼─────────────────────────┘
//!                        │
//!          ┌─────────────▼─────────────────────────┐
//!          │       SnapClone (local hub)            │
//!          │  - Stores points with TTL              │
//!          │  - Shows them on PollinatorPage radar  │
//!          │  - Shares public snaps to the bus      │
//!          │  - Never makes outbound HTTP/TCP       │
//!          └───────────────────────────────────────┘
//! ```
//!
//! # Channels
//!
//! ## Channel A: D-Bus (`org.snapclone.DataHub`)
//! - Typed RPC: `GetCurrentLocation`, `SubmitMapPoint`, `GetLayerPoints`
//! - Signals: `LocationChanged`, `SnapPublished`, `MapPointAdded`
//! - Best for: structured queries, signal subscriptions
//!
//! ## Channel B: XDG inbox (`~/.local/share/snapclone/inbox/*.json`)
//! - Drop a JSON file → SnapClone picks it up within 2 seconds
//! - No library needed — shell, cron, gpsd postscript all work
//! - Survives SnapClone restarts (files persist)
//! - Best for: batch imports, fire-and-forget, shell scripts
//!
//! ## Channel C: Unix stream socket (`$XDG_RUNTIME_DIR/snapclone-data.sock`)
//! - SnapClone streams events as newline-delimited JSON
//! - One-way OUTBOUND: `nc -U /run/user/1000/snapclone-data.sock`
//! - 250ms write timeout — slow clients are dropped, stream continues
//! - Mode 0600 — only the owning user can connect
//! - Best for: live location upload, real-time snap monitoring

pub mod point;
pub mod protocol;
pub mod server;
pub mod client;
pub mod store;
pub mod outbound;
pub mod inbox;
pub mod dbus_iface;

pub use point::{MapPoint, Layer, PublicSnap, WeatherReading};
pub use protocol::{BusMessage, BusReply};
pub use server::DataBusServer;
pub use client::DataBusClient;
pub use outbound::OutboundStream;
pub use inbox::InboxWatcher;
pub use dbus_iface::{DBUS_SERVICE_NAME, DBUS_OBJECT_PATH, DBUS_INTERFACE_NAME};
