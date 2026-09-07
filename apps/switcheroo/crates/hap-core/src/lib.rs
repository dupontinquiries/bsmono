//! hap-core — the HomeKit Accessory Protocol, controller side, with no
//! radio in it.
//!
//! HOMEKIT.md in this app said the WSP100 could not be switched because
//! pairing "is a real protocol implementation, not a missing UUID". This
//! is that implementation, in the layers the protocol has:
//!
//! * [`tlv8`] — the encoding everything is written in.
//! * [`srp`] — SRP-6a, verified against RFC 5054's vectors.
//! * [`keys`] — HKDF, ChaCha20-Poly1305 with HAP's nonces, the identity
//!   keys, and the [`keys::Pairing`] record that must be persisted.
//! * [`pair_setup`], [`pair_verify`] — the two handshakes as state machines
//!   over bytes.
//! * [`session`] — the per-direction counters the verified session runs on.
//! * [`ble`] — HAP-BLE PDUs, fragmentation, the [`ble::Transport`] seam, and
//!   the [`ble::Controller`] that drives all of the above.
//! * [`signature`] — `Characteristic-Signature-Read` and a capability probe, so a
//!   controller offers only the controls the accessory has: an outlet's read-only
//!   `Outlet In Use`, a light's `Brightness`, `Identify` for "which one are you".
//! * [`sim`] — an accessory behind that seam, so the whole thing is tested
//!   without a plug.
//!
//! What is NOT here, and why: IP transport (nothing in switcheroo speaks
//! HAP over IP yet — `session::SecureSession::seal_ip` is the only hook);
//! ADDING a pairing (there is no second controller to hand access to —
//! [`pairings`] does remove and list, which is what unpairing needs); MFi auth
//! (`Pair Setup with Auth`), which needs Apple's coprocessor; accessory
//! discovery from advertisements (switcheroo-core's `classify` does that).
//!
//! # The one thing tests cannot settle
//!
//! Two SRP conventions — `H(g)` unpadded in the client proof, `K = H(S)`
//! as the HKDF input — follow RFC 2945 and Apple's HomeKitADK, and the
//! simulator agrees with the controller because both were written from the
//! same reading. A real accessory rejecting M3 (error at state 4) is the
//! signal that one of those two is wrong, and `srp::Client::process` names
//! them.

pub mod ble;
pub mod keys;
pub mod pair_setup;
pub mod pair_verify;
/// The Pairings characteristic: removing a pairing so the accessory forgets us too.
pub mod pairings;
/// The HAP BLE advertisement: seeing an accessory change without connecting to it.
pub mod advert;
pub mod session;
/// Characteristic signatures and the capability probe: what an accessory actually has.
pub mod signature;
pub mod sim;
pub mod srp;
pub mod tlv8;

pub use ble::{Controller, HapError, Transport, TransportError};
pub use keys::Pairing;
pub use signature::{Capabilities, Signature};
