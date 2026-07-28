//! snapclone-dht — DHT peer identity and discovery.
//!
//! Handles the "account" side of SnapClone: generating and persisting the
//! stable sc-ID, publishing presence to the DHT, and resolving sc-IDs to
//! network addresses.
//!
//! # Porting status
//!
//! | C++ component                   | Rust module       | Status   |
//! |---------------------------------|-------------------|----------|
//! | Identity::loadOrGenerate()      | identity::load()  | DONE     |
//! | sc-ID format/validation         | identity::*       | DONE     |
//! | AppController::connectByDhtId() | resolver::*       | scaffold |
//! | GossipDiscovery peer records    | gossip::*         | scaffold |
//! | DhtDiscovery announce           | announce::*       | scaffold |

pub mod core;
pub mod wire;
pub mod engine;
pub mod presence;
#[cfg(test)]
mod sim;

pub mod identity;
pub mod resolver;
pub mod gossip;
pub mod announce;

pub use identity::{ScId, Identity};
