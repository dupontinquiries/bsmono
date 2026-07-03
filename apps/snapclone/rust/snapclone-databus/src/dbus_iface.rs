//! dbus_iface.rs — D-Bus session bus interface (Channel A).
//!
//! SnapClone registers as `org.snapclone.DataHub` on the session bus.
//! The network-aware sibling app calls methods on this interface to:
//! - Query what data SnapClone currently holds
//! - Submit incoming data from remote sources
//! - Subscribe to signals for live updates
//!
//! # Interface: org.snapclone.DataHub1
//!
//! ## Methods (sibling → SnapClone)
//!
//! | Method                       | Returns  | Description                          |
//! |------------------------------|----------|--------------------------------------|
//! | GetCurrentLocation()         | s (JSON) | Our last known GPS/IP fix            |
//! | GetPublicSnaps(bbox, limit)  | s (JSON) | In-TTL public snaps in bounding box  |
//! | GetLayerPoints(layer, limit) | s (JSON) | Any layer, snapshot query            |
//! | SubmitMapPoint(json)         | s (id)   | Push a point into our store          |
//! | GetCapabilities()            | as       | Which layers we have data for        |
//!
//! ## Signals (SnapClone → sibling)
//!
//! | Signal                       | Args               | Fires when…                |
//! |------------------------------|--------------------|----------------------------|
//! | LocationChanged              | lat, lon, acc, ssid| Our location updates        |
//! | SnapPublished                | snap_id, sender    | A public snap is shared     |
//! | MapPointAdded                | layer, id          | Any new point arrives       |
//!
//! # Python example (no D-Bus library beyond dbus-python)
//!
//! ```python
//! import dbus
//! bus  = dbus.SessionBus()
//! hub  = bus.get_object("org.snapclone.DataHub", "/org/snapclone/DataHub")
//! iface = dbus.Interface(hub, "org.snapclone.DataHub1")
//!
//! # Get our current location
//! loc = iface.GetCurrentLocation()
//! print(loc)  # {"lat":45.50,"lon":-73.56,"accuracy_m":15,"ssid":"MyWifi"}
//!
//! # Submit a weather reading from a remote forecast
//! iface.SubmitMapPoint(json.dumps({
//!     "layer": "weather_station",
//!     "lat": 45.5017, "lon": -73.5673,
//!     "source_app": "com.myforecastapp",
//!     "ttl_secs": 3600,
//!     "payload": {"temp_c": 18.0, "description": "Tomorrow: rain"}
//! }))
//!
//! # Subscribe to location changes
//! def on_location(lat, lon, acc, ssid):
//!     upload_to_my_server(lat, lon)
//!
//! hub.connect_to_signal("LocationChanged", on_location,
//!                        dbus_interface="org.snapclone.DataHub1")
//! loop = dbus.mainloop.glib.DBusGMainLoop()
//! GLib.MainLoop().run()
//! ```
//!
//! # gdbus example (shell, no libraries)
//!
//! ```bash
//! # Query current location
//! gdbus call --session \
//!   --dest org.snapclone.DataHub \
//!   --object-path /org/snapclone/DataHub \
//!   --method org.snapclone.DataHub1.GetCurrentLocation
//!
//! # Submit a weather pin
//! gdbus call --session \
//!   --dest org.snapclone.DataHub \
//!   --object-path /org/snapclone/DataHub \
//!   --method org.snapclone.DataHub1.SubmitMapPoint \
//!   '{"layer":"weather_station","lat":45.5,"lon":-73.5,"source_app":"myapp","ttl_secs":300,"payload":{"temp_c":21}}'
//! ```

// D-Bus integration is implemented on the Qt/C++ side using QDBusConnection.
// This file documents the interface and provides type-safe helpers for Rust clients.
// The actual D-Bus object is registered in DataBusQt (Qt C++ class).

/// The well-known D-Bus name SnapClone registers.
pub const DBUS_SERVICE_NAME:  &str = "org.snapclone.DataHub";
/// Object path for the DataHub interface.
pub const DBUS_OBJECT_PATH:   &str = "/org/snapclone/DataHub";
/// Primary interface name.
pub const DBUS_INTERFACE_NAME: &str = "org.snapclone.DataHub1";

/// D-Bus signal names emitted by SnapClone.
pub mod signals {
    pub const LOCATION_CHANGED:  &str = "LocationChanged";
    pub const SNAP_PUBLISHED:    &str = "SnapPublished";
    pub const MAP_POINT_ADDED:   &str = "MapPointAdded";
}

/// D-Bus method names available on the SnapClone hub.
pub mod methods {
    pub const GET_CURRENT_LOCATION: &str = "GetCurrentLocation";
    pub const GET_PUBLIC_SNAPS:     &str = "GetPublicSnaps";
    pub const GET_LAYER_POINTS:     &str = "GetLayerPoints";
    pub const SUBMIT_MAP_POINT:     &str = "SubmitMapPoint";
    pub const GET_CAPABILITIES:     &str = "GetCapabilities";
    pub const GET_SNAP_HISTORY:     &str = "GetSnapHistory";
}
