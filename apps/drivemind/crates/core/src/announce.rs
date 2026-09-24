//! announce — one status line to whoever is listening, std only. The wire form is the one
//! logscope's `hive::status` and hatch's status feed already speak: `{"at","source","level","text"}`
//! POSTed as JSON to `<url>/status`. `level` is fact / inference / unknown; drivemind only ever says
//! facts. A failed announce is reported, never fatal: a phone that is off must not fail a scrub.
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

pub fn status_json(source: &str, text: &str) -> String {
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    serde_json::json!({ "at": format!("unix:{at}"), "source": source, "level": "fact", "text": text.chars().take(240).collect::<String>() }).to_string()
}

/// POST the line to `base/status` with an optional bearer token. Returns the HTTP status code.
pub fn post_status(base: &str, token: Option<&str>, source: &str, text: &str) -> Result<u16, String> {
    let url = base.trim_end_matches('/');
    let rest = url.strip_prefix("http://").ok_or("announce: only http:// targets (the shim on the LAN)")?;
    let (hostport, _path) = rest.split_once('/').unwrap_or((rest, ""));
    let addr = if hostport.contains(':') { hostport.to_string() } else { format!("{hostport}:80") };
    let sock = addr.to_socket_addrs().map_err(|e| e.to_string())?.next().ok_or("no address")?;
    let body = status_json(source, text);
    let mut s = TcpStream::connect_timeout(&sock, Duration::from_secs(4)).map_err(|e| format!("connect {addr}: {e}"))?;
    s.set_read_timeout(Some(Duration::from_secs(4))).ok();
    let mut req = format!("POST /status HTTP/1.1\r\nHost: {hostport}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    if let Some(t) = token { req.push_str(&format!("Authorization: Bearer {t}\r\n")); }
    req.push_str("\r\n"); req.push_str(&body);
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = Vec::new(); let _ = s.read_to_end(&mut buf);
    let head = String::from_utf8_lossy(&buf);
    head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or_else(|| "no HTTP status in reply".into())
}
