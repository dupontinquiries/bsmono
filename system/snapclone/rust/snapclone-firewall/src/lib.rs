//! snapclone-firewall — Linux firewall detection and port allow.
//!
//! Rust port of FirewallPortManager.cpp (583 lines).
//! Detects which firewall is running and opens the SnapClone port
//! without requiring user interaction (where possible).
//!
//! # Backend priority
//! 1. firewalld  (Fedora/RHEL/openSUSE/Arch)
//! 2. ufw        (Ubuntu/Debian)
//! 3. nftables   (Alpine/postmarketOS — direct nft command)
//! 4. iptables   (legacy fallback)
//! 5. None       (no firewall detected / unsupported)
//!
//! # Porting status
//! | C++ method              | Rust location          | Status |
//! |-------------------------|------------------------|--------|
//! | detectBackend()         | detect::detect()       | DONE   |
//! | openPort() / all paths  | open::open_port()      | DONE   |
//! | backendName()           | Backend::name()        | DONE   |
//! | isRootOrCapable()       | detect::has_caps()     | DONE   |

pub mod detect;
pub mod open;
pub mod rule;

pub use detect::{Backend, detect_backend, has_caps};
pub use open::open_port;
pub use rule::FirewallRule;
