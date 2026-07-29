//! proximity.rs — infrastructure-free LOCAL transports for the dense cluster
//! layer.
//!
//! When the global Internet path is down, physically-near cluster co-members can
//! still form a connected graph over these media — strengthening the
//! smaller-connected-graphs resilience model. Each medium can DISCOVER nearby
//! peers and DELIVER the same RPCs as the IP transport. All three are **stubs**
//! today; the dense-routing layer (see [`crate::cluster`]) will rank them by
//! reachability and cost alongside IP, preferring whatever can actually reach a
//! given co-member right now.

use crate::node::{FindValue, Transport};
use crate::routing::{Contact, NodeId};
use crate::store::Key;

/// A local proximity medium: discovers nearby peers and carries RPCs to them
/// without any internet infrastructure.
pub trait ProximityMedium: Transport {
    /// Stable label for logs and reachability ranking.
    fn name(&self) -> &'static str;
    /// Peers currently in range of this medium.
    fn discover(&self, now: u64) -> Vec<Contact>;
}

/// Bluetooth Low Energy peer transport (GATT advertise/scan). Range ~10–100 m,
/// no access point required — good for ambient cluster presence.
pub struct Bluetooth;

impl ProximityMedium for Bluetooth {
    fn name(&self) -> &'static str {
        "bluetooth-le"
    }
    fn discover(&self, _now: u64) -> Vec<Contact> {
        // TODO(proximity/bluetooth): BLE scan for advertising cluster peers,
        // resolve NodeId from the service-data payload, return their contacts.
        unimplemented!("TODO: BLE peer discovery via GATT advertise/scan")
    }
}

impl Transport for Bluetooth {
    fn find_node(&self, _from: &Contact, _to: &Contact, _target: &NodeId) -> Vec<Contact> {
        // TODO(proximity/bluetooth): write FIND_NODE to a GATT characteristic,
        // read the response back; fragment to the negotiated MTU.
        unimplemented!("TODO: deliver FIND_NODE over BLE GATT")
    }
    fn find_value(&self, _from: &Contact, _to: &Contact, _key: &Key) -> FindValue {
        unimplemented!("TODO: deliver FIND_VALUE over BLE GATT")
    }
    fn store(&self, _from: &Contact, _to: &Contact, _key: Key, _value: Vec<u8>, _ttl: u64) {
        // TODO(proximity/bluetooth): chunk the value across GATT writes.
        unimplemented!("TODO: deliver STORE over BLE GATT (MTU chunking)")
    }
    fn ping(&self, _from: &Contact, _to: &Contact) -> bool {
        unimplemented!("TODO: BLE reachability ping")
    }
}

/// Near-ultrasonic data-over-sound (~18–22 kHz). Very short range, needs no
/// radio at all — useful for same-room pairing and cold-start bootstrap where
/// even Bluetooth pairing is undesirable.
pub struct Acoustic;

impl ProximityMedium for Acoustic {
    fn name(&self) -> &'static str {
        "acoustic-nearultrasound"
    }
    fn discover(&self, _now: u64) -> Vec<Contact> {
        // TODO(proximity/acoustic): listen for a near-ultrasonic beacon carrying
        // a NodeId + reachability hint; demodulate (e.g. FSK/Chirp) to a contact.
        unimplemented!("TODO: acoustic beacon discovery (near-ultrasound)")
    }
}

impl Transport for Acoustic {
    fn find_node(&self, _from: &Contact, _to: &Contact, _target: &NodeId) -> Vec<Contact> {
        unimplemented!("TODO: deliver FIND_NODE over data-over-sound")
    }
    fn find_value(&self, _from: &Contact, _to: &Contact, _key: &Key) -> FindValue {
        unimplemented!("TODO: deliver FIND_VALUE over data-over-sound")
    }
    fn store(&self, _from: &Contact, _to: &Contact, _key: Key, _value: Vec<u8>, _ttl: u64) {
        // TODO(proximity/acoustic): low bitrate — only viable for tiny values
        // (pointers/keys), not bulk; fall through to another medium for blobs.
        unimplemented!("TODO: deliver small STORE over data-over-sound")
    }
    fn ping(&self, _from: &Contact, _to: &Contact) -> bool {
        unimplemented!("TODO: acoustic presence ping")
    }
}

/// Ad-hoc wireless (Wi-Fi Direct / IBSS) — a peer-to-peer Wi-Fi link with no
/// access point, for higher-bandwidth local clusters fully off-grid.
pub struct AdHocWifi;

impl ProximityMedium for AdHocWifi {
    fn name(&self) -> &'static str {
        "adhoc-wifi"
    }
    fn discover(&self, _now: u64) -> Vec<Contact> {
        // TODO(proximity/adhoc): Wi-Fi Direct service discovery / IBSS scan;
        // map discovered peers to contacts (link-local addr + NodeId).
        unimplemented!("TODO: ad-hoc Wi-Fi peer discovery (Wi-Fi Direct / IBSS)")
    }
}

impl Transport for AdHocWifi {
    fn find_node(&self, _from: &Contact, _to: &Contact, _target: &NodeId) -> Vec<Contact> {
        unimplemented!("TODO: deliver FIND_NODE over ad-hoc Wi-Fi link")
    }
    fn find_value(&self, _from: &Contact, _to: &Contact, _key: &Key) -> FindValue {
        unimplemented!("TODO: deliver FIND_VALUE over ad-hoc Wi-Fi link")
    }
    fn store(&self, _from: &Contact, _to: &Contact, _key: Key, _value: Vec<u8>, _ttl: u64) {
        unimplemented!("TODO: deliver STORE over ad-hoc Wi-Fi link")
    }
    fn ping(&self, _from: &Contact, _to: &Contact) -> bool {
        unimplemented!("TODO: ad-hoc Wi-Fi reachability ping")
    }
}

/// The local media available to the dense layer, in default reachability-rank
/// order. (Selection logic is TODO — pick the first medium that can reach a
/// given co-member.)
pub fn local_media() -> [&'static str; 3] {
    // TODO(proximity): replace with a real registry that probes reachability per
    // co-member and returns live `&dyn ProximityMedium` handles to try in order.
    [Bluetooth.name(), AdHocWifi.name(), Acoustic.name()]
}
