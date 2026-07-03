//! SSDP M-SEARCH — discover UPnP IGD routers on the LAN.

use anyhow::Result;
use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tokio::time::{timeout, Duration};
use tracing::{debug, warn};

pub const SSDP_ADDR: &str = "239.255.255.250:1900";
pub const SSDP_TIMEOUT_MS: u64 = 4000;

/// Service types to probe, in priority order.
pub const SSDP_TARGETS: &[&str] = &[
    "urn:schemas-upnp-org:service:WANIPConnection:2",
    "urn:schemas-upnp-org:service:WANIPConnection:1",
    "urn:schemas-upnp-org:device:InternetGatewayDevice:1",
    "upnp:rootdevice",
];

/// A parsed SSDP response — router description URL and service type.
#[derive(Debug, Clone)]
pub struct SsdpResponse {
    pub location:     String,
    pub service_type: String,
}

/// Build an SSDP M-SEARCH datagram for the given service type.
pub fn build_msearch(target: &str) -> String {
    format!(
        "M-SEARCH * HTTP/1.1\r\n\
         HOST: 239.255.255.250:1900\r\n\
         MAN: \"ssdp:discover\"\r\n\
         MX: 3\r\n\
         ST: {}\r\n\
         \r\n",
        target
    )
}

/// Send SSDP M-SEARCH for all IGD service types and collect the first response.
pub async fn search() -> Result<Option<SsdpResponse>> {
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    socket.set_broadcast(true)?;
    let dest: SocketAddr = SSDP_ADDR.parse()?;

    for target in SSDP_TARGETS {
        let msg = build_msearch(target);
        socket.send_to(msg.as_bytes(), dest).await?;
        debug!("SSDP M-SEARCH sent for {}", target);
    }

    let mut buf = vec![0u8; 4096];
    match timeout(Duration::from_millis(SSDP_TIMEOUT_MS), socket.recv_from(&mut buf)).await {
        Ok(Ok((n, _from))) => {
            let response = String::from_utf8_lossy(&buf[..n]);
            Ok(parse_response(&response))
        }
        Ok(Err(e)) => { warn!("SSDP recv error: {}", e); Ok(None) }
        Err(_) => { debug!("SSDP timeout — no IGD found"); Ok(None) }
    }
}

/// Parse an SSDP HTTP response to extract LOCATION and ST headers.
pub fn parse_response(response: &str) -> Option<SsdpResponse> {
    if !response.to_ascii_uppercase().starts_with("HTTP/1.1 200") {
        return None;
    }
    let mut location = None;
    let mut service_type = None;
    for line in response.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("location:") {
            location = Some(line[9..].trim().to_owned());
        } else if lower.starts_with("st:") {
            service_type = Some(line[3..].trim().to_owned());
        }
    }
    Some(SsdpResponse {
        location:     location?,
        service_type: service_type.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_ssdp_response() {
        let r = "HTTP/1.1 200 OK\r\nLOCATION: http://192.168.1.1:5555/gatedesc.xml\r\nST: urn:schemas-upnp-org:service:WANIPConnection:1\r\n\r\n";
        let s = parse_response(r).unwrap();
        assert!(s.location.contains("192.168.1.1"));
        assert!(s.service_type.contains("WANIPConnection"));
    }
    #[test]
    fn rejects_non_200() {
        assert!(parse_response("HTTP/1.1 404 Not Found\r\n").is_none());
    }
}
