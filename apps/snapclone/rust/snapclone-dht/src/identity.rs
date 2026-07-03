//! Stable sc-ID identity — the "account" for a SnapClone device.
//!
//! The sc-ID is a 32-bit random value formatted as "sc-XXXXXXXX" (8 hex chars).
//! It is generated once on first launch and persisted to disk.
//! It is the primary key for contacts, sessions, and DHT routing.
//!
//! Ported from the identity setup in AppController::initialize().

use anyhow::{Context, Result};
use rand::Rng;
use std::path::Path;

/// A validated stable sc-ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScId(String);

impl ScId {
    /// Generate a new random sc-ID.
    pub fn generate() -> Self {
        let n: u32 = rand::thread_rng().gen();
        Self(format!("sc-{:08x}", n))
    }

    /// Parse and validate an existing sc-ID string.
    pub fn parse(s: &str) -> Option<Self> {
        if s.len() == 11
            && s.starts_with("sc-")
            && s[3..].chars().all(|c| c.is_ascii_hexdigit())
        {
            Some(Self(s.to_owned()))
        } else {
            None
        }
    }

    pub fn as_str(&self) -> &str { &self.0 }

    /// Extract a sc-ID from a display string like "user@ip (sc-32a51661)".
    pub fn extract_from_display(display: &str) -> Option<Self> {
        let start = display.find("(sc-")?;
        let end   = display[start..].find(')')? + start;
        Self::parse(&display[start + 1..end])
    }

    /// Strip ip/sc-id suffix to get just the username.
    /// "kesslerd@192.168.1.5 (sc-32a51661)" → "kesslerd"
    pub fn extract_username(real_id: &str) -> String {
        let base = real_id.find(" (").map(|i| &real_id[..i]).unwrap_or(real_id);
        let user = base.find('@').map(|i| &base[..i]).unwrap_or(base);
        if user.is_empty() { real_id.to_owned() } else { user.to_owned() }
    }
}

impl std::fmt::Display for ScId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Persistent device identity — sc-ID plus display name.
#[derive(Debug, Clone)]
pub struct Identity {
    pub sc_id:        ScId,
    pub display_name: String,
}

impl Identity {
    /// Load existing identity from `path`, or generate and save a new one.
    pub fn load_or_generate(path: &Path) -> Result<Self> {
        if path.exists() {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Reading identity file {:?}", path))?;
            let sc_id = ScId::parse(content.trim())
                .ok_or_else(|| anyhow::anyhow!("Invalid sc-ID in {:?}", path))?;
            return Ok(Self { sc_id, display_name: String::new() });
        }

        // First launch — generate
        let sc_id = ScId::generate();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Creating identity dir {:?}", parent))?;
        }
        std::fs::write(path, sc_id.as_str())
            .with_context(|| format!("Writing identity to {:?}", path))?;

        Ok(Self { sc_id, display_name: String::new() })
    }

    /// The full DHT routing string used as realId in the C++ layer.
    /// Format: "username@ip (sc-XXXXXXXX)"
    pub fn realid(&self, ip: &str) -> String {
        format!("{} {}@{} ({})",
            if self.display_name.is_empty() { "user" } else { &self.display_name },
            whoami_username(), ip, self.sc_id
        )
    }
}

fn whoami_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "user".to_owned())
}

pub mod resolver {
    //! sc-ID → network address resolution.
    //! Scaffold for AppController::connectByDhtId() Rust port.

    /// Result of a DHT lookup for a sc-ID.
    #[derive(Debug, Clone)]
    pub struct PeerAddr {
        pub sc_id: String,
        pub host:  String,
        pub port:  u16,
    }

    /// Resolve a sc-ID to a network address via the DHT.
    /// Async because DHT lookups are inherently latency-bound.
    /// Returns `None` if the peer is not found after timeout.
    pub async fn resolve(_sc_id: &str) -> Option<PeerAddr> {
        // TODO: implement via JamiAdapter / OpenDHT bridge
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_valid() {
        let id = ScId::generate();
        assert!(ScId::parse(id.as_str()).is_some());
    }

    #[test]
    fn extract_from_display_string() {
        let id = ScId::extract_from_display("kesslerd@192.168.1.5 (sc-32a51661)");
        assert_eq!(id.as_deref().map(|s| s.as_str()), Some("sc-32a51661"));
    }

    #[test]
    fn extract_username() {
        assert_eq!(ScId::extract_username("kesslerd@192.168.1.5 (sc-32a51661)"), "kesslerd");
    }
}
