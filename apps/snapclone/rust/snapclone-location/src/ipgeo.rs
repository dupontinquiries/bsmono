//! IP-based geolocation via ip-api.com — ported from LocationService::fallbackIpGeo().
//!
//! Enabled via feature = "http". Uses `reqwest` + `tokio`.
//!
//! # Rate limiting
//!
//! ip-api.com free tier: 45 req/min. The C++ code had a 10-minute cache to
//! avoid hitting limits. This implementation exposes the cache TTL so callers
//! can implement their own caching strategy.

use crate::types::LocationFix;
use anyhow::{bail, Result};
use chrono::Utc;
use serde::Deserialize;

/// ip-api.com response fields we care about.
#[derive(Deserialize)]
struct IpApiResponse {
    status: String,
    lat:    Option<f64>,
    lon:    Option<f64>,
}

/// Fetch an IP-based location fix from ip-api.com.
///
/// - Accuracy is fixed at 5000m (city-level).
/// - Caller is responsible for caching (reuse the result for ≥10 minutes).
/// - Times out after 10 seconds.
#[cfg(feature = "http")]
pub async fn fetch_ip_location() -> Result<LocationFix> {
    let url = "https://ip-api.com/json/?fields=lat,lon,status";

    let resp = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        reqwest::get(url),
    ).await
        .map_err(|_| anyhow::anyhow!("IP geolocation timed out"))?
        .map_err(|e| anyhow::anyhow!("IP geolocation request failed: {}", e))?;

    let body: IpApiResponse = resp.json().await
        .map_err(|e| anyhow::anyhow!("IP geolocation JSON parse error: {}", e))?;

    if body.status != "success" {
        bail!("ip-api.com returned non-success status");
    }
    let lat = body.lat.ok_or_else(|| anyhow::anyhow!("Missing lat"))?;
    let lon = body.lon.ok_or_else(|| anyhow::anyhow!("Missing lon"))?;

    if lat < -90.0 || lat > 90.0 || lon < -180.0 || lon > 180.0 {
        bail!("ip-api.com returned invalid coordinates: {},{}", lat, lon);
    }

    Ok(LocationFix {
        lat,
        lon,
        accuracy_m: 5000.0,
        has_gps:    false,
        ssid:       crate::ssid::read_ssid().unwrap_or_default(),
        timestamp:  Some(Utc::now()),
        ..Default::default()
    })
}
