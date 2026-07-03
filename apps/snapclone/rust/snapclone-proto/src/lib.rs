//! snapclone-proto — v3 snap wire protocol.
//!
//! Ported from SnapRouter.cpp. Handles:
//! - Frame flag definitions (ANNOUNCE, DATA, ACK, SYNC, NEED, DELTA)
//! - v3 frame serialisation and deserialisation
//! - Chunked payload splitting and reassembly
//! - Content hash verification (SHA-256)
//!
//! # Porting status
//!
//! | C++ method                      | Rust location              | Status   |
//! |---------------------------------|----------------------------|----------|
//! | SnapRouter::sendV3()            | frame::build()             | DONE     |
//! | SnapRouter::handleV3Message()   | frame::parse()             | DONE     |
//! | SnapRouter::chunkPayload()      | chunk::split()             | DONE     |
//! | SnapRouter::reassembleChunks()  | chunk::Reassembler         | DONE     |
//! | Content hash verify             | hash::verify()             | DONE     |
//! | handleV3Sync stagger logic      | sync::stagger_send()       | scaffold |

pub mod frame;
pub mod chunk;
pub mod hash;
pub mod sync;
pub mod types;
