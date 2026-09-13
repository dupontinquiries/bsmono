//! depot-httpd — a small, visible HTTP/1.1 server on std threads.
//!
//! Why not hyper/axum: depot's job is to make a fleet buildable with no
//! internet, and the mirror itself has to be buildable from the mirror. Every
//! transitive dependency is one more crate that must be in the offline store
//! before the store exists. A std-only server with one pure-Rust dependency
//! (flate2's miniz_oxide backend, for gzip of text) keeps that bootstrap set
//! small. The cost is HTTP/1.1-only and no TLS; on a private LAN where every
//! artifact carries its own signature (RPM GPG, Cargo.lock cksum, narinfo Sig)
//! that is the right trade — see DECISIONS.md §3. If it stops being right, the
//! [`Handler`] trait is the seam: everything above it is transport-agnostic.
//!
//! What it does:
//!   * a fixed worker pool over `TcpListener::incoming` (visible backpressure:
//!     when every worker is busy, accept blocks on the channel);
//!   * request parsing with a bounded header buffer and a **streaming body**
//!     (`Request::body` is a `Read`, so a 400 MB upload goes to disk, not RAM);
//!   * HTTP/1.1 keep-alive (cargo's sparse index is thousands of tiny GETs);
//!   * static file serving from a root with path-traversal rejection, ETag /
//!     If-None-Match → 304, single byte-`Range` → 206 (dnf and curl resume with
//!     it), HEAD, content types by extension, and gzip for text bodies when the
//!     client accepts it;
//!   * one log line per request to stderr (journald collects it).
//!
//! What it deliberately does not do: chunked request bodies (nothing depot
//! talks to sends them), pipelining, multi-range, TLS.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Upper bound on the request head (request line + headers). 16 KiB is far
/// above anything cargo, dnf, nix or a browser sends; a head that exceeds it is
/// treated as hostile and the connection is dropped with 431.
pub const MAX_HEAD: usize = 16 * 1024;

// ─────────────────────────────────────────────────────────────────────────────
// Request
// ─────────────────────────────────────────────────────────────────────────────

/// One parsed request. `body` is the still-unread body, bounded by
/// `Content-Length`; read it (`read_to_end` for JSON, `io::copy` to a file for
/// an upload) or leave it — the server drains what you leave before the next
/// keep-alive request.
pub struct Request<'a> {
    pub method: String,
    /// Percent-decoded path, always starting with `/`.
    pub path: String,
    /// Raw query string (without the `?`), empty if none.
    pub query: String,
    /// Header names lower-cased at parse time; values trimmed.
    pub headers: HashMap<String, String>,
    pub content_length: u64,
    pub body: &'a mut dyn Read,
    pub peer: SocketAddr,
    pub http11: bool,
}

impl<'a> Request<'a> {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    /// `name=value` from the query string, percent-decoded.
    pub fn query_param(&self, name: &str) -> Option<String> {
        self.query.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (k == name).then(|| percent_decode(v))
        })
    }

    /// A bearer token: `Authorization: Bearer <t>` or `X-Api-Key: <t>`.
    pub fn token(&self) -> Option<&str> {
        self.header("authorization")
            .and_then(|a| a.strip_prefix("Bearer "))
            .or_else(|| self.header("x-api-key"))
            .map(str::trim)
    }

    pub fn accepts_gzip(&self) -> bool {
        self.header("accept-encoding")
            .map(|v| v.split(',').any(|e| e.trim().starts_with("gzip")))
            .unwrap_or(false)
    }

    /// Read the whole body into memory, refusing bodies above `max`.
    pub fn read_body(&mut self, max: u64) -> io::Result<Vec<u8>> {
        if self.content_length > max {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "body too large"));
        }
        let mut v = Vec::with_capacity(self.content_length as usize);
        self.body.read_to_end(&mut v)?;
        Ok(v)
    }
}

