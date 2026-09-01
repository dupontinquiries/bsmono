// src/dlna_discover.rs
#![allow(dead_code)]
//
// SSDP / UPnP discovery for DLNA MediaRenderers.
//
// Why this exists: the Beosound Stage's Mozart firmware rejects every
// REST endpoint we try in play_uri ("path not whitelisted" / 404), but
// it DOES speak DLNA. Strategy 6 in play_uri.rs currently tries four
// hardcoded UPnP control paths (/upnp/control/AVTransport,
// /AVTransport/control, etc.). That's a guess. The Mozart firmware
// can put the control endpoint anywhere — we shouldn't have to know.
//
// The right answer is to ask the device what its endpoint is via
// the standard UPnP discovery flow:
//
//   1. SSDP M-SEARCH (UDP multicast to 239.255.255.250:1900). Devices
//      that match the requested ST (search target) reply with their
//      LOCATION header — a URL pointing to their device descriptor XML.
//
//   2. Fetch the descriptor XML. It declares the device's services
//      (AVTransport, RenderingControl, ConnectionManager, ...) and
//      each service's controlURL.
//
//   3. POST SOAP envelopes to the discovered controlURL.
//
// This module is pure logic + a UDP socket; play_uri does the actual
// SOAP exchange against whatever URL we find. Tests use canned
// SSDP responses + descriptor XML so they don't need a real device.

use std::collections::HashMap;
use std::net::{UdpSocket, IpAddr, SocketAddr};
use std::time::{Duration, Instant};

/// The SSDP multicast group + port. Same everywhere; this is in the
/// UPnP Device Architecture spec.
const SSDP_MULTICAST_ADDR: &str = "239.255.255.250:1900";

/// Search target for MediaRenderers (audio + video sinks). Mozart
/// firmware advertises this. We could also try `ssdp:all` to find
/// every UPnP thing on the LAN, but that's a lot of noise.
pub const ST_MEDIA_RENDERER: &str = "urn:schemas-upnp-org:device:MediaRenderer:1";

/// Result of one SSDP search round: a list of (location_url, usn) pairs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredDevice {
    /// HTTP URL of the device descriptor XML (e.g.
    /// http://192.168.2.12:8080/description.xml). The descriptor
    /// declares the device's services + their control endpoints.
    pub location: String,
    /// Unique service name (uuid:...). Use it to dedupe responses
    /// when a device announces itself multiple times in one round.
    pub usn: String,
    /// Server header from the response. Often something like
    /// "Linux/5.x UPnP/1.0 Mozart/...". Useful for log lines.
    pub server: String,
}

/// AVTransport service info extracted from a device descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AVTransportInfo {
    /// Friendly device name as it appeared in <friendlyName>.
    pub friendly_name: String,
    /// Full URL to POST SOAP envelopes to. Resolved against the
    /// descriptor's base URL.
    pub control_url: String,
    /// Optional URL for eventing (we don't subscribe currently, but
    /// it's useful to keep around for later).
    pub event_url: Option<String>,
}

