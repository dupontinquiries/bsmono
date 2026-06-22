//! ndJSON wire protocol for the data bus.

use crate::point::MapPoint;
use serde::{Deserialize, Serialize};

/// Messages sent FROM a client TO the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum BusMessage {
    /// Publish a new MapPoint to the bus.
    Pub { point: MapPoint },
    /// Subscribe to one or more layers. Server will push matching points.
    Sub { layers: Vec<String> },
    /// Cancel all subscriptions.
    Unsub,
    /// Request all in-TTL points for the given layers (snapshot query).
    List {
        layers: Vec<String>,
        /// Optional bounding box filter [min_lat, min_lon, max_lat, max_lon]
        #[serde(default)]
        bbox: Option<[f64; 4]>,
        /// Max results (default 200)
        #[serde(default)]
        limit: Option<usize>,
    },
    /// Ping (keepalive).
    Ping,
}

/// Messages sent FROM the server TO a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum BusReply {
    /// A new point matching the client's subscription.
    Push { point: MapPoint },
    /// Reply to a List request.
    Points { items: Vec<MapPoint>, total: usize },
    /// Acknowledgement of pub/sub.
    Ack { id: String },
    /// Error.
    Err { message: String },
    /// Pong.
    Pong,
}

pub fn encode(msg: &BusReply) -> anyhow::Result<String> {
    let mut s = serde_json::to_string(msg)?;
    s.push('\n');
    Ok(s)
}

pub fn decode(line: &str) -> anyhow::Result<BusMessage> {
    Ok(serde_json::from_str(line.trim())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::point::{Layer, MapPoint};

    #[test]
    fn pub_roundtrip() {
        let pt = MapPoint::new(Layer::WeatherStation, 45.5, -73.5, "test");
        let msg = BusMessage::Pub { point: pt };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"op\":\"pub\""));
        let decoded: BusMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded, BusMessage::Pub { .. }));
    }

    #[test]
    fn sub_message() {
        let msg = BusMessage::Sub { layers: vec!["public_snap".into(), "weather_station".into()] };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("sub"));
        assert!(json.contains("public_snap"));
    }

    #[test]
    fn list_with_bbox() {
        let msg = BusMessage::List {
            layers: vec!["public_snap".into()],
            bbox: Some([45.0, -74.0, 46.0, -73.0]),
            limit: Some(50),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("bbox"));
    }
}
