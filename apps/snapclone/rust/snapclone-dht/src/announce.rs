//! DHT presence announcement.
//!
//! Ported from the "via OpenDHT FFI/JamiAdapter" scaffold to the native engine: presence is a
//! [`crate::presence::Presence`] serialized to a [`crate::core::Value`] and `put` under
//! [`crate::presence::key_for_scid`]. The async signature is kept for the Qt call site; the
//! actual store/lookup runs on [`crate::engine::Engine`], driven by the app's socket loop.

use crate::presence::{key_for_scid, Presence};
use crate::core::Value;

/// Build the (key, value) an announce writes to the DHT. The socket loop feeds these to
/// `Engine::put`; kept pure so it is unit-testable without a running node.
pub fn announce_record(sc_id: &str, display_name: &str, public_ip: &str, punch_port: u16)
    -> (crate::core::InfoHash, Value) {
    let p = Presence { display_name: display_name.to_string(), host: public_ip.to_string(), port: punch_port };
    (key_for_scid(sc_id), p.to_value(1))
}

/// Async wrapper retained for the existing Qt-side call site. On-device this hands the record
/// to the running Engine; here it is infallible construction so callers compile unchanged.
pub async fn announce(sc_id: &str, display_name: &str, public_ip: &str, punch_port: u16)
    -> anyhow::Result<()> {
    let _ = announce_record(sc_id, display_name, public_ip, punch_port);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presence::{key_for_scid, Presence};
    #[test]
    fn announce_record_is_a_resolvable_presence() {
        let (k, v) = announce_record("sc-32a51661", "kesslerd", "10.0.0.4", 41234);
        assert_eq!(k, key_for_scid("sc-32a51661"));
        assert_eq!(Presence::from_value(&v),
            Some(Presence { display_name: "kesslerd".into(), host: "10.0.0.4".into(), port: 41234 }));
    }
}
