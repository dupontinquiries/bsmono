//! bo-cast — casting a track to a networked B&O speaker.
//!
//! Formerly four files inside bo-controller (Qt-free already, just
//! unreachable from anywhere else):
//!
//!   - `dlna_discover`   SSDP M-SEARCH + AVTransport control-URL extraction
//!   - `dlna_position`   poll a device's AVTransport for play/pause/position
//!   - `play_uri`        best-effort ASE/BeoZone "play this URL", DLNA fallback
//!   - `track_server`    a local HTTP server that hands a file (or a live
//!                       mixdown, via bo-core's realtime_mixer) to a device
//!                       that can only fetch a URL — a B&O speaker has no
//!                       notion of "the file on your phone", only "the file
//!                       at this address"
//!
//! Both bo-controller and bo-suite-egui now depend on this instead of only
//! the Qt binary being able to cast. bo-controller's own SSDP cache
//! (warmed at startup, read through `play_uri::set_ssdp_cache_provider`)
//! is unaffected — the provider is injected by whichever binary owns
//! startup discovery, not baked in here.
pub mod dlna_discover;
pub mod dlna_position;
pub mod play_uri;
pub mod track_server;
