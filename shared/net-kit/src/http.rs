//! http.rs — a minimal, dependency-free HTTP/1.1 CLIENT: build a request, parse a
//! response, and (the one IO seam) round-trip it over a TCP socket. Generalized
//! from the logscope ollama client so the hand-rolled HTTP scattered across
//! bo-suite and logscope can build on one tested core. The builder and parser are
//! PURE and unit-tested; only [`roundtrip`] touches a socket. Std only.
//!
//! Scope: one request per connection (`Connection: close`); no chunked-transfer
//! decoding, no TLS — for LAN/control-plane HTTP, the same niche as netserve.
//! Pair with [`crate::json`] to build a JSON body and read a field out of the
//! reply, and [`crate::auth`] for a bearer token.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// A request to build. `body` is sent as-is with a matching `Content-Length`;
/// `Connection: close` is always set.
pub struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub host: &'a str,
    pub content_type: &'a str,
    pub body: String,
}

impl<'a> Request<'a> {
    /// A `POST` of a JSON body to `path` on `host` (`host` is the `Host:` header
    /// and the connect target, "ip:port").
    pub fn post_json(host: &'a str, path: &'a str, body: String) -> Self {
        Self { method: "POST", path, host, content_type: "application/json", body }
    }

    /// Serialize to the raw HTTP/1.1 request bytes.
    pub fn to_wire(&self) -> String {
        format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            self.method,
            self.path,
            self.host,
            self.content_type,
            self.body.len(),
            self.body
        )
    }
}

/// A parsed response: status code + body (headers split off). Enough for a simple
/// client; not a full HTTP parser.
pub struct Response {
    pub status: u16,
    pub body: String,
}

/// Split headers from body and read the status code off the status line
/// ("HTTP/1.1 200 OK" → 200). Status `0` means the status line was unparseable.
pub fn parse_response(raw: &str) -> Response {
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Response { status, body: body.to_string() }
}

/// The IO seam: connect, send the request bytes, read the whole response (the
/// server closes the connection — `Connection: close`). Integration-tested
/// against a real server.
pub fn roundtrip(host: &str, request: &str, timeout: Duration) -> std::io::Result<String> {
    let mut stream = TcpStream::connect(host)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.write_all(request.as_bytes())?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_wire_has_method_path_host_and_correct_content_length() {
        let body = format!("{{\"prompt\":\"{}\"}}", crate::json::escape("hi \"there\"\n"));
        let req = Request::post_json("10.0.0.5:11434", "/api/generate", body);
        let wire = req.to_wire();
        assert!(wire.starts_with("POST /api/generate HTTP/1.1\r\n"));
        assert!(wire.contains("Host: 10.0.0.5:11434\r\n"));
        assert!(wire.contains("Content-Type: application/json\r\n"));
        // body is everything after the blank line; Content-Length must equal its byte length
        let (head, after) = wire.split_once("\r\n\r\n").unwrap();
        let cl: usize = head
            .lines()
            .find_map(|l| l.strip_prefix("Content-Length: "))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(cl, after.len());
        assert!(after.contains("hi \\\"there\\\"\\n"), "body carries the JSON-escaped prompt: {after}");
    }

    #[test]
    fn parse_response_reads_status_and_body() {
        let raw = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"response\":\"ok\"}";
        let r = parse_response(raw);
        assert_eq!(r.status, 200);
        assert_eq!(crate::json::find_string_field(&r.body, "response").as_deref(), Some("ok"));
    }

    #[test]
    fn parse_response_handles_error_status() {
        let r = parse_response("HTTP/1.1 404 Not Found\r\n\r\nnope");
        assert_eq!(r.status, 404);
        assert_eq!(r.body, "nope");
    }

    #[test]
    fn parse_response_unparseable_status_is_zero() {
        let r = parse_response("garbage-no-status\r\n\r\nbody");
        assert_eq!(r.status, 0);
        assert_eq!(r.body, "body");
    }
}
