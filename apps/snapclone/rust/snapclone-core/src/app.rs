//! AppCore — the composition root in Rust.
//!
//! Equivalent to C++ AppController but as a pure-Rust struct with no Qt deps.
//! The Qt C++ AppController will eventually delegate to this.
//!
//! # Usage
//!
//! ```rust,no_run
//! use snapclone_core::{AppCore, AppConfig, AppCommand, AppEvent};
//!
//! let (mut app, mut events) = AppCore::new(AppConfig::default());
//! app.dispatch(AppCommand::Initialize);
//!
//! // In your UI loop:
//! while let Some(evt) = events.try_recv().ok() {
//!     match evt {
//!         AppEvent::ContactsLoaded { contacts } => ui.set_contacts(contacts),
//!         AppEvent::PeerConnected  { peer }     => ui.mark_online(peer.id),
//!         AppEvent::SnapReceived   { snap }      => ui.show_snap(snap),
//!         _ => {}
//!     }
//! }
//! ```

use crate::{AppCommand, AppEvent};
use std::sync::mpsc;

/// Configuration for AppCore initialisation.
#[derive(Debug, Clone, Default)]
pub struct AppConfig {
    /// Path to the SQLite database file.
    pub db_path:     String,
    /// Relay WebSocket URL (empty = disable relay).
    pub relay_url:   String,
    /// Override sc-ID (empty = load from disk or generate).
    pub sc_id_override: Option<String>,
}

/// The application core. Owns all subsystems.
/// Thread-safe: `dispatch()` can be called from any thread.
pub struct AppCore {
    cmd_tx: mpsc::SyncSender<AppCommand>,
    evt_rx: mpsc::Receiver<AppEvent>,
}

impl AppCore {
    /// Create a new AppCore. Returns the core and an event receiver.
    /// The core is idle until `dispatch(AppCommand::Initialize)` is called.
    pub fn new(_config: AppConfig) -> (Self, mpsc::Receiver<AppEvent>) {
        let (cmd_tx, _cmd_rx) = mpsc::sync_channel::<AppCommand>(64);
        let (evt_tx, evt_rx)  = mpsc::channel::<AppEvent>();

        // TODO: spawn the command processing loop on a background thread.
        // The loop reads from cmd_rx, calls subsystems, and sends to evt_tx.
        // For now this is a scaffold — the Qt AppController is the live impl.
        let _ = evt_tx; // suppress unused warning until impl is wired

        (Self { cmd_tx, evt_rx }, evt_rx)
    }

    /// Send a command to the core. Non-blocking; returns Err if queue is full.
    pub fn dispatch(&self, cmd: AppCommand) -> anyhow::Result<()> {
        self.cmd_tx.try_send(cmd)
            .map_err(|e| anyhow::anyhow!("Command queue full: {}", e))
    }
}
