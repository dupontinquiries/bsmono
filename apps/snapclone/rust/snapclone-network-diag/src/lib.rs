//! snapclone-network-diag — async network connectivity checks.
//!
//! Rust port of NetworkDiagnostics.cpp (491 lines).
//! Each check is an async function that resolves with a `CheckResult`.
//! All checks run in parallel via `tokio::join!` or `futures::join_all`.
//!
//! # Checks
//! | C++ method                   | Rust function          | Status |
//! |------------------------------|------------------------|--------|
//! | checkUdpMulticast()          | check_udp_multicast()  | DONE   |
//! | checkTcpListen()             | check_tcp_listen()     | DONE   |
//! | checkLoopbackWebSocket()     | check_ws_loopback()    | scaffold |
//! | checkPublicAddress()         | check_public_ip()      | DONE   |
//! | checkUdpStun()               | check_stun_udp()       | DONE   |

pub mod checks;
pub mod result;
pub mod runner;

pub use result::{CheckResult, CheckStatus, DiagReport};
pub use runner::run_all;
