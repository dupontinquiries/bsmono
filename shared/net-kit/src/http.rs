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

// ── server side: parse an incoming request, build a response, serve a socket ────
//
// Lifted from netserve's HTTP/1.1 server (the LAN control plane). The parser and
// builder are PURE and tested; the accept/read loop is the IO seam. Auth is the
// handler's job via [`crate::auth::Keyring`] and [`ServerRequest::token`].

use std::net::TcpListener;

/// An incoming request, parsed server-side: method, path (query split off),
/// headers, body. The mirror of [`Request`] (client-side).
pub struct ServerRequest {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl ServerRequest {
    /// Parse one request's raw bytes (head + body split on the blank line).
    pub fn parse(raw: &[u8]) -> Option<ServerRequest> {
        let sep = find_subslice(raw, b"\r\n\r\n")?;
        let head = std::str::from_utf8(&raw[..sep]).ok()?;
        let body = raw[sep + 4..].to_vec();
        let mut lines = head.split("\r\n");
        let mut start = lines.next()?.split_whitespace();
        let method = start.next()?.to_string();
        let target = start.next()?;
        let (path, query) = match target.split_once('?') {
            Some((p, q)) => (p.to_string(), q.to_string()),
            None => (target.to_string(), String::new()),
        };
        let headers = lines
            .filter_map(|l| l.split_once(": ").map(|(k, v)| (k.to_string(), v.to_string())))
            .collect();
        Some(ServerRequest { method, path, query, headers, body })
    }

    /// A header value by name (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// A bearer token: `Authorization: Bearer <t>`, or an `X-Api-Key` header.
    pub fn token(&self) -> Option<&str> {
        self.header("authorization")
            .and_then(|a| a.strip_prefix("Bearer "))
            .or_else(|| self.header("x-api-key"))
    }

    /// A `name=value` query parameter.
    pub fn query_param(&self, name: &str) -> Option<&str> {
        self.query.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            (k == name).then_some(v)
        })
    }
}

/// A response to build server-side.
pub struct ServerResponse {
    pub status: u16,
    pub reason: String,
    pub content_type: String,
    pub body: Vec<u8>,
}

impl ServerResponse {
    pub fn text(status: u16, reason: &str, body: impl Into<String>) -> Self {
        Self {
            status,
            reason: reason.to_string(),
            content_type: "text/plain; charset=utf-8".to_string(),
            body: body.into().into_bytes(),
        }
    }
    pub fn json(status: u16, reason: &str, body: impl Into<String>) -> Self {
        Self {
            status,
            reason: reason.to_string(),
            content_type: "application/json".to_string(),
            body: body.into().into_bytes(),
        }
    }
    /// Serialize to the raw HTTP/1.1 response bytes.
    pub fn to_wire(&self) -> Vec<u8> {
        let head = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.status,
            self.reason,
            self.content_type,
            self.body.len()
        );
        let mut out = head.into_bytes();
        out.extend_from_slice(&self.body);
        out
    }
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Read one full request off a stream: headers to the blank line, then the
/// `Content-Length` body. The blocking read seam.
pub fn read_request(stream: &mut impl Read) -> Option<ServerRequest> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(p) = find_subslice(&buf, b"\r\n\r\n") {
            let head = std::str::from_utf8(&buf[..p]).ok()?;
            let clen = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(": ")?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            while buf.len() < p + 4 + clen {
                let n = stream.read(&mut chunk).ok()?;
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            break;
        }
    }
    ServerRequest::parse(&buf)
}

/// Serve forever on `listener`: read each request, call `handler`, write its
/// response. Single-threaded (one connection at a time) — the simplest correct
/// server; thread the handler at the call site for concurrency. IO seam.
pub fn serve(listener: TcpListener, mut handler: impl FnMut(&ServerRequest) -> ServerResponse) {
    for stream in listener.incoming().flatten() {
        let mut stream = stream;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
        if let Some(req) = read_request(&mut stream) {
            let resp = handler(&req);
            let _ = stream.write_all(&resp.to_wire());
        }
        let _ = stream.flush();
    }
}

#[cfg(test)]
mod server_tests {
    use super::*;

    #[test]
    fn parses_method_path_query_headers_body_and_token() {
        let raw = b"POST /generate?max=64 HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer s3cret\r\nContent-Length: 5\r\n\r\nhello";
        let r = ServerRequest::parse(raw).unwrap();
        assert_eq!(r.method, "POST");
        assert_eq!(r.path, "/generate");
        assert_eq!(r.query_param("max"), Some("64"));
        assert_eq!(r.header("host"), Some("x"));
        assert_eq!(r.token(), Some("s3cret"));
        assert_eq!(r.body, b"hello");
    }

    #[test]
    fn response_wire_has_status_line_and_content_length() {
        let wire = ServerResponse::json(404, "Not Found", "{\"e\":1}").to_wire();
        let s = String::from_utf8(wire).unwrap();
        assert!(s.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(s.contains("Content-Type: application/json\r\n"));
        let (head, body) = s.split_once("\r\n\r\n").unwrap();
        let cl: usize = head.lines().find_map(|l| l.strip_prefix("Content-Length: ")).unwrap().parse().unwrap();
        assert_eq!(cl, body.len());
    }

    #[test]
    fn auth_via_keyring_rejects_then_accepts() {
        let keys = crate::auth::Keyring::new(vec!["good".into()]);
        let ok = ServerRequest::parse(b"GET /x HTTP/1.1\r\nX-Api-Key: good\r\n\r\n").unwrap();
        assert!(keys.accepts(ok.token().unwrap_or("").as_bytes()));
        let bad = ServerRequest::parse(b"GET /x HTTP/1.1\r\n\r\n").unwrap();
        assert!(!keys.accepts(bad.token().unwrap_or("").as_bytes()));
    }
}
