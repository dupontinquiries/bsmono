// src/service/map_pin.rs
//
// A `MapPin` is a single piece of geo-located data submitted by a
// network-aware sibling app. We render it; we don't fetch it.
//
// Examples include:
//
//   kind = "weather"  → cached forecast / observation from a weather
//                       app's network calls
//   kind = "photo"    → a snap with a location, mirrored from a
//                       remote photo source by an importer
//   kind = "shop"     → an open shopping map listing pulled in by a
//                       commerce sync
//   kind = "snap"     → transient public snap shared via some other
//                       service the user runs
//
// We don't enforce a closed enum on `kind`. The whole point of this
// API is that whichever app on the system actually does networking
// can publish onto our local map without us shipping an update first.
// We do, however, normalize a few well-known kinds in
// `MapPinKind::well_known()`.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Free-form pin kind. Wrapped in a struct so we can add helpers
/// without committing to an enum.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MapPinKind(pub String);

impl MapPinKind {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Best-effort categorization for UI grouping. Unknown kinds get
    /// `"other"`, never `None`, so callers always have something to
    /// render.
    pub fn category(&self) -> &'static str {
        match self.0.as_str() {
            "weather" | "forecast" | "observation" => "weather",
            "photo" | "snap" | "image"             => "media",
            "shop" | "shopping" | "listing"        => "commerce",
            "directory" | "places"                 => "directory",
            _                                      => "other",
        }
    }

    /// The five well-known kinds we recognize and decorate specially.
    /// External apps can submit anything; this list is just for the
    /// "what categories does the UI know about" badge.
    pub fn well_known() -> &'static [&'static str] {
        &["weather", "photo", "snap", "shop", "directory"]
    }
}