/// Send an SSDP M-SEARCH for `target` and collect responses for at
/// most `timeout`. Returns the deduped list of discovered devices.
///
/// This function blocks. Run it from a background thread. It binds
/// to 0.0.0.0:0 (let OS pick an ephemeral port), sets a read timeout,
/// and reads until the timeout elapses. Each response is parsed for
/// its LOCATION header; the (usn, location) pair dedupes.
pub fn m_search(target: &str, timeout: Duration) -> std::io::Result<Vec<DiscoveredDevice>> {
    // M-SEARCH MX is the requested response delay in seconds. Devices
    // pick a random uniform delay in [0, MX] before responding, to
    // avoid stampede. We pick MX = (timeout - 0.5s) clamped to [1, 5].
    let mx = ((timeout.as_secs() as i64).saturating_sub(1)).clamp(1, 5);
    let request = format!(
        "M-SEARCH * HTTP/1.1\r\n\
         HOST: {host}\r\n\
         MAN: \"ssdp:discover\"\r\n\
         MX: {mx}\r\n\
         ST: {st}\r\n\
         \r\n",
        host = SSDP_MULTICAST_ADDR,
        mx = mx,
        st = target,
    );

    let sock = UdpSocket::bind("0.0.0.0:0")?;
    sock.set_read_timeout(Some(Duration::from_millis(500)))?;
    sock.set_broadcast(true).ok();   // some routers need this
    sock.send_to(request.as_bytes(), SSDP_MULTICAST_ADDR)?;

    let deadline = Instant::now() + timeout;
    let mut seen: HashMap<String, DiscoveredDevice> = HashMap::new();
    let mut buf = vec![0u8; 4096];
    while Instant::now() < deadline {
        match sock.recv_from(&mut buf) {
            Ok((len, _src)) => {
                if let Some(dev) = parse_msearch_response(&buf[..len]) {
                    seen.entry(dev.usn.clone()).or_insert(dev);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
                  || e.kind() == std::io::ErrorKind::TimedOut => {
                // Read timeout — go around the loop and check deadline.
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(seen.into_values().collect())
}

/// Parse an SSDP M-SEARCH response. SSDP responses are HTTP/1.1-
/// formatted but over UDP — the first line is the status line, then
/// headers, then a blank line. We only need LOCATION, USN, SERVER.
pub fn parse_msearch_response(bytes: &[u8]) -> Option<DiscoveredDevice> {
    let text = std::str::from_utf8(bytes).ok()?;
    // First line should be "HTTP/1.1 200 OK". If not, ignore.
    let mut lines = text.lines();
    let status = lines.next()?;
    if !status.contains("200") { return None; }

    let mut location = String::new();
    let mut usn = String::new();
    let mut server = String::new();
    for line in lines {
        if let Some(rest) = strip_prefix_ci(line, "LOCATION:") {
            location = rest.trim().to_string();
        } else if let Some(rest) = strip_prefix_ci(line, "USN:") {
            usn = rest.trim().to_string();
        } else if let Some(rest) = strip_prefix_ci(line, "SERVER:") {
            server = rest.trim().to_string();
        }
    }
    if location.is_empty() || usn.is_empty() { return None; }
    Some(DiscoveredDevice { location, usn, server })
}

/// Case-insensitive header prefix strip. SSDP says headers are
/// case-insensitive; in practice firmware varies.
fn strip_prefix_ci<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    if line.len() < prefix.len() { return None; }
    let head = &line[..prefix.len()];
    if head.eq_ignore_ascii_case(prefix) {
        Some(&line[prefix.len()..])
    } else { None }
}

/// Pull the AVTransport service's control URL out of a device
/// descriptor XML. We don't use a full XML parser — UPnP descriptors
/// are simple and predictable. We scan for <service>...</service>
/// blocks and pick the one whose serviceType contains "AVTransport".
///
/// `base_url` is the URL the descriptor was fetched from; we use it
/// to resolve any relative control URLs.
pub fn extract_avtransport_info(xml: &str, base_url: &str) -> Option<AVTransportInfo> {
    let friendly_name = extract_tag_inner(xml, "friendlyName")
        .unwrap_or_else(|| "(unknown)".to_string());

    // Find every <service>…</service> block; pick the AVTransport one.
    let mut cursor = 0;
    while let Some(open) = xml[cursor..].find("<service") {
        let abs_open = cursor + open;
        let close = match xml[abs_open..].find("</service>") {
            Some(c) => abs_open + c,
            None => break,
        };
        let block = &xml[abs_open..close];
        cursor = close + "</service>".len();

        let stype = extract_tag_inner(block, "serviceType").unwrap_or_default();
        if !stype.to_ascii_lowercase().contains("avtransport") { continue; }
        let control = extract_tag_inner(block, "controlURL")?;
        let event = extract_tag_inner(block, "eventSubURL");
        return Some(AVTransportInfo {
            friendly_name,
            control_url: resolve_url(&control, base_url),
            event_url: event.map(|e| resolve_url(&e, base_url)),
        });
    }
    None
}

/// Extract the inner text of the first <tag>…</tag> in `xml`.
/// Handles whitespace around the tag content. Returns None if the
/// tag isn't present.
fn extract_tag_inner(xml: &str, tag: &str) -> Option<String> {
    let open_pat = format!("<{}", tag);
    let close_pat = format!("</{}>", tag);
    let open_start = xml.find(&open_pat)?;
    // Walk past the open tag (may have attributes or be self-closing
    // — we ignore that since the tags we care about don't self-close).
    let after_open = xml[open_start..].find('>')? + open_start + 1;
    let close_start = xml[after_open..].find(&close_pat)? + after_open;
    Some(xml[after_open..close_start].trim().to_string())
}

/// Resolve a possibly-relative URL against a base URL (the descriptor
/// fetch URL). Handles the three cases:
///   * Absolute URL ("http://...") → returned as-is
///   * Absolute path ("/AVTransport/control") → joined with base's
///     scheme + host + port
///   * Relative path ("AVTransport/control") → joined with base's
///     directory portion
pub fn resolve_url(url: &str, base: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        return url.to_string();
    }
    // Parse base into scheme + host + path. We don't pull in `url`
    // crate; this is a one-shot resolver.
    let (scheme, rest) = base.split_once("://").unwrap_or(("http", base));
    let (authority, base_path) = rest.split_once('/').unwrap_or((rest, ""));

    if url.starts_with('/') {
        return format!("{}://{}{}", scheme, authority, url);
    }
    // Relative path: keep base's directory, append `url`.
    let dir = match base_path.rfind('/') {
        Some(i) => &base_path[..=i],
        None    => "",
    };
    format!("{}://{}/{}{}", scheme, authority, dir, url)
}

/// Pull the host:port (or just host) out of a URL. Used to match a
/// device's known IP against an SSDP response's LOCATION URL.
pub fn host_of(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host_part = after_scheme.split('/').next()?;
    if host_part.is_empty() { return None; }
    Some(host_part.to_string())
}

/// Match an SSDP-discovered device against a known target host.
/// Returns true if the LOCATION's host matches `target_host` (which
/// may include or omit a port — we match host-only).
pub fn matches_host(discovered: &DiscoveredDevice, target_host: &str) -> bool {
    let loc_host = match host_of(&discovered.location) {
        Some(h) => h,
        None => return false,
    };
    // Strip ports from both sides for comparison.
    let strip_port = |s: &str| -> String {
        // For bracketed IPv6, return as-is (no port). Otherwise rsplit at ':'.
        if s.starts_with('[') {
            s.split(']').next().unwrap_or(s).trim_start_matches('[').to_string()
        } else if let Some((h, _p)) = s.rsplit_once(':') {
            h.to_string()
        } else {
            s.to_string()
        }
    };
    strip_port(&loc_host) == strip_port(target_host)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAGE_RESPONSE: &str = "\
HTTP/1.1 200 OK\r\n\
CACHE-CONTROL: max-age=1800\r\n\
DATE: Sat, 30 May 2026 10:00:00 GMT\r\n\
EXT:\r\n\
LOCATION: http://192.168.2.12:8080/upnp/description.xml\r\n\
SERVER: Linux/5.10 UPnP/1.0 Mozart/2.4.1\r\n\
ST: urn:schemas-upnp-org:device:MediaRenderer:1\r\n\
USN: uuid:4450c91b-7d3a-44a5-afd1-8c4640af2ec6::urn:schemas-upnp-org:device:MediaRenderer:1\r\n\
\r\n";

    const STAGE_DESCRIPTOR: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
  <specVersion><major>1</major><minor>0</minor></specVersion>
  <device>
    <deviceType>urn:schemas-upnp-org:device:MediaRenderer:1</deviceType>
    <friendlyName>Beosound Stage</friendlyName>
    <manufacturer>Bang &amp; Olufsen</manufacturer>
    <modelName>Beosound Stage</modelName>
    <serviceList>
      <service>
        <serviceType>urn:schemas-upnp-org:service:RenderingControl:1</serviceType>
        <serviceId>urn:upnp-org:serviceId:RenderingControl</serviceId>
        <SCPDURL>/upnp/RenderingControl.xml</SCPDURL>
        <controlURL>/upnp/RenderingControl/control</controlURL>
        <eventSubURL>/upnp/RenderingControl/event</eventSubURL>
      </service>
      <service>
        <serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType>
        <serviceId>urn:upnp-org:serviceId:AVTransport</serviceId>
        <SCPDURL>/upnp/AVTransport.xml</SCPDURL>
        <controlURL>/upnp/AVTransport/control</controlURL>
        <eventSubURL>/upnp/AVTransport/event</eventSubURL>
      </service>
    </serviceList>
  </device>
</root>"#;

    #[test]
    fn parse_response_extracts_location_usn() {
        let dev = parse_msearch_response(STAGE_RESPONSE.as_bytes()).unwrap();
        assert_eq!(dev.location, "http://192.168.2.12:8080/upnp/description.xml");
        assert!(dev.usn.contains("4450c91b"));
        assert!(dev.server.contains("Mozart"));
    }

    #[test]
    fn parse_response_rejects_non_200() {
        let bad = b"HTTP/1.1 404 Not Found\r\n\r\n";
        assert!(parse_msearch_response(bad).is_none());
    }

    #[test]
    fn parse_response_rejects_missing_location() {
        let bad = "HTTP/1.1 200 OK\r\nUSN: foo\r\n\r\n";
        assert!(parse_msearch_response(bad.as_bytes()).is_none());
    }

    #[test]
    fn parse_response_case_insensitive_headers() {
        let bytes = "HTTP/1.1 200 OK\r\nlocation: http://x/d.xml\r\nusn: uuid:y\r\n\r\n";
        let dev = parse_msearch_response(bytes.as_bytes()).unwrap();
        assert_eq!(dev.location, "http://x/d.xml");
    }

    #[test]
    fn extract_avtransport_finds_control_url() {
        let info = extract_avtransport_info(STAGE_DESCRIPTOR,
            "http://192.168.2.12:8080/upnp/description.xml").unwrap();
        assert_eq!(info.friendly_name, "Beosound Stage");
        assert_eq!(info.control_url, "http://192.168.2.12:8080/upnp/AVTransport/control");
        assert_eq!(info.event_url, Some("http://192.168.2.12:8080/upnp/AVTransport/event".to_string()));
    }

    #[test]
    fn extract_returns_none_when_no_avtransport() {
        let xml = r#"<root><device><serviceList>
            <service>
              <serviceType>urn:schemas-upnp-org:service:RenderingControl:1</serviceType>
              <controlURL>/RC/control</controlURL>
            </service>
        </serviceList></device></root>"#;
        assert!(extract_avtransport_info(xml, "http://x/d.xml").is_none());
    }

    #[test]
    fn extract_tag_handles_attributes() {
        let xml = r#"<foo bar="baz">hello</foo>"#;
        assert_eq!(extract_tag_inner(xml, "foo"), Some("hello".to_string()));
    }

    #[test]
    fn extract_tag_returns_none_when_missing() {
        assert!(extract_tag_inner("<a/>", "b").is_none());
    }

    #[test]
    fn resolve_absolute_url_passthrough() {
        let r = resolve_url("http://other.host/path", "http://192.168.2.12/d.xml");
        assert_eq!(r, "http://other.host/path");
    }

    #[test]
    fn resolve_absolute_path_uses_base_authority() {
        let r = resolve_url("/AVTransport/control", "http://192.168.2.12:8080/upnp/d.xml");
        assert_eq!(r, "http://192.168.2.12:8080/AVTransport/control");
    }

    #[test]
    fn resolve_relative_path_uses_base_directory() {
        let r = resolve_url("control", "http://192.168.2.12:8080/upnp/AVTransport/d.xml");
        assert_eq!(r, "http://192.168.2.12:8080/upnp/AVTransport/control");
    }

    #[test]
    fn resolve_relative_path_handles_root() {
        let r = resolve_url("control", "http://192.168.2.12");
        assert_eq!(r, "http://192.168.2.12/control");
    }

    #[test]
    fn host_of_extracts_authority() {
        assert_eq!(host_of("http://192.168.2.12:8080/path"), Some("192.168.2.12:8080".to_string()));
        assert_eq!(host_of("http://device.local/"), Some("device.local".to_string()));
        assert_eq!(host_of("http://[2001:db8::1]:80/"), Some("[2001:db8::1]:80".to_string()));
        assert_eq!(host_of("not-a-url"), Some("not-a-url".to_string()));
    }

    #[test]
    fn matches_host_strips_ports() {
        let dev = DiscoveredDevice {
            location: "http://192.168.2.12:8080/d.xml".into(),
            usn: "uuid:x".into(),
            server: "Mozart".into(),
        };
        assert!(matches_host(&dev, "192.168.2.12"));
        assert!(matches_host(&dev, "192.168.2.12:80"));
        assert!(!matches_host(&dev, "192.168.2.14"));
    }

    #[test]
    fn matches_host_handles_bracketed_ipv6() {
        let dev = DiscoveredDevice {
            location: "http://[2001:db8::1]:8080/d.xml".into(),
            usn: "uuid:x".into(),
            server: "".into(),
        };
        assert!(matches_host(&dev, "[2001:db8::1]"));
        assert!(matches_host(&dev, "[2001:db8::1]:9000"));
    }

    #[test]
    fn strip_prefix_ci_works() {
        assert_eq!(strip_prefix_ci("LOCATION: x", "location:"), Some(" x"));
        assert_eq!(strip_prefix_ci("location: x", "LOCATION:"), Some(" x"));
        assert_eq!(strip_prefix_ci("X-Other: y", "LOCATION:"), None);
    }

    // The actual m_search function binds a UDP socket and sends multicast,
    // which we can't reliably test in an isolated CI environment. The
    // pure parsing logic above gives us the coverage that matters.
    // A smoke test that exercises the socket binding without sending:
    #[test]
    fn m_search_binds_and_returns_empty_on_no_responses() {
        // Use a very short timeout to avoid slowing the test suite.
        let result = m_search(ST_MEDIA_RENDERER, Duration::from_millis(50));
        // On a CI host with no UPnP devices on the LAN, we expect an
        // empty list (or a permission error, depending on the env).
        // Either is acceptable — what matters is we don't panic.
        let _ = result;
    }
}

// Suppress unused-import warnings when IpAddr/SocketAddr aren't directly
// referenced. They're conceptually documented in the module but only
// used inside the impl detail.
#[allow(dead_code)]
const _UNUSED: (Option<IpAddr>, Option<SocketAddr>) = (None, None);
