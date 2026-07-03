//! Location data types — matches LocationTypes.hpp.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A location fix — GPS or IP-based.
/// Matches Core::LocationFix in C++.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LocationFix {
    /// Sender peer ID (sc-ID), filled by AppController when forwarding.
    pub sender_id:  String,
    /// Latitude in degrees (0.0 if unavailable).
    pub lat:        f64,
    /// Longitude in degrees (0.0 if unavailable).
    pub lon:        f64,
    /// Accuracy radius in metres (-1 if unknown).
    pub accuracy_m: f32,
    /// WiFi SSID of connected network (empty if unavailable or wired).
    pub ssid:       String,
    /// True if coordinates came from GPS/GeoClue (false = IP-based, ~5km accuracy).
    pub has_gps:    bool,
    /// When this fix was acquired.
    pub timestamp:  Option<DateTime<Utc>>,
}

impl LocationFix {
    /// True if this fix carries any useful information.
    pub fn is_valid(&self) -> bool {
        self.has_gps || !self.ssid.is_empty()
    }

    /// Round accuracy to nearest 100m (privacy: reduces location precision in DB).
    pub fn privacy_rounded(&self) -> Self {
        let rounded = (self.accuracy_m / 100.0).round() * 100.0;
        Self { accuracy_m: rounded, ..self.clone() }
    }

    /// Age of this fix in seconds. None if no timestamp.
    pub fn age_secs(&self) -> Option<i64> {
        self.timestamp.map(|t| Utc::now().signed_duration_since(t).num_seconds())
    }

    /// True if the fix is less than `max_age_secs` old.
    pub fn is_fresh(&self, max_age_secs: i64) -> bool {
        self.age_secs().map(|a| a < max_age_secs).unwrap_or(false)
    }
}

/// Address classification for display and relay decisions.
/// Mirrors AddrClass in snapclone-nat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddrClass { Public, Cgnat, Private, Loopback }