/// A single geo-located item. Type-erased payload — the publisher and
/// renderer agree on shape via `kind`.
///
/// Keep this struct small and serializable by both serde_json (file
/// inbox) and zbus's `OwnedValue` (D-Bus interop). The payload is a
/// `HashMap<String, serde_json::Value>` rather than a `zvariant::Value`
/// because we want round-tripping through JSON files to be lossless.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MapPin {
    /// Stable id chosen by the publisher. Re-submitting with the same
    /// id replaces the existing pin. Use a UUID if you don't have a
    /// natural id.
    pub id: String,

    /// Free-form category string (see `MapPinKind`).
    pub kind: String,

    pub latitude: f64,
    pub longitude: f64,

    /// Optional human-readable label used by the UI when no richer
    /// rendering is available for this kind.
    #[serde(default)]
    pub title: Option<String>,

    /// Optional descriptive text or HTML-free body.
    #[serde(default)]
    pub description: Option<String>,

    /// Optional URI the publisher points us at: a photo path, a deep
    /// link into the chat app, etc. We never auto-open these — that's
    /// an explicit user action.
    #[serde(default)]
    pub uri: Option<String>,

    /// Free-form payload. Convention: `{"temperature_c": 21.5,
    /// "humidity_pct": 60}` for a weather observation; `{"price_cents":
    /// 199, "currency": "USD"}` for a shop listing; etc.
    #[serde(default)]
    pub payload: HashMap<String, serde_json::Value>,

    /// Identifier of the publisher (D-Bus unique name, file basename,
    /// or "self" for our own entries). Useful for moderation, mute
    /// lists, and audit trails.
    #[serde(default)]
    pub source: Option<String>,

    /// Unix timestamp of submission. Defaults to `now()` on construct.
    #[serde(default = "now_unix")]
    pub submitted_at: i64,

    /// Optional unix timestamp at which the pin should expire. The
    /// service-side runtime drops expired pins automatically.
    #[serde(default)]
    pub expires_at: Option<i64>,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl MapPin {
    /// Validate a pin against the constraints we care about:
    ///   - id non-empty and reasonably short
    ///   - kind non-empty and reasonably short
    ///   - lat/lon inside Earth's coordinate range
    ///   - payload fields use sensible JSON types
    ///
    /// Returns the pin (possibly minus normalized fields) on success
    /// or an explanation string on failure. We deliberately use a
    /// `Result<Self, String>` rather than an error enum because the
    /// rejection reason is going to be shown to whichever app
    /// submitted the pin, and a free-form string is more helpful
    /// than a typed `BadKind` discriminant.
    pub fn validated(self) -> Result<Self, String> {
        const MAX_ID_LEN: usize = 128;
        const MAX_KIND_LEN: usize = 64;
        const MAX_TITLE_LEN: usize = 256;
        const MAX_DESC_LEN: usize = 4096;
        const MAX_URI_LEN: usize = 2048;

        if self.id.is_empty() {
            return Err("id must not be empty".into());
        }
        if self.id.len() > MAX_ID_LEN {
            return Err(format!("id longer than {} bytes", MAX_ID_LEN));
        }
        // ids end up as filenames in the inbox; reject path traversal
        // and other characters that would surprise a sysadmin grepping
        // through the directory.
        if self.id.contains('/') || self.id.contains('\\') || self.id.contains('\0') {
            return Err("id must not contain '/', '\\', or NUL".into());
        }

        if self.kind.is_empty() {
            return Err("kind must not be empty".into());
        }
        if self.kind.len() > MAX_KIND_LEN {
            return Err(format!("kind longer than {} bytes", MAX_KIND_LEN));
        }

        if !(-90.0..=90.0).contains(&self.latitude) {
            return Err(format!("latitude {} outside [-90, 90]", self.latitude));
        }
        if !(-180.0..=180.0).contains(&self.longitude) {
            return Err(format!("longitude {} outside [-180, 180]", self.longitude));
        }

        if let Some(t) = &self.title {
            if t.len() > MAX_TITLE_LEN {
                return Err(format!("title longer than {} bytes", MAX_TITLE_LEN));
            }
        }
        if let Some(d) = &self.description {
            if d.len() > MAX_DESC_LEN {
                return Err(format!("description longer than {} bytes", MAX_DESC_LEN));
            }
        }
        if let Some(u) = &self.uri {
            if u.len() > MAX_URI_LEN {
                return Err(format!("uri longer than {} bytes", MAX_URI_LEN));
            }
        }

        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> MapPin {
        MapPin {
            id: "test-1".into(),
            kind: "photo".into(),
            latitude: 45.5,
            longitude: -73.5,
            title: None,
            description: None,
            uri: None,
            payload: HashMap::new(),
            source: None,
            submitted_at: 0,
            expires_at: None,
        }
    }

    #[test]
    fn rejects_empty_id() {
        let mut p = base();
        p.id = "".into();
        assert!(p.validated().is_err());
    }

    #[test]
    fn rejects_path_traversal_in_id() {
        let mut p = base();
        p.id = "../../etc/passwd".into();
        assert!(p.validated().is_err());
    }

    #[test]
    fn rejects_bad_lat_lon() {
        let mut p = base();
        p.latitude = 91.0;
        assert!(p.validated().is_err());

        let mut p = base();
        p.longitude = -181.0;
        assert!(p.validated().is_err());
    }

    #[test]
    fn well_known_categories() {
        assert_eq!(MapPinKind::new("weather").category(), "weather");
        assert_eq!(MapPinKind::new("snap").category(), "media");
        assert_eq!(MapPinKind::new("shop").category(), "commerce");
        assert_eq!(MapPinKind::new("zoinks").category(), "other");
    }

    #[test]
    fn round_trip_through_json() {
        let p = MapPin {
            payload: {
                let mut m = HashMap::new();
                m.insert("temperature_c".into(), serde_json::json!(21.5));
                m
            },
            ..base()
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: MapPin = serde_json::from_str(&s).unwrap();
        assert_eq!(back.id, p.id);
        assert_eq!(back.payload["temperature_c"], serde_json::json!(21.5));
    }

    #[test]
    fn rejects_oversized_fields() {
        // Each of these should fail validation on a length cap.
        // We deliberately use 1-char-longer-than-max strings to
        // pin down the exact boundary; if a future change raises a
        // cap, this test will fail loudly rather than silently
        // letting through twice as much data.

        let mut p = base();
        p.id = "x".repeat(129); // MAX_ID_LEN = 128
        assert!(p.clone().validated().unwrap_err().contains("id longer"));

        p = base();
        p.kind = "k".repeat(65); // MAX_KIND_LEN = 64
        assert!(p.clone().validated().unwrap_err().contains("kind longer"));

        p = base();
        p.title = Some("t".repeat(257)); // MAX_TITLE_LEN = 256
        assert!(p.clone().validated().unwrap_err().contains("title longer"));

        p = base();
        p.description = Some("d".repeat(4097)); // MAX_DESC_LEN = 4096
        assert!(p.clone().validated().unwrap_err().contains("description"));

        p = base();
        p.uri = Some("u".repeat(2049)); // MAX_URI_LEN = 2048
        assert!(p.clone().validated().unwrap_err().contains("uri longer"));
    }

    #[test]
    fn accepts_exactly_max_size_fields() {
        // The boundary itself must succeed. This pins down the
        // "longer than X" vs "at least X" semantics.
        let mut p = base();
        p.id = "x".repeat(128);
        assert!(p.validated().is_ok());

        let mut p = base();
        p.kind = "k".repeat(64);
        assert!(p.validated().is_ok());
    }

    #[test]
    fn lat_lon_boundary_values_accepted() {
        // The poles and the antimeridian are valid lat/lon values
        // and must not be rejected. Off-by-one in our range check
        // would break ships, polar stations, etc.
        let mut p = base();
        p.latitude = 90.0;
        p.longitude = 180.0;
        assert!(p.clone().validated().is_ok());

        p.latitude = -90.0;
        p.longitude = -180.0;
        assert!(p.validated().is_ok());
    }

    #[test]
    fn rejects_nul_in_id() {
        // The id field becomes a filename in the inbox; NUL bytes
        // would be silently truncated by some filesystems and
        // cause spurious "pin not found" behaviors. Reject them
        // explicitly rather than rely on the FS.
        let mut p = base();
        p.id = "abc\0xyz".into();
        let err = p.validated().unwrap_err();
        assert!(err.contains("NUL") || err.contains("\\0"), "got: {}", err);
    }
}
