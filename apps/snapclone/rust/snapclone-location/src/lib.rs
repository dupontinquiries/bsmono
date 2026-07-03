//! snapclone-location — location types, haversine distance, and IP-geolocation.
//!
//! Rust port of LocationService.cpp and LocationTypes.hpp.
//! The HTTP client is optional (feature = "http") so this crate can be used
//! in embedded or no-std contexts with just the types and math.
//!
//! # Privacy model (matching doc/LOCATION_PRIVACY.md)
//!
//! - Location is never sent without an explicit user action
//! - Fixes are cached in RAM only; DB write happens on snap send
//! - Accuracy rounded to nearest 100m before storage
//! - Cache cleared on drop

pub mod types;
pub mod haversine;
pub mod ssid;
pub mod freshness;
pub mod freshness_ffi;
pub mod motion;
pub mod motion_ffi;
#[cfg(feature = "http")]
pub mod ipgeo;

pub use types::{LocationFix, AddrClass};
pub use haversine::distance_km;