/// Decode `%XX` escapes and `+` is NOT treated as space (paths, not forms).
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hi = (b[i + 1] as char).to_digit(16);
            let lo = (b[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ─────────────────────────────────────────────────────────────────────────────
// Response
// ─────────────────────────────────────────────────────────────────────────────

pub enum Body {
    Empty,
    Bytes(Vec<u8>),
    /// Stream `len` bytes from `file` starting at its current position.
    File { file: File, len: u64 },
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Body,
}

impl Response {
    pub fn new(status: u16) -> Self {
        Response { status, headers: Vec::new(), body: Body::Empty }
    }
    pub fn with_header(mut self, k: &str, v: impl Into<String>) -> Self {
        self.headers.push((k.to_string(), v.into()));
        self
    }
    pub fn bytes(status: u16, content_type: &str, body: Vec<u8>) -> Self {
        Response::new(status).with_header("Content-Type", content_type).with_body(Body::Bytes(body))
    }
    pub fn text(status: u16, body: impl Into<String>) -> Self {
        Self::bytes(status, "text/plain; charset=utf-8", body.into().into_bytes())
    }
    pub fn html(body: impl Into<String>) -> Self {
        Self::bytes(200, "text/html; charset=utf-8", body.into().into_bytes())
    }
    pub fn json(status: u16, body: impl Into<String>) -> Self {
        Self::bytes(status, "application/json", body.into().into_bytes())
    }
    pub fn not_found() -> Self {
        Self::text(404, "not found\n")
    }
    pub fn with_body(mut self, body: Body) -> Self {
        self.body = body;
        self
    }
    fn body_len(&self) -> u64 {
        match &self.body {
            Body::Empty => 0,
            Body::Bytes(b) => b.len() as u64,
            Body::File { len, .. } => *len,
        }
    }
}

pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Payload Too Large",
        416 => "Range Not Satisfiable",
        422 => "Unprocessable Entity",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler seam
// ─────────────────────────────────────────────────────────────────────────────

/// The seam between transport and application. Everything depot knows about
/// crates, RPMs and packs lives above this; everything about sockets lives
/// below it. A handler is shared across worker threads, so it holds `Arc`s or
/// `Mutex`es, never `&mut self`.
pub trait Handler: Send + Sync + 'static {
    fn handle(&self, req: &mut Request<'_>) -> Response;
}

impl<F> Handler for F
where
    F: Fn(&mut Request<'_>) -> Response + Send + Sync + 'static,
{
    fn handle(&self, req: &mut Request<'_>) -> Response {
        self(req)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Server
// ─────────────────────────────────────────────────────────────────────────────

pub struct Server {
    listener: TcpListener,
    handler: Arc<dyn Handler>,
    workers: usize,
    queue: usize,
    log: bool,
    read_timeout: Duration,
    keepalive_timeout: Duration,
}

impl Server {
    pub fn bind(addr: &str, handler: impl Handler) -> io::Result<Server> {
        let listener = TcpListener::bind(addr)?;
        Ok(Server {
            listener,
            handler: Arc::new(handler),
            workers: 32,
            queue: 256,
            log: true,
            read_timeout: Duration::from_secs(30),
            keepalive_timeout: Duration::from_secs(15),
        })
    }
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }
    pub fn workers(mut self, n: usize) -> Self {
        self.workers = n.max(1);
        self
    }
    pub fn log(mut self, on: bool) -> Self {
        self.log = on;
        self
    }

    /// Serve until `stop` becomes true (checked every 100 ms) — the accept loop
    /// is the whole scheduler: accepted sockets go onto a bounded channel that
    /// `workers` threads drain. When the channel is full, `send` blocks and the
    /// kernel's listen backlog absorbs the rest; that is the backpressure.
    pub fn serve(self, stop: Arc<AtomicBool>) -> io::Result<()> {
        let (tx, rx) = mpsc::sync_channel::<TcpStream>(self.queue);
        let rx = Arc::new(Mutex::new(rx));
        let mut threads = Vec::with_capacity(self.workers);
        for i in 0..self.workers {
            let rx = Arc::clone(&rx);
            let handler = Arc::clone(&self.handler);
            let (log, rt, ka) = (self.log, self.read_timeout, self.keepalive_timeout);
            threads.push(
                std::thread::Builder::new()
                    .name(format!("httpd-{i}"))
                    .spawn(move || loop {
                        let stream = match rx.lock().unwrap().recv() {
                            Ok(s) => s,
                            Err(_) => return, // sender dropped: shutting down
                        };
                        handle_connection(stream, &*handler, log, rt, ka);
                    })?,
            );
        }
        self.listener.set_nonblocking(true)?;
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match self.listener.accept() {
                Ok((stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    if tx.send(stream).is_err() {
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => {
                    eprintln!("httpd: accept: {e}");
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
        drop(tx);
        for t in threads {
            let _ = t.join();
        }
        Ok(())
    }
}

/// A body reader bounded to `Content-Length`; `read_body`/`io::copy` on the
/// [`Request`] go through this.
struct Bounded<'a> {
    inner: &'a mut BufReader<TcpStream>,
    left: u64,
}
impl Read for Bounded<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(self.left) as usize;
        let n = self.inner.read(&mut buf[..want])?;
        self.left -= n as u64;
        Ok(n)
    }
}

fn handle_connection(
    stream: TcpStream,
    handler: &dyn Handler,
    log: bool,
    read_timeout: Duration,
    keepalive: Duration,
) {
    let peer = stream.peer_addr().unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap());
    let _ = stream.set_nodelay(true);
    let mut writer = match stream.try_clone() {
        Ok(w) => w,
        Err(_) => return,
    };
    let mut reader = BufReader::with_capacity(64 * 1024, stream);
    let mut first = true;
    loop {
        // The first request waits `read_timeout`; subsequent keep-alive
        // requests wait only `keepalive` before we hang up on an idle socket.
        let _ = reader.get_ref().set_read_timeout(Some(if first { read_timeout } else { keepalive }));
        first = false;
        let started = Instant::now();
        let head = match read_head(&mut reader) {
            Ok(Some(h)) => h,
            Ok(None) => return, // clean EOF between requests
            Err(HeadError::TooLarge) => {
                let _ = write_response(&mut writer, Response::text(431, "request head too large\n"), false, false);
                return;
            }
            Err(HeadError::Io(e)) => {
                // Idle keep-alive sockets time out here constantly; only
                // genuinely odd errors are worth a line.
                if !matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::ConnectionReset) && log {
                    eprintln!("{peer} read: {e}");
                }
                return;
            }
            Err(HeadError::Malformed) => return,
        };
        let (method, target, version, headers) = match parse_head(&head) {
            Some(x) => x,
            None => {
                let _ = write_response(&mut writer, Response::text(400, "malformed request\n"), true, false);
                return;
            }
        };
        let http11 = version == "HTTP/1.1";
        let content_length = headers.get("content-length").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        if headers.get("transfer-encoding").map(|v| v.contains("chunked")).unwrap_or(false) {
            let _ = write_response(&mut writer, Response::text(411, "chunked request bodies are not supported; send Content-Length\n"), true, false);
            return;
        }
        let (path, query) = match target.split_once('?') {
            Some((p, q)) => (percent_decode(p), q.to_string()),
            None => (percent_decode(&target), String::new()),
        };
        let client_wants_close = headers.get("connection").map(|v| v.eq_ignore_ascii_case("close")).unwrap_or(!http11);

        let mut bounded = Bounded { inner: &mut reader, left: content_length };
        let is_head = method == "HEAD";
        let mut req = Request {
            method: method.clone(),
            path: path.clone(),
            query,
            headers,
            content_length,
            body: &mut bounded,
            peer,
            http11,
        };
        let resp = handler.handle(&mut req);
        // Drain whatever the handler left so the next request parses cleanly.
        // If it left a lot (an upload it rejected early), closing is cheaper
        // than reading it all; 64 KiB is the threshold.
        let leftover = bounded.left;
        let drained_ok = if leftover > 0 && leftover <= 64 * 1024 {
            io::copy(&mut bounded, &mut io::sink()).is_ok()
        } else {
            leftover == 0
        };
        let keep = http11 && !client_wants_close && drained_ok;
        let status = resp.status;
        let len = resp.body_len();
        let ok = write_response(&mut writer, resp, is_head, keep).is_ok();
        if log {
            eprintln!(
                "{peer} {method} {path} {status} {len} {}ms",
                started.elapsed().as_millis()
            );
        }
        if !ok || !keep {
            return;
        }
    }
}

enum HeadError {
    TooLarge,
    Malformed,
    Io(io::Error),
}

/// Read up to the blank line. Returns Ok(None) on a clean EOF before any byte.
fn read_head(r: &mut BufReader<TcpStream>) -> Result<Option<Vec<u8>>, HeadError> {
    let mut head = Vec::with_capacity(512);
    loop {
        let buf = r.fill_buf().map_err(HeadError::Io)?;
        if buf.is_empty() {
            return if head.is_empty() { Ok(None) } else { Err(HeadError::Malformed) };
        }
        // Look for \r\n\r\n across the boundary of what we already have.
        let take = buf.len().min(MAX_HEAD - head.len().min(MAX_HEAD));
        head.extend_from_slice(&buf[..take]);
        if let Some(pos) = find(&head, b"\r\n\r\n") {
            let consumed = take - (head.len() - (pos + 4));
            r.consume(consumed);
            head.truncate(pos + 4);
            return Ok(Some(head));
        }
        r.consume(take);
        if head.len() >= MAX_HEAD {
            return Err(HeadError::TooLarge);
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn parse_head(head: &[u8]) -> Option<(String, String, String, HashMap<String, String>)> {
    let text = std::str::from_utf8(head).ok()?;
    let mut lines = text.split("\r\n");
    let mut rl = lines.next()?.split(' ');
    let method = rl.next()?.to_string();
    let target = rl.next()?.to_string();
    let version = rl.next().unwrap_or("HTTP/1.0").to_string();
    if method.is_empty() || !target.starts_with('/') {
        return None;
    }
    let mut headers = HashMap::new();
    for l in lines {
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    Some((method, target, version, headers))
}

fn write_response(w: &mut TcpStream, resp: Response, is_head: bool, keep: bool) -> io::Result<()> {
    let len = resp.body_len();
    let mut head = format!("HTTP/1.1 {} {}\r\n", resp.status, reason(resp.status));
    let mut has_ct = false;
    for (k, v) in &resp.headers {
        if k.eq_ignore_ascii_case("content-type") {
            has_ct = true;
        }
        head.push_str(k);
        head.push_str(": ");
        head.push_str(v);
        head.push_str("\r\n");
    }
    if !has_ct && len > 0 {
        head.push_str("Content-Type: application/octet-stream\r\n");
    }
    head.push_str(&format!("Content-Length: {len}\r\n"));
    head.push_str(if keep { "Connection: keep-alive\r\n" } else { "Connection: close\r\n" });
    head.push_str("Server: depot\r\n\r\n");
    w.write_all(head.as_bytes())?;
    if !is_head {
        match resp.body {
            Body::Empty => {}
            Body::Bytes(b) => w.write_all(&b)?,
            Body::File { mut file, len } => {
                let mut limited = (&mut file).take(len);
                io::copy(&mut limited, w)?;
            }
        }
    }
    w.flush()
}

// ─────────────────────────────────────────────────────────────────────────────
// Static files
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve a URL path against a root, refusing anything that could escape it.
/// Returns None for `..`, absolute components, NUL, or a path that
/// canonicalizes outside `root`. Symlinks inside the root are followed only if
/// their target is also inside the root.
pub fn safe_join(root: &Path, url_path: &str) -> Option<PathBuf> {
    if url_path.contains('\0') {
        return None;
    }
    let mut p = root.to_path_buf();
    for c in Path::new(url_path.trim_start_matches('/')).components() {
        match c {
            Component::Normal(seg) => p.push(seg),
            Component::CurDir => {}
            _ => return None, // ParentDir, RootDir, Prefix
        }
    }
    // Belt and braces: after joining, the canonical path must still be under
    // the canonical root (catches symlink escapes).
    let root_c = root.canonicalize().ok()?;
    match p.canonicalize() {
        Ok(c) if c.starts_with(&root_c) => Some(c),
        Ok(_) => None,
        Err(_) => {
            // Does not exist (yet). Still refuse if the nearest existing
            // ancestor resolves outside the root — a symlinked directory plus
            // a fresh filename must not become a write outside the store.
            let mut anc = p.parent();
            while let Some(a) = anc {
                if let Ok(c) = a.canonicalize() {
                    return c.starts_with(&root_c).then_some(p);
                }
                anc = a.parent();
            }
            None
        }
    }
}

pub fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "json" => "application/json",
        "txt" | "md" | "sh" | "repo" | "narinfo" | "toml" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        "gz" | "crate" => "application/gzip",
        "xz" => "application/x-xz",
        "zst" => "application/zstd",
        "rpm" => "application/x-rpm",
        "nar" => "application/x-nix-nar",
        "zip" => "application/zip",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        _ => "application/octet-stream",
    }
}

fn is_text(ct: &str) -> bool {
    ct.starts_with("text/") || ct == "application/json" || ct == "application/xml" || ct.starts_with("application/javascript")
}

/// Serve a regular file with ETag/304, Range/206, HEAD and gzip-for-text.
/// `etag` may be supplied by the caller (e.g. a content hash the store already
/// knows); otherwise a weak mtime+len tag is used.
pub fn serve_file(path: &Path, req: &Request<'_>, etag: Option<String>) -> Response {
    let meta = match std::fs::metadata(path) {
        Ok(m) if m.is_file() => m,
        _ => return Response::not_found(),
    };
    let len = meta.len();
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    let etag = etag.unwrap_or_else(|| format!("\"{mtime:x}-{len:x}\""));
    if let Some(inm) = req.header("if-none-match") {
        if inm.split(',').any(|t| t.trim() == etag || t.trim() == "*") {
            return Response::new(304).with_header("ETag", etag.clone());
        }
    }
    let ct = content_type(path);
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return Response::not_found(),
    };
    let base = Response::new(200)
        .with_header("Content-Type", ct)
        .with_header("ETag", etag)
        .with_header("Last-Modified", http_date(mtime))
        .with_header("Accept-Ranges", "bytes")
        .with_header("Cache-Control", if is_text(ct) { "no-cache" } else { "public, max-age=31536000, immutable" });

    if let Some(range) = req.header("range") {
        return match parse_range(range, len) {
            Some((start, end)) => {
                if file.seek(SeekFrom::Start(start)).is_err() {
                    return Response::text(500, "seek failed\n");
                }
                let mut r = base;
                r.status = 206;
                r.with_header("Content-Range", format!("bytes {start}-{end}/{len}")).with_body(Body::File { file, len: end - start + 1 })
            }
            None => Response::new(416).with_header("Content-Range", format!("bytes */{len}")),
        };
    }

    // gzip small text bodies for browsers/cargo; never gzip artifacts, whose
    // bytes must reach the client exactly (their checksums are the security).
    if is_text(ct) && req.accepts_gzip() && len <= 8 * 1024 * 1024 && len > 256 {
        let mut raw = Vec::with_capacity(len as usize);
        if file.read_to_end(&mut raw).is_ok() {
            let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            if enc.write_all(&raw).is_ok() {
                if let Ok(gz) = enc.finish() {
                    return base.with_header("Content-Encoding", "gzip").with_header("Vary", "Accept-Encoding").with_body(Body::Bytes(gz));
                }
            }
        }
        return Response::text(500, "gzip failed\n");
    }
    base.with_body(Body::File { file, len })
}

/// Parse `bytes=a-b` / `bytes=a-` / `bytes=-n` into an inclusive (start, end),
/// or None if unsatisfiable. Multi-range is treated as unsatisfiable on purpose.
pub fn parse_range(h: &str, len: u64) -> Option<(u64, u64)> {
    let spec = h.strip_prefix("bytes=")?;
    if spec.contains(',') || len == 0 {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        if n == 0 {
            return None;
        }
        (len.saturating_sub(n), len - 1)
    } else {
        let s: u64 = a.parse().ok()?;
        let e = if b.is_empty() { len - 1 } else { b.parse::<u64>().ok()?.min(len - 1) };
        (s, e)
    };
    (start <= end && start < len).then_some((start, end))
}

pub fn http_date(secs: u64) -> String {
    // RFC 7231 IMF-fixdate without pulling in a time crate.
    let days = secs / 86400;
    let rem = secs % 86400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days as i64);
    let wd = ((days + 4) % 7) as usize; // 1970-01-01 was a Thursday
    const WD: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MO: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{}, {:02} {} {} {:02}:{:02}:{:02} GMT", WD[wd], d, MO[(mo - 1) as usize], y, h, m, s)
}

// Howard Hinnant's days→civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Minimal HTML escaping for the directory listing and error pages.
pub fn html_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            _ => o.push(c),
        }
    }
    o
}

/// A plain directory listing (what a Fedora mirror shows you). Browsable, not
/// pretty; dnf and cargo never ask for it.
pub fn dir_listing(url_path: &str, dir: &Path) -> Response {
    let mut names: Vec<(String, bool)> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .map(|e| {
                let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                (e.file_name().to_string_lossy().into_owned(), is_dir)
            })
            .collect(),
        Err(_) => return Response::not_found(),
    };
    names.sort();
    let mut html = format!("<!doctype html><meta charset=utf-8><title>{0}</title><h1>{0}</h1><ul>", html_escape(url_path));
    if url_path != "/" {
        html.push_str("<li><a href=\"../\">../</a></li>");
    }
    for (n, is_dir) in names {
        let suffix = if is_dir { "/" } else { "" };
        html.push_str(&format!("<li><a href=\"{0}{1}\">{0}{1}</a></li>", html_escape(&n), suffix));
    }
    html.push_str("</ul>");
    Response::html(html)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpStream;

    fn start(handler: impl Handler) -> (SocketAddr, Arc<AtomicBool>, std::thread::JoinHandle<()>) {
        let srv = Server::bind("127.0.0.1:0", handler).unwrap().workers(4).log(false);
        let addr = srv.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = Arc::clone(&stop);
        let jh = std::thread::spawn(move || srv.serve(s2).unwrap());
        (addr, stop, jh)
    }

    fn raw(addr: SocketAddr, req: &str) -> (u16, HashMap<String, String>, Vec<u8>) {
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(req.as_bytes()).unwrap();
        read_one(&mut BufReader::new(s))
    }

    fn read_one(r: &mut BufReader<TcpStream>) -> (u16, HashMap<String, String>, Vec<u8>) {
        read_one_opt(r, false)
    }

    fn read_one_opt(r: &mut BufReader<TcpStream>, head_only: bool) -> (u16, HashMap<String, String>, Vec<u8>) {
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        let status: u16 = line.split(' ').nth(1).unwrap().parse().unwrap();
        let mut headers = HashMap::new();
        loop {
            let mut l = String::new();
            r.read_line(&mut l).unwrap();
            let l = l.trim_end().to_string();
            if l.is_empty() {
                break;
            }
            let (k, v) = l.split_once(": ").unwrap();
            headers.insert(k.to_ascii_lowercase(), v.to_string());
        }
        let len: usize = headers.get("content-length").map(|v| v.parse().unwrap()).unwrap_or(0);
        if head_only {
            return (status, headers, Vec::new());
        }
        let mut body = vec![0u8; len];
        r.read_exact(&mut body).unwrap();
        (status, headers, body)
    }

    #[test]
    fn echo_handler_roundtrip_and_query_decoding() {
        let (addr, stop, jh) = start(|req: &mut Request<'_>| {
            let body = req.read_body(1 << 20).unwrap();
            Response::text(200, format!("{} {} q={:?} body={}", req.method, req.path, req.query_param("x"), String::from_utf8_lossy(&body)))
        });
        let (st, _, body) = raw(addr, "POST /a%20b?x=1%2B1 HTTP/1.1\r\nHost: t\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello");
        assert_eq!(st, 200);
        assert_eq!(String::from_utf8(body).unwrap(), "POST /a b q=Some(\"1+1\") body=hello");
        stop.store(true, Ordering::Relaxed);
        jh.join().unwrap();
    }

    #[test]
    fn keepalive_serves_two_requests_on_one_socket() {
        let (addr, stop, jh) = start(|req: &mut Request<'_>| Response::text(200, req.path.clone()));
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(b"GET /one HTTP/1.1\r\nHost: t\r\n\r\nGET /two HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n").unwrap();
        let mut r = BufReader::new(s);
        let (_, h1, b1) = read_one(&mut r);
        assert_eq!(b1, b"/one");
        assert_eq!(h1.get("connection").unwrap(), "keep-alive");
        let (_, h2, b2) = read_one(&mut r);
        assert_eq!(b2, b"/two");
        assert_eq!(h2.get("connection").unwrap(), "close");
        stop.store(true, Ordering::Relaxed);
        jh.join().unwrap();
    }

    #[test]
    fn oversized_head_gets_431() {
        let (addr, stop, jh) = start(|_: &mut Request<'_>| Response::text(200, "x"));
        let big = format!("GET / HTTP/1.1\r\nX-Junk: {}\r\n\r\n", "a".repeat(MAX_HEAD + 10));
        let (st, _, _) = raw(addr, &big);
        assert_eq!(st, 431);
        stop.store(true, Ordering::Relaxed);
        jh.join().unwrap();
    }

    #[test]
    fn safe_join_rejects_traversal_and_accepts_normal() {
        let tmp = std::env::temp_dir().join(format!("depot-httpd-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("a")).unwrap();
        std::fs::write(tmp.join("a/f.txt"), b"hi").unwrap();
        assert!(safe_join(&tmp, "/../etc/passwd").is_none());
        assert!(safe_join(&tmp, "/a/../../x").is_none());
        assert!(safe_join(&tmp, "/a/\0").is_none());
        assert!(safe_join(&tmp, "/a/f.txt").unwrap().ends_with("a/f.txt"));
        assert!(safe_join(&tmp, "/a/./f.txt").unwrap().ends_with("a/f.txt"));
        // An escaping symlink is refused even though every component is Normal.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc", tmp.join("a/esc")).unwrap();
            assert!(safe_join(&tmp, "/a/esc/passwd").is_none());
            assert!(safe_join(&tmp, "/a/esc/does-not-exist-yet").is_none(), "escape via symlinked dir + new name");
            assert!(safe_join(&tmp, "/a/new-file").is_some(), "a new name under the real root is fine");
        }
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn range_parsing_covers_all_three_forms_and_rejects_bad() {
        assert_eq!(parse_range("bytes=0-9", 100), Some((0, 9)));
        assert_eq!(parse_range("bytes=90-", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=50-1000", 100), Some((50, 99)));
        assert_eq!(parse_range("bytes=100-", 100), None);
        assert_eq!(parse_range("bytes=5-2", 100), None);
        assert_eq!(parse_range("bytes=0-1,5-6", 100), None);
        assert_eq!(parse_range("bytes=0-", 0), None);
    }

    #[test]
    fn serve_file_does_etag_304_range_206_and_gzip_for_text() {
        let tmp = std::env::temp_dir().join(format!("depot-httpd-sf-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(tmp.join("blob.rpm"), &data).unwrap();
        std::fs::write(tmp.join("t.txt"), "hello world ".repeat(200)).unwrap();
        let root = tmp.clone();
        let (addr, stop, jh) = start(move |req: &mut Request<'_>| {
            let p = safe_join(&root, &req.path).unwrap();
            serve_file(&p, req, None)
        });
        let (st, h, body) = raw(addr, "GET /blob.rpm HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
        assert_eq!(st, 200);
        assert_eq!(body, data);
        let etag = h.get("etag").unwrap().clone();
        assert!(h.get("cache-control").unwrap().contains("immutable"));
        let (st, _, body) = raw(addr, &format!("GET /blob.rpm HTTP/1.1\r\nHost: t\r\nIf-None-Match: {etag}\r\nConnection: close\r\n\r\n"));
        assert_eq!(st, 304);
        assert!(body.is_empty());
        let (st, h, body) = raw(addr, "GET /blob.rpm HTTP/1.1\r\nHost: t\r\nRange: bytes=10-19\r\nConnection: close\r\n\r\n");
        assert_eq!(st, 206);
        assert_eq!(h.get("content-range").unwrap(), "bytes 10-19/100000");
        assert_eq!(body, &data[10..20]);
        let (st, h, _) = raw(addr, "GET /blob.rpm HTTP/1.1\r\nHost: t\r\nRange: bytes=999999-\r\nConnection: close\r\n\r\n");
        assert_eq!(st, 416);
        assert_eq!(h.get("content-range").unwrap(), "bytes */100000");
        let mut hs = TcpStream::connect(addr).unwrap();
        hs.write_all(b"HEAD /blob.rpm HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n").unwrap();
        let mut hr = BufReader::new(hs);
        let (st, h, _) = read_one_opt(&mut hr, true);
        assert_eq!(st, 200);
        assert_eq!(h.get("content-length").unwrap(), "100000");
        let mut rest = Vec::new();
        hr.read_to_end(&mut rest).unwrap();
        assert!(rest.is_empty(), "HEAD must send no body");
        // gzip only when asked, only for text
        let (st, h, body) = raw(addr, "GET /t.txt HTTP/1.1\r\nHost: t\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n");
        assert_eq!(st, 200);
        assert_eq!(h.get("content-encoding").unwrap(), "gzip");
        let mut dec = flate2::read::GzDecoder::new(&body[..]);
        let mut out = String::new();
        dec.read_to_string(&mut out).unwrap();
        assert_eq!(out, "hello world ".repeat(200));
        let (_, h, _) = raw(addr, "GET /blob.rpm HTTP/1.1\r\nHost: t\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n");
        assert!(h.get("content-encoding").is_none(), "artifacts are never recompressed");
        let (st, _, _) = raw(addr, "GET /nope HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
        assert_eq!(st, 404);
        stop.store(true, Ordering::Relaxed);
        jh.join().unwrap();
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn http_date_matches_known_values() {
        assert_eq!(http_date(0), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(http_date(1_000_000_000), "Sun, 09 Sep 2001 01:46:40 GMT");
        assert_eq!(http_date(1_725_890_250), "Mon, 09 Sep 2024 13:57:30 GMT");
    }

    #[test]
    fn percent_decode_edges() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("a+b"), "a+b");
    }
}
