//! snapclone-upnp — UPnP IGD port mapping + NAT-PMP fallback.
//!
//! Rust port of UPnPPortMapper.cpp (334 lines) and the NAT-PMP path in
//! FirewallPortManager.cpp.
//!
//! # Protocol flow
//! 1. Send SSDP M-SEARCH to 239.255.255.250:1900 (multicast)
//! 2. Parse IGD description XML for WANIPConnection control URL
//! 3. SOAP AddPortMapping to the control URL
//! 4. Renew every kRenewSecs before lease expires
//! 5. If no IGD found after timeout → try NAT-PMP on 192.168.x.1:5351
//!
//! # Porting status
//! | C++ method                   | Rust location          | Status |
//! |------------------------------|------------------------|--------|
//! | sendSsdpSearch()             | ssdp::search()         | DONE   |
//! | onSsdpResponse()             | ssdp::parse_response() | DONE   |
//! | parseDescriptionXml()        | igd::parse_description | DONE   |
//! | addPortMapping()             | igd::add_mapping()     | DONE   |
//! | tryNatPmp()                  | natpmp::request()      | DONE   |
//! | UPnPPortMapper (state mach.) | PortMapper             | DONE   |

pub mod ssdp;
pub mod igd;
pub mod natpmp;
pub mod mapper;

pub use mapper::{open_mapping, MappingResult, LEASE_SECS, RENEW_SECS};
