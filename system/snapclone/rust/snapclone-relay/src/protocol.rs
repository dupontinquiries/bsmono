//! Relay protocol message types and codec.

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Messages exchanged with the relay server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum RelayMessage {
    /// Register this connection with the relay.
    Bind { id: String, peer: String },
    /// Relay server confirms the pair is ready.
    Ready,
    /// Opaque payload forwarded to/from the peer.
    Data { payload: String }, // base64 encoded
    /// Relay server keepalive.
    Ping,
    /// Reply to Ping.
    Pong,
    /// Error from the relay server.
    Error { message: String },
}

pub fn encode_relay_msg(msg: &RelayMessage) -> Result<String> {
    Ok(serde_json::to_string(msg)?)
}

pub fn decode_relay_msg(s: &str) -> Result<RelayMessage> {
    Ok(serde_json::from_str(s)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bind_roundtrip() {
        let m = RelayMessage::Bind { id: "sc-aabbccdd".into(), peer: "sc-11223344".into() };
        let s = encode_relay_msg(&m).unwrap();
        assert!(s.contains("bind"));
        let m2 = decode_relay_msg(&s).unwrap();
        if let RelayMessage::Bind { id, .. } = m2 { assert_eq!(id, "sc-aabbccdd"); }
        else { panic!("Wrong variant"); }
    }
    #[test]
    fn data_roundtrip() {
        let m = RelayMessage::Data { payload: "aGVsbG8=".into() };
        let s = encode_relay_msg(&m).unwrap();
        let m2 = decode_relay_msg(&s).unwrap();
        if let RelayMessage::Data { payload } = m2 { assert_eq!(payload, "aGVsbG8="); }
        else { panic!(); }
    }
}
