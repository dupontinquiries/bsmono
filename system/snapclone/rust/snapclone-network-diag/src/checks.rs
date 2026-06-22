//! Individual connectivity checks.

use crate::result::CheckResult;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::{net::{TcpListener, UdpSocket}, time::timeout};
use tracing::debug;

/// Check 1: UDP multicast loopback — can we send to 224.0.0.251 and recv it?
pub async fn check_udp_multicast(gossip_port: u16) -> CheckResult {
    let name = format!("UDP Multicast (port {})", gossip_port);
    let socket = match UdpSocket::bind(
        SocketAddr::from(([0u8, 0, 0, 0], gossip_port))
    ).await {
        Ok(s) => s,
        Err(e) => return CheckResult::fail(&name, &e.to_string(),
            "Another process may be using this port"),
    };

    // Send a probe datagram to ourselves via multicast
    let probe = b"SCAP-DIAG";
    if socket.send_to(probe, "224.0.0.251:1900").await.is_err() {
        // Fallback: send to loopback
        let _ = socket.send_to(probe,
            SocketAddr::from(([127, 0, 0, 1], gossip_port))).await;
    }

    let mut buf = [0u8; 64];
    match timeout(Duration::from_millis(300), socket.recv_from(&mut buf)).await {
        Ok(Ok(_)) => CheckResult::pass(&name, "UDP multicast loopback OK"),
        _ => CheckResult::warn(&name, "No loopback echo",
            "Multicast may be disabled on this interface — LAN discovery may not work"),
    }
}

/// Check 2: TCP listen — can we bind the WebSocket port?
pub async fn check_tcp_listen(ws_port: u16) -> CheckResult {
    let name = format!("TCP Listen (port {})", ws_port);
    match TcpListener::bind(SocketAddr::from(([0u8, 0, 0, 0], ws_port))).await {
        Ok(_) => CheckResult::pass(&name, "Port available"),
        Err(e) => CheckResult::fail(&name, &e.to_string(),
            "The port is already in use — restart SnapClone or choose a different port"),
    }
}

/// Check 3: Public IP reachability — can we fetch our public IP?
pub async fn check_public_ip() -> CheckResult {
    let name = "Public IP (ip-api.com)".to_owned();
    use std::io::{Read, Write};

    let result: anyhow::Result<String> = (|| {
        let mut stream = std::net::TcpStream::connect("ip-api.com:80")?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.write_all(b"GET /json/?fields=status,query HTTP/1.0\r\nHost: ip-api.com\r\n\r\n")?;
        let mut resp = String::new();
        stream.read_to_string(&mut resp)?;
        Ok(resp)
    })();

    match result {
        Ok(resp) if resp.contains("\"status\":\"success\"") => {
            let ip = resp.split("\"query\":\"").nth(1)
                .and_then(|s| s.split('"').next())
                .unwrap_or("unknown");
            CheckResult::pass(&name, &format!("Public IP: {}", ip))
        }
        Ok(_) => CheckResult::warn(&name, "Unexpected response from ip-api.com",
            "Internet connectivity may be limited"),
        Err(e) => CheckResult::fail(&name, &e.to_string(),
            "No internet access — relay connections will also fail"),
    }
}

/// Check 4: STUN UDP — can we send a UDP packet to a STUN server?
pub async fn check_stun_udp() -> CheckResult {
    let name = "STUN UDP (stun.l.google.com:19302)".to_owned();
    let socket = match UdpSocket::bind("0.0.0.0:0").await {
        Ok(s) => s,
        Err(e) => return CheckResult::fail(&name, &e.to_string(), "Cannot bind UDP socket"),
    };

    // Minimal STUN binding request (20 bytes)
    let mut req = [0u8; 20];
    req[0] = 0x00; req[1] = 0x01; // Binding Request
    req[4..8].copy_from_slice(&0x2112_A442u32.to_be_bytes()); // magic cookie
    rand_fill(&mut req[8..20]);

    let dest = "stun.l.google.com:19302";
    let addrs: Vec<SocketAddr> = match tokio::net::lookup_host(dest).await {
        Ok(a) => a.filter(|a| a.is_ipv4()).collect(),
        Err(e) => return CheckResult::fail(&name, &e.to_string(), "DNS resolution failed"),
    };
    let Some(addr) = addrs.first().copied() else {
        return CheckResult::fail(&name, "No IPv4 address resolved", "Check DNS");
    };

    let _ = socket.send_to(&req, addr).await;
    let mut buf = [0u8; 64];
    match timeout(Duration::from_secs(3), socket.recv_from(&mut buf)).await {
        Ok(Ok(_)) => CheckResult::pass(&name, "STUN response received — UDP outbound OK"),
        _ => CheckResult::warn(&name, "No STUN response",
            "UDP may be blocked — hole-punching and direct peer connections may fail"),
    }
}

fn rand_fill(buf: &mut [u8]) {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    std::time::SystemTime::now().hash(&mut h);
    let v = h.finish().to_le_bytes();
    for (i, b) in buf.iter_mut().enumerate() { *b = v[i % 8]; }
}
