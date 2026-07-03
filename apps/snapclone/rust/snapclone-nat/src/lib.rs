//! snapclone-nat — STUN, UDP hole-punching, and CGNAT detection for SnapClone.
//!
//! # Architecture
//!
//! This crate is a pure-Rust port of NatTraversal.cpp. It has no Qt or OS
//! dependencies beyond tokio for async I/O. The Qt layer calls into it via
//! the C FFI surface in `ffi.rs`.
//!
//! ## Porting status
//!
//! | C++ method                      | Rust location           | Status   |
//! |---------------------------------|-------------------------|----------|
//! | NatTraversal::start()           | NatTraversal::start()   | scaffold |
//! | NatTraversal::sendStunRequest() | stun::send_request()    | DONE     |
//! | NatTraversal::parseStunResponse | stun::parse_response()  | DONE     |
//! | NatTraversal::onStunTimeout()   | NatTraversal::step()    | scaffold |
//! | NatTraversal::startPunch()      | punch::run()            | scaffold |
//! | NatTraversal::onPunchReadyRead  | punch::on_recv()        | scaffold |
//! | CGNAT detection (100.64/10)     | cgnat::is_cgnat()       | DONE     |
//! | NatTraversal::restartStunOnly() | NatTraversal::restart() | scaffold |

pub mod stun;
pub mod punch;
pub mod cgnat;
pub mod traversal;
#[cfg(feature = "ffi")]
pub mod ffi;

pub use traversal::{NatTraversal, NatConfig, NatEvent};
