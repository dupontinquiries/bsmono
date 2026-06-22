// src/service/mod.rs
//
// The "service" feature exposes our app's data and capabilities to
// other apps on the same Linux session. We expose four local IPC
// touch-points; none of them require networking on our side.
//
//   1. D-Bus on the session bus            — typed RPC, signals
//      `org.ambientlearner.Service1` at /org/ambientlearner/Service
//
//   2. XDG inbox files                     — no-library pin submission
//      $XDG_DATA_HOME/ambient-learner/inbox/pins/<id>.json
//
//   3. Our own Unix socket                 — newline-JSON event stream
//      $XDG_RUNTIME_DIR/ambient-learner.sock
//
//   4. snapclone-bus federation client     — opt-in mesh with peer apps
//      $XDG_RUNTIME_DIR/snapclone-bus.sock
//
// (1) and (2) and (3) are *us* exposing endpoints. (4) is us *talking
// to* a daemon some other app runs. All four work whether the others
// are present or absent.
//
// Network access still happens only in apps the user separately runs
// (a chat app, a weather sync, a photo importer). We are local-only.

#![cfg(feature = "service")]

pub mod map_pin;
pub mod pin_store;
pub mod inbox;
pub mod dbus_service;
pub mod unix_socket;
pub mod databus_client;

// These re-exports form the crate-public surface of the service
// feature. They're not all referenced inside this module, so rustc
// would warn — but external code (and our docs) does use them, so
// the allow is intentional and explicit.
#[allow(unused_imports)]
pub use map_pin::{MapPin, MapPinKind};
#[allow(unused_imports)]
pub use pin_store::{PinStore, UpsertResult};
#[allow(unused_imports)]
pub use dbus_service::{ServiceState, SERVICE_NAME, SERVICE_PATH};

use std::sync::Arc;
use std::time::Duration;

use crate::systems::CalibrationQueue;
use crate::world_state::WorldState;

/// Holds every long-lived handle the service feature spawns. Drop
/// to tear everything down (in tests) or just keep it alive for
/// the rest of the process (in main).
pub struct Runtime {
    /// The D-Bus connection that owns the well-known name. Dropping
    /// this releases the name from the bus.
    pub connection: Arc<zbus::Connection>,
    /// Inbox watcher handle. Kept so its background thread keeps
    /// running.
    pub _inbox: inbox::WatcherHandle,
    /// Our outbound Unix socket. Dropping it unlinks the file.
    pub _socket: Option<unix_socket::SocketHandle>,
    /// snapclone-bus federation client. Always present; the client
    /// itself handles the case where the daemon isn't running.
    pub _databus: databus_client::DataBusClient,
    /// Shared pin store, exposed so QML / GTK can read pins for
    /// rendering on the map.
    pub pins: PinStore,
}

impl Runtime {
    /// Spin everything up.
    pub async fn start(
        world: WorldState,
        calibration_queue: CalibrationQueue,
    ) -> Result<Self, String> {
        let pins = PinStore::new();
        pins.start_expiry_sweeper(Duration::from_secs(60));

        // Inbox: best-effort.
        let inbox_handle = inbox::ensure_and_start(pins.clone())
            .map_err(|e| format!("inbox watcher failed: {e}"))?;

        // D-Bus: required.
        let svc_state = ServiceState {
            world: world.clone(),
            pins: pins.clone(),
            calibration_queue,
        };
        let conn = dbus_service::start(svc_state)
            .await
            .map_err(|e| format!("D-Bus service failed: {e}"))?;

        let conn = Arc::new(conn);

        dbus_service::spawn_temperature_signal_emitter(
            (*conn).clone(),
            world.clone(),
        );

        // Our outbound Unix socket: best-effort.
        let socket_handle = match unix_socket::start(
            unix_socket::default_socket_path(),
            world.clone(),
            pins.clone(),
        ) {
            Ok(h) => Some(h),
            Err(e) => {
                log::warn!("unix socket disabled: {e}");
                None
            }
        };

        // snapclone-bus federation client: always start. The client
        // handles "daemon not running" gracefully.
        let databus = databus_client::DataBusClient::start(
            databus_client::default_socket_path(),
            pins.clone(),
        );

        // Mirror local pin upserts onto the federation bus. We do this
        // by spawning a small "sync" thread that compares the current
        // pin list against what it last forwarded and publishes the
        // delta. Two reasons over a callback approach:
        //
        //   * `PinStore::on_change` is fan-out-style and doesn't tell
        //     us *which* pin changed, so a callback would have to walk
        //     the whole list each time — quadratic.
        //   * A polling thread naturally collapses bursts: if a single
        //     pin gets upserted twice within one tick, we forward it
        //     once.
        //
        // We only forward pins whose `source` doesn't start with
        // "databus:" — that's how we avoid republishing pins that
        // arrived *from* the bus, which would loop forever.
        databus_client::spawn_outbound_mirror(
            databus.clone(),
            pins.clone(),
            Duration::from_millis(500),
        );

        // Publish our own sensor reading onto the bus so other apps
        // subscribed to `sensor_reading` see live temperature/humidity
        // /pressure as our user moves around. Only publishes when the
        // user has a current location with coordinates — without those,
        // a bus point would be meaningless.
        databus_client::spawn_sensor_publisher(
            databus.clone(),
            world.clone(),
            Duration::from_secs(60),
        );

        Ok(Self {
            connection: conn,
            _inbox: inbox_handle,
            _socket: socket_handle,
            _databus: databus,
            pins,
        })
    }
}
