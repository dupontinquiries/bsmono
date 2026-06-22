//! snapclone-net-intel — reachability memory and traversal intelligence.
//!
//! Implements the Reachability Memory System from NETWORK_ARCHITECTURE.md §1.
//!
//! Core idea: instead of brute-force retries, learn which paths succeed and
//! bias future connection attempts toward "warm" (recently successful) paths.
//!
//! # Key types
//! - `ReachabilityDb`  — persists path scores with time decay
//! - `PathScore`       — weighted success/failure record for a peer+network combo
//! - `TraversalHints`  — which strategy to try first, based on past experience
//! - `NetworkBin`      — subnet/ASN grouping for generalized learning

pub mod memory;
pub mod scoring;
pub mod hints;
pub mod bins;
pub mod lan_decision;
pub mod connreq_targets;

pub use memory::{ReachabilityDb, PathRecord};
pub use scoring::{PathScore, Protocol};
pub use hints::{TraversalHints, TraversalStrategy};
pub use bins::NetworkBin;
pub use lan_decision::{AddrClass, ConnType, ConnectDecision, SkipReason, RefuseReason};
