//! NAT-PMP (RFC 6886) fallback port mapping.
//! Sent when no UPnP IGD is found — common on simpler home routers.

use anyhow::{bail, Result};
use std::net::{Ipv4Addr, SocketAddr};
use tokio::net::UdpSocket;
use tokio::time::{timeout, Duration};
use tracing::{debug, info, warn};

pub const NATPMP_PORT: u16 = 5351;
pub const NATPMP_TIMEOUT_MS: u64 = 2000;

/// Request an external port mapping via NAT-PMP.
/// `gateway` should be the default route IP (usually 192.168.x.1).
pub async fn request(gateway: Ipv4Addr, internal_port: u16, lease_secs: u32) -> Result<u16> {
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    let dest = SocketAddr::new(gateway.into(), NATPMP_PORT);

    // NAT-PMP TCP map request (version=0, opcode=2=TCP)
    let mut req = [0u8; 12];
    req[0] = 0;    // version
    req[1] = 2;    // opcode: map TCP
    // req[2..3] = reserved (0)
    req[4..6].copy_from_slice(&internal_port.to_be_bytes()); // internal port
    req[6..8].copy_from_slice(&internal_port.to_be_bytes()); // suggested external (same)
    req[8..12].copy_from_slice(&lease_secs.to_be_bytes());

    socket.send_to(&req, dest).await?;
    debug!("NAT-PMP: request sent to {}", dest);

    let mut buf = [0u8; 16];
    match timeout(Duration::from_millis(NATPMP_TIMEOUT_MS), socket.recv_from(&mut buf)).await {
        Ok(Ok((n, _))) if n >= 16 => {
            let result_code = u16::from_be_bytes([buf[2], buf[3]]);
            if result_code != 0 {
                bail!("NAT-PMP error code: {}", result_code);
            }
            let external_port = u16::from_be_bytes([buf[12], buf[13]]);
            info!("NAT-PMP: mapped {}→{} (lease {}s)", internal_port, external_port, lease_secs);
            Ok(external_port)
        }
        Ok(Ok((n, _))) => bail!("NAT-PMP response too short: {} bytes", n),
        Ok(Err(e)) => bail!("NAT-PMP recv error: {}", e),
        Err(_) => bail!("NAT-PMP timeout"),
    }
}
