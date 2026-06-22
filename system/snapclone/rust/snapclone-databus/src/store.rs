//! In-memory ring buffer of live MapPoints, indexed by layer.

use crate::point::{Layer, MapPoint};
use std::collections::HashMap;
use tracing::debug;

/// Max points stored per layer (oldest evicted when full).
const MAX_PER_LAYER: usize = 500;

#[derive(Debug, Default)]
pub struct PointStore {
    // layer_key → vec of points (ring buffer, newest at back)
    layers: HashMap<String, Vec<MapPoint>>,
}

impl PointStore {
    pub fn new() -> Self { Self::default() }

    /// Insert a point. Evicts expired and overflow entries.
    pub fn insert(&mut self, mut point: MapPoint) {
        // Ensure ID is set
        if point.id.is_empty() {
            point.id = uuid::Uuid::new_v4().to_string();
        }
        let key = point.layer.as_str().to_owned();
        let bucket = self.layers.entry(key.clone()).or_default();

        // Remove expired
        bucket.retain(|p| p.is_alive());
        // Evict oldest if full
        if bucket.len() >= MAX_PER_LAYER {
            bucket.remove(0);
            debug!("PointStore: evicted oldest point from layer {}", key);
        }
        bucket.push(point);
    }

    /// Return all live points for the requested layers, optionally filtered by
    /// a bounding box [min_lat, min_lon, max_lat, max_lon].
    pub fn query(
        &self,
        layers: &[String],
        bbox: Option<[f64; 4]>,
        limit: usize,
    ) -> Vec<MapPoint> {
        let mut results: Vec<MapPoint> = layers.iter()
            .filter_map(|l| self.layers.get(l))
            .flat_map(|bucket| bucket.iter())
            .filter(|p| p.is_alive())
            .filter(|p| {
                bbox.map_or(true, |b| {
                    p.lat >= b[0] && p.lon >= b[1] && p.lat <= b[2] && p.lon <= b[3]
                })
            })
            .take(limit)
            .cloned()
            .collect();

        // Newest first
        results.sort_by(|a, b| b.published_at.cmp(&a.published_at));
        results
    }

    /// Prune all expired points across all layers.
    pub fn prune(&mut self) {
        for bucket in self.layers.values_mut() {
            bucket.retain(|p| p.is_alive());
        }
        self.layers.retain(|_, v| !v.is_empty());
    }

    pub fn total_live(&self) -> usize {
        self.layers.values().map(|v| v.iter().filter(|p| p.is_alive()).count()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::point::Layer;

    #[test]
    fn insert_and_query() {
        let mut store = PointStore::new();
        let p = MapPoint::new(Layer::WeatherStation, 45.5, -73.5, "test");
        store.insert(p);
        let results = store.query(&["weather_station".to_owned()], None, 100);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn bbox_filter() {
        let mut store = PointStore::new();
        let inside  = MapPoint::new(Layer::PublicSnap, 45.5, -73.5, "test");
        let outside = MapPoint::new(Layer::PublicSnap, 50.0, -73.5, "test");
        store.insert(inside);
        store.insert(outside);
        let results = store.query(
            &["public_snap".to_owned()],
            Some([45.0, -74.0, 46.0, -73.0]),
            100,
        );
        assert_eq!(results.len(), 1);
        assert!((results[0].lat - 45.5).abs() < 0.01);
    }
}
