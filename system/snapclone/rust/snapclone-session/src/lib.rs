//! snapclone-session — transport selection and retry backoff.
//!
//! Combines snapclone-peer (session state machine) with snapclone-net-intel
//! (reachability memory) to make intelligent connection decisions.
//!
//! # Connection escalation ladder (NETWORK_ARCHITECTURE.md §9)
//!
//! ```text
//! Attempt 1: Direct UDP (if warm path exists in ReachabilityDb)
//! Attempt 2: Direct TCP WebSocket  
//! Attempt 3: Assistant-assisted punch (if 3rd-device available)
//! Attempt 4: DHT mailbox rendezvous (deferred)
//! Attempt 5: Relay (last resort)
//! ```
//!
//! Backoff: 1s → 2s → 4s → 8s → 30s (exponential, capped at 30s)

pub mod backoff;
pub mod selector;
pub mod coordinator;

pub use backoff::RetryBackoff;
pub use selector::{TransportSelector, TransportChoice};
pub use coordinator::SessionCoordinator;
