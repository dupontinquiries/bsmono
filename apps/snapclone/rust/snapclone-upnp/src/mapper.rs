//! High-level PortMapper — coordinates SSDP, IGD, and NAT-PMP.

use crate::{igd, natpmp, ssdp};
use anyhow::Result;
use std::net::Ipv4Addr;
use tracing::{info, warn};

pub const LEASE_SECS:  u32 = 7200; // 2-hour lease, matches C++
pub const RENEW_SECS:  u64 = 600;  // renew at 10 min to give buffer

#[derive(Debug, Clone)]
pub enum MappingResult {
    Upnp  { external_port: u16, control_url: String },
    NatPmp{ external_port: u16 },
    None,
}

/// Try to open a port mapping using UPnP then NAT-PMP as fallback.
/// This is the Rust equivalent of UPnPPortMapper::start() in C++.
pub async fn open_mapping(
    internal_ip: &str,
    port: u16,
    gateway: Option<Ipv4Addr>,
) -> Result<MappingResult> {
    // Step 1: SSDP discovery
    match ssdp::search().await? {
        Some(ssdp_resp) => {
            // Step 2: fetch IGD description
            let xml = fetch_description(&ssdp_resp.location).await
                .unwrap_or_default();
            if let Some(service) = igd::parse_description(&xml, &ssdp_resp.location) {
                // Step 3: add port mapping
                match igd::add_mapping(&service, internal_ip, port, LEASE_SECS).await {
                    Ok(()) => {
                        info!("UPnP mapping established on port {}", port);
                        return Ok(MappingResult::Upnp {
                            external_port: port,
                            control_url: service.control_url,
                        });
                    }
                    Err(e) => warn!("UPnP AddPortMapping failed: {} — trying NAT-PMP", e),
                }
            }
        }
        None => info!("No UPnP IGD found — trying NAT-PMP"),
    }

    // Step 4: NAT-PMP fallback
    if let Some(gw) = gateway {
        match natpmp::request(gw, port, LEASE_SECS).await {
            Ok(ext_port) => return Ok(MappingResult::NatPmp { external_port: ext_port }),
            Err(e) => warn!("NAT-PMP failed: {}", e),
        }
    }

    Ok(MappingResult::None)
}

async fn fetch_description(url: &str) -> Result<String> {
    // Simple blocking HTTP GET (avoids reqwest dependency)
    use std::io::{Read, Write};
    let no_scheme = url.strip_prefix("http://").unwrap_or(url);
    let (host_port, path) = no_scheme.split_once('/').unwrap_or((no_scheme, ""));
    let path = format!("/{}", path);
    let mut stream = std::net::TcpStream::connect(host_port)?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(3)))?;
    let req = format!("GET {} HTTP/1.0\r\nHost: {}\r\n\r\n", path, host_port);
    stream.write_all(req.as_bytes())?;
    let mut resp = String::new();
    stream.read_to_string(&mut resp)?;
    // Strip HTTP headers
    Ok(resp.split_once("\r\n\r\n").map(|(_, body)| body).unwrap_or(&resp).to_owned())
}
