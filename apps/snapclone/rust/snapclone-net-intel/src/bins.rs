//! Network binning — group IPs by subnet for generalized learning.
//!
//! NETWORK_ARCHITECTURE.md §2: "Instead of tracking individual IPs, group by
//! subnet, ASN, interface type, NAT classification."

use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// A grouping of IP addresses for learning purposes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NetworkBin {
    /// /24 subnet (e.g. 192.168.1.0/24)
    Subnet24([u8; 3]),
    /// /20 subnet
    Subnet20([u8; 3]),
    /// Carrier-grade NAT range (100.64/10)
    Cgnat,
    /// Private RFC 1918
    PrivateLan,
    /// IPv6 /48 prefix
    Ipv6Prefix48([u8; 6]),
    /// Unknown / unclassifiable
    Unknown,
}

impl NetworkBin {
    pub fn of(addr: IpAddr) -> Self {
        match addr {
            IpAddr::V4(v4) => {
                let o = v4.octets();
                // CGNAT
                if o[0] == 100 && o[1] >= 64 && o[1] <= 127 { return Self::Cgnat; }
                // RFC 1918
                if o[0] == 10 || (o[0]==172 && o[1]>=16 && o[1]<=31)
                              || (o[0]==192 && o[1]==168) {
                    return Self::PrivateLan;
                }
                Self::Subnet24([o[0], o[1], o[2]])
            }
            IpAddr::V6(v6) => {
                let b = v6.octets();
                Self::Ipv6Prefix48([b[0],b[1],b[2],b[3],b[4],b[5]])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    #[test]
    fn cgnat_binned() {
        let ip = IpAddr::from_str("100.72.0.1").unwrap();
        assert_eq!(NetworkBin::of(ip), NetworkBin::Cgnat);
    }
    #[test]
    fn rfc1918_binned() {
        let ip = IpAddr::from_str("192.168.1.50").unwrap();
        assert_eq!(NetworkBin::of(ip), NetworkBin::PrivateLan);
    }
}
