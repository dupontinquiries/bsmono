//! MapPoint — a typed geographic data point.
//!
//! This is the atom of the data bus. Every piece of data has:
//! - A geographic position (lat/lon + optional accuracy)
//! - A layer type (what kind of data)
//! - A source app identifier (reverse DNS)
//! - A TTL (seconds until the point expires)
//! - A JSON payload (layer-specific structured data)

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Identifies what kind of data a MapPoint contains.
/// Consumers subscribe to specific layers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    /// A publicly shared SnapClone snap (photo/video visible to nearby users)
    PublicSnap,
    /// Current atmospheric conditions at a location
    WeatherStation,
    /// A geotagged photo shared publicly (not ephemeral)
    PublicPhoto,
    /// A shopping venue or open data directory entry
    ShoppingDirectory,
    /// Raw sensor reading (temperature, air quality, noise, etc.)
    SensorReading,
    /// User-defined layer for app-specific data
    Custom(String),
}

impl Layer {
    pub fn as_str(&self) -> &str {
        match self {
            Self::PublicSnap       => "public_snap",
            Self::WeatherStation   => "weather_station",
            Self::PublicPhoto      => "public_photo",
            Self::ShoppingDirectory => "shopping_directory",
            Self::SensorReading    => "sensor_reading",
            Self::Custom(s)        => s.as_str(),
        }
    }
    pub fn from_str(s: &str) -> Self {
        match s {
            "public_snap"        => Self::PublicSnap,
            "weather_station"    => Self::WeatherStation,
            "public_photo"       => Self::PublicPhoto,
            "shopping_directory" => Self::ShoppingDirectory,
            "sensor_reading"     => Self::SensorReading,
            other                => Self::Custom(other.to_owned()),
        }
    }
}

/// A geographic data point published to the bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapPoint {
    /// Unique ID for this point (UUID). Publisher may set; server assigns if empty.
    pub id: String,
    /// Layer type — what kind of data this is.
    pub layer: Layer,
    /// Latitude in degrees.
    pub lat: f64,
    /// Longitude in degrees.
    pub lon: f64,
    /// Accuracy radius in metres (-1 = unknown).
    #[serde(default = "minus_one")]
    pub accuracy_m: f32,
    /// Reverse-DNS app identifier (e.g. "org.snapclone", "com.myweatherapp").
    pub source_app: String,
    /// Seconds until this point should be considered stale (0 = never expire).
    #[serde(default = "default_ttl")]
    pub ttl_secs: u32,
    /// Layer-specific structured data.
    pub payload: Value,
    /// When this point was published (server sets this).
    #[serde(default = "utc_now")]
    pub published_at: DateTime<Utc>,
}

impl MapPoint {
    pub fn new(layer: Layer, lat: f64, lon: f64, source_app: &str) -> Self {
        Self {
            id:           Uuid::new_v4().to_string(),
            layer,
            lat,
            lon,
            accuracy_m:   -1.0,
            source_app:   source_app.to_owned(),
            ttl_secs:     300,
            payload:      Value::Null,
            published_at: Utc::now(),
        }
    }

    /// True if this point is still within its TTL.
    pub fn is_alive(&self) -> bool {
        if self.ttl_secs == 0 { return true; }
        let age = Utc::now().signed_duration_since(self.published_at).num_seconds();
        age < self.ttl_secs as i64
    }

    /// Distance from (lat, lon) in kilometres.
    pub fn distance_km(&self, lat: f64, lon: f64) -> f64 {
        snapclone_location_math(self.lat, self.lon, lat, lon)
    }
}

// ── Typed payload helpers ─────────────────────────────────────────────────────

/// Structured payload for a public snap.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicSnap {
    /// sc-ID of the sender.
    pub sender_id:    String,
    /// Human-readable display name.
    pub sender_name:  String,
    /// Content hash (SHA-256 hex) — used to deduplicate.
    pub content_hash: String,
    /// Duration in seconds the snap remains visible.
    pub duration_secs: u32,
    /// Optional caption.
    pub caption:      Option<String>,
    /// Media type: "image" or "video"
    pub media_type:   String,
}

/// Structured payload for a weather station reading.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeatherReading {
    /// Temperature in Celsius.
    pub temp_c:       f32,
    /// Relative humidity 0–100.
    pub humidity_pct: f32,
    /// Wind speed in km/h.
    pub wind_kmh:     Option<f32>,
    /// Short description ("Partly cloudy", "Rain", etc.)
    pub description:  String,
    /// Station name or app-defined identifier.
    pub station_name: Option<String>,
}

/// Structured payload for a shopping / open-data directory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShoppingEntry {
    pub name:        String,
    pub category:    String,       // "grocery", "pharmacy", "market", etc.
    pub open_now:    Option<bool>,
    pub hours:       Option<String>,
    pub url:         Option<String>,
    pub description: Option<String>,
}

/// Structured payload for a raw sensor reading.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorReading {
    pub sensor_type: String,   // "temperature", "co2", "pm25", "noise_db", etc.
    pub value:       f64,
    pub unit:        String,
    pub quality:     Option<String>, // "good", "moderate", "poor"
}

// Serde helpers
fn minus_one() -> f32 { -1.0 }
fn default_ttl() -> u32 { 300 }
fn utc_now() -> DateTime<Utc> { Utc::now() }

// Inline haversine (avoid importing snapclone-location to keep crate minimal)
fn snapclone_location_math(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    use std::f64::consts::PI;
    let r = 6371.0f64;
    let dl = (lat2-lat1)*PI/180.0;
    let dlo= (lon2-lon1)*PI/180.0;
    let a = (dl/2.0).sin().powi(2)
          + (lat1*PI/180.0).cos() * (lat2*PI/180.0).cos() * (dlo/2.0).sin().powi(2);
    r * 2.0 * a.sqrt().atan2((1.0-a).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn point_ttl_alive() {
        let p = MapPoint::new(Layer::WeatherStation, 45.5, -73.5, "test");
        assert!(p.is_alive());
    }
    #[test]
    fn layer_roundtrip() {
        assert_eq!(Layer::from_str("public_snap"), Layer::PublicSnap);
        assert_eq!(Layer::from_str("my_custom"), Layer::Custom("my_custom".to_owned()));
        assert_eq!(Layer::PublicSnap.as_str(), "public_snap");
    }
    #[test]
    fn distance_same_point() {
        let p = MapPoint::new(Layer::PublicPhoto, 48.8566, 2.3522, "test");
        assert!(p.distance_km(48.8566, 2.3522) < 1e-9);
    }
}
