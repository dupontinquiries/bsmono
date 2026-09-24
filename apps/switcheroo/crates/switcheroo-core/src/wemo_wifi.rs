//! wemo_wifi — the Wi-Fi Wemos (Mini Smart Plug, Smart Plug, Insight, Light Switch): local UPnP
//! control, no cloud, no pairing, no crypto. Belkin's protocol is SOAP over HTTP on one of four
//! ports; `SetBinaryState` turns the relay, `GetBinaryState` reads it. std only, so it runs on the
//! Fairphone and the Studio the same way, and is testable against an in-process fake.
//!
//! Not the WSP100: that one is HomeKit-over-Thread/BLE and lives in `hap-core`.
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::time::Duration;

pub const PORTS: [u16; 4] = [49153, 49152, 49154, 49155];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found { pub host: String, pub port: u16, pub friendly_name: String, pub model: String }

fn http(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, &str)], body: &str, timeout: Duration) -> Result<(u16, String), String> {
    let mut s = TcpStream::connect_timeout(&addr, timeout).map_err(|e| format!("connect {addr}: {e}"))?;
    s.set_read_timeout(Some(timeout)).ok(); s.set_write_timeout(Some(timeout)).ok();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in headers { req.push_str(&format!("{k}: {v}\r\n")); }
    req.push_str("\r\n"); req.push_str(body);
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = Vec::new(); let _ = s.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf).to_string();
    let code: u16 = text.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Ok((code, body))
}

fn soap(host: &str, port: u16, service: &str, action: &str, args: &str) -> Result<String, String> {
    let addr = format!("{host}:{port}").to_socket_addrs().map_err(|e| e.to_string())?.next().ok_or("no address")?;
    let body = format!(r#"<?xml version="1.0" encoding="utf-8"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><u:{action} xmlns:u="urn:Belkin:service:{service}:1">{args}</u:{action}></s:Body></s:Envelope>"#);
    let soapaction = format!("\"urn:Belkin:service:{service}:1#{action}\"");
    let (code, resp) = http(addr, "POST", &format!("/upnp/control/{service}1"), &[("Content-Type", "text/xml; charset=\"utf-8\""), ("SOAPACTION", &soapaction)], &body, Duration::from_secs(4))?;
    if code != 200 { return Err(format!("{action}: HTTP {code}")); }
    Ok(resp)
}

fn tag(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>"); let close = format!("</{name}>");
    let a = xml.find(&open)? + open.len(); let b = xml[a..].find(&close)? + a;
    Some(xml[a..b].trim().to_string())
}

/// Try the four ports the firmware picks between (it changes after reboots) and remember the one that answers.
pub fn find_port(host: &str) -> Option<u16> {
    PORTS.iter().copied().find(|&p| soap(host, p, "basicevent", "GetBinaryState", "").is_ok())
}

/// 0 = off, 1 = on, 8 = on-but-standby (Insight). Anything else is the raw value.
pub fn get_state(host: &str, port: u16) -> Result<u8, String> {
    let r = soap(host, port, "basicevent", "GetBinaryState", "")?;
    tag(&r, "BinaryState").and_then(|s| s.parse().ok()).ok_or_else(|| "no BinaryState in reply".into())
}

/// Returns the state the accessory reports AFTER the write, not the one we asked for.
pub fn set_state(host: &str, port: u16, on: bool) -> Result<u8, String> {
    let r = soap(host, port, "basicevent", "SetBinaryState", &format!("<BinaryState>{}</BinaryState>", if on { 1 } else { 0 }))?;
    // Some firmware answers "Error" in BinaryState when the relay is already in that state; read back.
    let _ = tag(&r, "BinaryState");
    get_state(host, port)
}

pub fn friendly_name(host: &str, port: u16) -> Result<String, String> {
    let r = soap(host, port, "basicevent", "GetFriendlyName", "")?;
    tag(&r, "FriendlyName").ok_or_else(|| "no FriendlyName".into())
}

/// SSDP discovery: `M-SEARCH` for Belkin devices, parse LOCATION headers, confirm each with GetFriendlyName.
pub fn discover(wait: Duration) -> Vec<Found> {
    let mut out = Vec::new();
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else { return out };
    let _ = sock.set_read_timeout(Some(Duration::from_millis(500)));
    let msearch = "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: urn:Belkin:device:**\r\n\r\n";
    let _ = sock.send_to(msearch.as_bytes(), "239.255.255.250:1900");
    let t0 = std::time::Instant::now();
    let mut seen: Vec<(String, u16)> = Vec::new();
    while t0.elapsed() < wait {
        let mut buf = [0u8; 2048];
        let Ok((n, _)) = sock.recv_from(&mut buf) else { continue };
        let text = String::from_utf8_lossy(&buf[..n]).to_string();
        let Some(loc) = text.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("location:").map(|_| l.split_once(':').unwrap().1.trim().to_string())) else { continue };
        // http://host:port/setup.xml
        let Some(hp) = loc.strip_prefix("http://").and_then(|r| r.split('/').next()) else { continue };
        let (host, port) = match hp.rsplit_once(':') { Some((h, p)) => (h.to_string(), p.parse().unwrap_or(49153)), None => (hp.to_string(), 49153) };
        if seen.contains(&(host.clone(), port)) { continue; }
        seen.push((host.clone(), port));
        let model = text.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("st:").map(|_| l.split_once(':').unwrap().1.trim().to_string())).unwrap_or_default();
        if let Ok(name) = friendly_name(&host, port) { out.push(Found { host, port, friendly_name: name, model }); }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A fake Wemo: answers GetBinaryState / SetBinaryState / GetFriendlyName on a random port.
    fn fake() -> (u16, std::thread::JoinHandle<()>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap(); let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let mut state = 0u8;
            for stream in l.incoming().take(6) {
                let mut s = stream.unwrap(); let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap(); let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let body = if req.contains("SetBinaryState") { state = if req.contains("<BinaryState>1</BinaryState>") { 1 } else { 0 }; format!("<BinaryState>{state}</BinaryState>") }
                    else if req.contains("GetFriendlyName") { "<FriendlyName>Lamp</FriendlyName>".into() }
                    else { format!("<BinaryState>{state}</BinaryState>") };
                let resp = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}", body.len(), body);
                s.write_all(resp.as_bytes()).unwrap();
            }
        });
        (port, h)
    }

    #[test]
    fn set_and_read_back_against_a_fake_plug() {
        let (port, h) = fake();
        assert_eq!(get_state("127.0.0.1", port).unwrap(), 0);
        assert_eq!(set_state("127.0.0.1", port, true).unwrap(), 1);
        assert_eq!(friendly_name("127.0.0.1", port).unwrap(), "Lamp");
        assert_eq!(set_state("127.0.0.1", port, false).unwrap(), 0);
        drop(h);
    }
}
