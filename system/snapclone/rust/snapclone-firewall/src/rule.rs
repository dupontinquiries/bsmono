//! Firewall rule descriptor — what we want to open.

/// A port-allow rule to apply to the firewall.
#[derive(Debug, Clone)]
pub struct FirewallRule {
    pub port:     u16,
    pub protocol: Protocol,
    pub comment:  String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol { Tcp, Udp, Both }

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self { Self::Tcp => "tcp", Self::Udp => "udp", Self::Both => "tcp+udp" }
    }
}

impl FirewallRule {
    /// The default SnapClone rule — open port 9876 for both TCP and UDP.
    pub fn snapclone_default() -> Self {
        Self {
            port:     9876,
            protocol: Protocol::Both,
            comment:  "SnapClone peer-to-peer".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_as_str() {
        assert_eq!(Protocol::Tcp.as_str(), "tcp");
        assert_eq!(Protocol::Udp.as_str(), "udp");
        assert_eq!(Protocol::Both.as_str(), "tcp+udp");
    }

    #[test]
    fn default_rule_is_9876_both() {
        let r = FirewallRule::snapclone_default();
        assert_eq!(r.port, 9876);
        assert_eq!(r.protocol, Protocol::Both);
        assert!(!r.comment.is_empty());
    }
}
