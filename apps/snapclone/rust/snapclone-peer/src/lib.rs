//! snapclone-peer — peer session lifecycle state machine.
//!
//! Pure Rust port of the session management logic in NetworkManager.cpp.
//! No I/O, no Qt, no tokio — the state machine is entirely driven by events
//! passed in by the caller, which makes it trivially testable.
//!
//! # State machine
//!
//! ```text
//!              ┌──────────┐
//!   connect()  │          │  disconnect()
//!  ──────────► │Connecting│ ◄──────────────┐
//!              │          │                │
//!              └─────┬────┘                │
//!        shak_ok()   │                     │
//!              ┌─────▼────┐  error()       │
//!              │          ├───────────────►│
//!              │ Connected│                │
//!              │          │ disconnect()   │
//!              └─────┬────┘──────────────►┘
//!                    │ relay_activated()
//!              ┌─────▼────┐
//!              │  Relayed │  (connected via relay, not direct)
//!              └──────────┘
//! ```
//!
//! # Usage
//! ```rust
//! use snapclone_peer::{PeerSession, SessionEvent};
//!
//! let mut session = PeerSession::new("sc-aabbccdd");
//! session.apply(SessionEvent::ConnectInitiated { address: "192.168.1.5:9876".to_owned() });
//! session.apply(SessionEvent::ShakCompleted { display_name: "Alice".to_owned() });
//! assert!(session.is_connected());
//! ```

pub mod session;
pub mod pool;
pub mod epoch;

pub use session::{PeerSession, SessionState, SessionEvent, TransportKind};
pub use pool::SessionPool;
