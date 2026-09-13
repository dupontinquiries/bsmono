//! snapclone-nat — STUN, UDP hole-punching, and CGNAT detection for SnapClone.
//!
//! # Architecture
//!
//! A pure-Rust port of `NatTraversal.cpp` (the tested Qt implementation). No
//! Qt; tokio for async I/O. The Qt/C++ side reaches it through the C FFI in
//! `ffi.rs` (feature `ffi`); Rust callers use [`NatTraversal`] directly.
//!
//! The design that makes it work — and that the first scaffold here missed —
//! is ONE socket: STUN, punching, and the SCUD data channel all go through a
//! single UDP socket, so the reflexive address STUN discovers is the mapping
//! the punch packets actually use. See `traversal.rs`'s module doc.
//!
//! ## Porting status
//!
//! | C++                               | Rust                              | Status |
//! |-----------------------------------|-----------------------------------|--------|
//! | punch socket bind (9876→9877→0)   | `traversal::bind_reusable`        | DONE   |
//! | `start()` / `tryNextStunServer()` | `NatTraversal::discover`          | DONE   |
//! | DNS-failure skip to IP literals   | `udp_stun_all_servers`            | DONE   |
//! | TCP STUN (parallel + fallback)    | `NatTraversal::tcp_stun`          | DONE   |
//! | `restartStunOnly()`               | `restart_stun_only`               | DONE   |
//! | `sendStunBindingRequest`/parse    | `stun::build_request/parse_response` | DONE |
//! | `onPunchReadyRead()` demux        | `wire::classify` + `receive_loop` | DONE   |
//! | loopback / self-id / self-reflexive guards | `wire::self_guard`       | DONE   |
//! | `initiateHolePunch()` burst+maintenance | `initiate_hole_punch`/`punch_loop` | DONE |
//! | `stopHolePunch()`                 | `stop_hole_punch`                 | DONE   |
//! | CGNAT keepalive                   | `keepalive_loop`                  | DONE   |
//! | CGNAT detection (100.64/10)       | `cgnat::is_cgnat`                 | DONE   |
//! | `AppContext` burst halving        | —                                 | not ported (policy hook) |
//! | `turnRelayUrl()`                  | —                                 | not ported |
//! | SCUD channel (`UdpChannel`)       | surfaced as `NatEvent::ScudFrame` | boundary only |
//!
//! `punch.rs` is the earlier standalone burst helper, kept for callers that
//! already use it; `NatTraversal` does not go through it (it needs the
//! shared socket and the framed payload, which `punch::run` predates).

pub mod cgnat;
pub mod punch;
pub mod stun;
pub mod traversal;
pub mod wire;
#[cfg(feature = "ffi")]
pub mod ffi;

pub use traversal::{NatConfig, NatEvent, NatTraversal, PeerAddress};
