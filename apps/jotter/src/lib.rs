//! jotter -- handwriting that syncs only with devices you have trusted.
//!
//! The headless core, each piece tested without a display or a network:
//!
//! * [`identity`]: this device's keys (Ed25519 to sign, X25519 to agree a secret) and the short fingerprint two
//!   people compare to be sure.
//! * [`pairing`]: an [`pairing::Offer`] (name, keys, addresses, a one-time nonce, signed) is what a screen shows as a
//!   QR; whoever scans it can make a [`pairing::claim`] that only someone who saw the code can make, and the shower
//!   [`pairing::open_claim`]s it. Nothing is trusted until the owner accepts the fingerprint on screen.
//! * [`trust`]: the list of paired devices, a plain text file.
//! * [`note`]: a page of strokes as a CRDT (a set of strokes plus tombstones, so two devices that both edited merge
//!   without conflict, in any order), with a compact binary codec small enough for a QR or two.
//! * [`share`]: cutting bytes into QR frames with a header, reassembling them, and laying several frames out as ONE
//!   mega QR mosaic on a screen (an e-ink panel holds a still picture of a dozen codes better than it cycles them).

pub mod identity;
pub mod note;
pub mod pairing;
pub mod share;
pub mod trust;

/// The app: notes list, a touch canvas, devices and QR screens (feature `app`).
#[cfg(feature = "app")]
pub mod app;
