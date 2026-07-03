//! snapclone-microdht — a tiny, dependency-free DHT node for SnapClone beta
//! testing.
//!
//! # Why this exists
//!
//! The production app talks to the real Jami/OpenDHT network (bootstrap
//! `bootstrap.jami.net:4222`, Kademlia over UDP). That's great for shipping
//! but painful for *testing*: you can't deterministically reproduce "peer
//! published its endpoint, I looked it up, the value expired after an hour",
//! and you can't run it offline. This crate is a LOCAL stand-in that reproduces
//! exactly the app-visible semantics of the DHT — keyed by sc-ID, opaque
//! encrypted endpoint values, TTL expiry, multi-value-per-key, re-announce —
//! over a trivial UDP text protocol, so a beta tester can:
//!
//!   * run `sc-microdht` on loopback (or a LAN box),
//!   * point a dev build of the app at it ("Beta testers" settings submenu),
//!   * and exercise publish / lookup / mailbox flows end-to-end, comparing the
//!     behavior against a build pointed at the real DHT.
//!
//! # What it is NOT
//!
//! It is **not** wire-compatible with OpenDHT — it does not implement Kademlia
//! routing, node IDs, or the OpenDHT packet format. A real device on the real
//! DHT cannot talk to this directly. It models the *value layer* the app cares
//! about, not the *transport*. The app integrates with it through the same
//! abstraction it uses for the real DHT (see the C++ MicroDhtClient shim), so
//! swapping between "real DHT" and "micro DHT" is a settings toggle, not a code
//! path change in feature logic.
//!
//! # Layout
//!
//! | Module      | Responsibility                                            |
//! |-------------|-----------------------------------------------------------|
//! | `store`     | TTL-expiring key -> values map (the data model)           |
//! | `protocol`  | the UDP text protocol (parse requests / encode responses) |
//! | `node`      | the UDP server + background sweeper                       |
//! | `client`    | blocking client used by tests + the app shim              |
//!
//! The keys are sc-ID strings (`sc-XXXXXXXX`); values are the app's opaque
//! encrypted endpoint blobs and connection-request mailbox entries. The DHT
//! never inspects values — same as OpenDHT.

pub mod client;
pub mod node;
pub mod protocol;
pub mod store;

pub use client::{Client, ClientError};
pub use node::{Node, MAX_TTL_SECS};
pub use store::Store;
