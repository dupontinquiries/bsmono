//! inbox.rs — XDG inbox folder watcher (Channel B).
//!
//! The network-aware sibling app drops JSON files here.
//! SnapClone picks them up, processes them, and moves them to processed/.
//! No library needed on the sender side — shell, cron, gpsd postscript, anything.
//!
//! Inbox path: ~/.local/share/snapclone/inbox/
//! Processed:  ~/.local/share/snapclone/inbox/processed/
//!
//! # File format
//!
//! Any .json file dropped in the inbox is treated as a MapPoint:
//!
//! ```json
//! {
//!   "layer": "weather_station",
//!   "lat": 45.5017,
//!   "lon": -73.5673,
//!   "accuracy_m": 500.0,
//!   "source_app": "com.myweatherapp",
//!   "ttl_secs": 300,
//!   "payload": {
//!     "temp_c": 21.4,
//!     "humidity_pct": 62,
//!     "description": "Partly cloudy"
//!   }
//! }
//! ```
//!
//! # Shell example (from a weather script)
//!
//! ```bash
//! cat > ~/.local/share/snapclone/inbox/weather-$(date +%s).json << EOF
//! {"layer":"weather_station","lat":45.5017,"lon":-73.5673,
//!  "source_app":"my-weather-script","ttl_secs":1800,
//!  "payload":{"temp_c":21.4,"description":"Partly cloudy"}}
//! EOF
//! ```
//!
//! # gpsd → SnapClone bridge (10-line shell script)
//!
//! ```bash
//! #!/bin/sh
//! gpspipe -w | while read line; do
//!   lat=$(echo $line | jq -r '.lat // empty')
//!   lon=$(echo $line | jq -r '.lon // empty')
//!   [ -z "$lat" ] && continue
//!   echo "{\"layer\":\"sensor_reading\",\"lat\":$lat,\"lon\":$lon,
//!     \"source_app\":\"gpsd\",\"ttl_secs\":60,
//!     \"payload\":{\"sensor_type\":\"gps\",\"value\":0,\"unit\":\"fix\"}}" \
//!     > ~/.local/share/snapclone/inbox/gps-$(date +%s%N).json
//! done
//! ```

use crate::{point::MapPoint, store::PointStore};
use anyhow::Result;
use std::{
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

/// Inbox folder path.
pub fn inbox_path() -> PathBuf {
    let data = std::env::var("XDG_DATA_HOME")
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_owned());
            format!("{home}/.local/share")
        });
    PathBuf::from(data).join("snapclone").join("inbox")
}

pub fn processed_path() -> PathBuf { inbox_path().join("processed") }

/// Watch the inbox folder and load new .json files into the point store.
pub struct InboxWatcher {
    store: Arc<Mutex<PointStore>>,
}

impl InboxWatcher {
    pub fn new(store: Arc<Mutex<PointStore>>) -> Self { Self { store } }

    /// Start polling the inbox directory every 2 seconds.
    pub async fn start(self) -> Result<()> {
        let inbox = inbox_path();
        let processed = processed_path();
        std::fs::create_dir_all(&inbox)?;
        std::fs::create_dir_all(&processed)?;
        info!("Inbox watcher: {:?}", inbox);

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(2));
            loop {
                ticker.tick().await;
                if let Err(e) = self.scan_once(&inbox, &processed).await {
                    warn!("Inbox scan error: {}", e);
                }
            }
        });
        Ok(())
    }

    async fn scan_once(&self, inbox: &PathBuf, processed: &PathBuf) -> Result<()> {
        let entries = std::fs::read_dir(inbox)?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") { continue; }

            match self.load_point(&path) {
                Ok(point) => {
                    let id = point.id.clone();
                    self.store.lock().await.insert(point);
                    info!("Inbox: loaded point {} from {:?}", id, path.file_name().unwrap_or_default());
                    // Move to processed/
                    let dest = processed.join(path.file_name().unwrap());
                    let _ = std::fs::rename(&path, &dest);
                }
                Err(e) => {
                    warn!("Inbox: skipping {:?}: {}", path.file_name().unwrap_or_default(), e);
                    // Move broken files to processed/broken/ so we don't loop
                    let broken = processed.join("broken");
                    let _ = std::fs::create_dir_all(&broken);
                    let _ = std::fs::rename(&path, broken.join(path.file_name().unwrap()));
                }
            }
        }
        Ok(())
    }

    fn load_point(&self, path: &PathBuf) -> Result<MapPoint> {
        let data = std::fs::read(path)?;
        // Guard: 1MB max
        if data.len() > 1_048_576 {
            anyhow::bail!("File too large: {} bytes", data.len());
        }
        let mut point: MapPoint = serde_json::from_slice(&data)?;
        if point.id.is_empty() {
            // Assign an ID from the filename (without extension)
            point.id = path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("inbox")
                .to_owned();
        }
        point.published_at = chrono::Utc::now();
        debug!("Inbox: parsed {} @ {},{}", point.layer.as_str(), point.lat, point.lon);
        Ok(point)
    }
}
