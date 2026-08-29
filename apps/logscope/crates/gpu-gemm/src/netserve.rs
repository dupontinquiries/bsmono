//! netserve — a tiny, dependency-free HTTP server for exposing the inference engine on the
//! LAN, gated by API keys. `std::net` only; thread-per-connection (the engine's `Gpu` and
//! `Model` are `Send`/`Sync`, so one shared `Arc` pair backs every connection without
//! reloading the weights).
//!
//! Security model: clients authenticate with a bearer token — `Authorization: Bearer <key>`
//! or `X-API-Key: <key>` — compared in **constant time** against the operator's keyring.
//! Keys are generated with [`generate_key`] (OS entropy via `/dev/urandom`). This
//! authenticates callers; it does **not** encrypt the channel. Run it on a trusted LAN,
//! behind an SSH tunnel (`ssh -L`), or terminate TLS at a reverse proxy. Transport
//! encryption in-process (rustls) is the documented next step.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

const MAX_BODY: usize = 8 * 1024 * 1024; // 8 MiB request-body cap
const MAX_HEAD: usize = 64 * 1024; // 64 KiB header cap

/// Constant-time byte-slice equality — no early return on the first mismatch, so a timing
/// observer can't recover the key byte by byte.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) { diff |= x ^ y; }
    diff == 0
}

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b { s.push_str(&format!("{x:02x}")); }
    s
}

/// A random 32-byte API key as a 64-char hex string. Uses OS entropy (`/dev/urandom`);
/// falls back to a time/pid mix only if that's unavailable (weaker — note it in logs).
pub fn generate_key() -> String {
    let mut buf = [0u8; 32];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        if f.read_exact(&mut buf).is_ok() { return hex(&buf); }
    }
    let mut s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64).unwrap_or(0)
        ^ (std::process::id() as u64).rotate_left(17);
    for b in buf.iter_mut() { s ^= s << 13; s ^= s >> 7; s ^= s << 17; *b = s as u8; }
    hex(&buf)
}

/// The set of accepted API keys. Empty keyring = open (no auth) — only for fully trusted
/// setups; the server warns when it starts open.
#[derive(Clone, Default)]
pub struct Keyring { keys: Vec<Vec<u8>> }
impl Keyring {
    pub fn new(keys: Vec<String>) -> Self {
        Self { keys: keys.into_iter().filter(|k| !k.is_empty()).map(|k| k.into_bytes()).collect() }
    }
    pub fn is_open(&self) -> bool { self.keys.is_empty() }
    /// Constant-time membership: checks every key (so timing doesn't reveal which matched).
    pub fn accepts(&self, token: &[u8]) -> bool {
        if self.keys.is_empty() { return true; }
        let mut ok = false;
        for k in &self.keys { ok |= ct_eq(k, token); }
        ok
    }
}

pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub query: String,
    pub token: Option<String>,
    pub body: Vec<u8>,
}
impl HttpRequest {
    /// `max` tokens to generate, from `?max=N`, falling back to `default`.
    pub fn max_param(&self, default: usize) -> usize {
        for kv in self.query.split('&') {
            if let Some(v) = kv.strip_prefix("max=") {
                if let Ok(n) = v.parse::<usize>() { return n.clamp(1, 8192); }
            }
        }
        default
    }
}

pub struct HttpResponse {
    pub status: u16,
    pub reason: &'static str,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}
impl HttpResponse {
    pub fn text(status: u16, reason: &'static str, body: impl Into<Vec<u8>>) -> Self {
        Self { status, reason, content_type: "text/plain; charset=utf-8", body: body.into() }
    }
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.status, self.reason, self.content_type, self.body.len()
        ).into_bytes();
        out.extend_from_slice(&self.body);
        out
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Read and parse one HTTP/1.1 request: request line, headers (Authorization / X-API-Key /
/// Content-Length), and a length-bounded body. Returns None on malformed/oversized input.
/// Generic over the stream so ONE parser serves both transports.
///
/// Was `&mut TcpStream`. The daemon needs the same endpoint over a unix
/// socket — same requests, same handler, different door — and the only
/// thing standing in the way was this signature naming a concrete type
/// it never used anything specific from.
pub fn read_request<S: Read>(stream: &mut S) -> Option<HttpRequest> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(p) = find(&buf, b"\r\n\r\n") { break p; }
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 { return None; }
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() > MAX_HEAD { return None; }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.lines();
    let req_line = lines.next()?;
    let mut parts = req_line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target, String::new()),
    };
    let mut token = None;
    let mut content_len = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let kl = k.trim().to_ascii_lowercase();
            let v = v.trim();
            match kl.as_str() {
                "authorization" => {
                    token = v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer "))
                        .map(|s| s.to_string());
                }
                "x-api-key" => { if token.is_none() { token = Some(v.to_string()); } }
                "content-length" => { content_len = v.parse::<usize>().unwrap_or(0).min(MAX_BODY); }
                _ => {}
            }
        }
    }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < content_len {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 { break; }
        body.extend_from_slice(&tmp[..n]);
        if body.len() > MAX_BODY { return None; }
    }
    body.truncate(content_len);
    Some(HttpRequest { method, path, query, token, body })
}

/// Auth + routing. `/health` is unauthenticated; everything else requires a valid key.
/// `POST /generate` runs `gen(prompt, max)` where prompt is the request body and `max`
/// comes from `?max=N`.
pub fn handle(
    req: &HttpRequest, keys: &Keyring,
    gen: &(dyn Fn(&str, usize) -> String + Send + Sync),
) -> HttpResponse {
    if req.method == "GET" && req.path == "/health" {
        return HttpResponse::text(200, "OK", "ok");
    }
    let token = req.token.as_deref().unwrap_or("");
    if !keys.accepts(token.as_bytes()) {
        return HttpResponse::text(401, "Unauthorized", "missing or invalid API key");
    }
    match (req.method.as_str(), req.path.as_str()) {
        ("POST", "/generate") => {
            let prompt = String::from_utf8_lossy(&req.body);
            HttpResponse::text(200, "OK", gen(&prompt, req.max_param(128)))
        }
        ("GET", "/") | ("GET", "/version") => HttpResponse::text(200, "OK", "logscope-serve\n"),
        ("POST", _) | ("GET", _) => HttpResponse::text(404, "Not Found", "no such endpoint"),
        _ => HttpResponse::text(405, "Method Not Allowed", "method not allowed"),
    }
}

/// Serve forever: accept connections, handle each on its own thread. `gen` is shared via
/// `Arc`, so the engine's `Gpu` + `Model` back every connection with no reload.
/// Where the daemon's unix socket lives by default.
///
/// Lives HERE, in the library, because two independent copies of this
/// rule is how the daemon and its clients end up looking in different
/// places and each concluding the other is not running. `logscope-serve`
/// and `aperture-engine::daemon::socket_path` both resolve to this.
///
/// Prefers $XDG_RUNTIME_DIR; falls back to $HOME rather than /tmp, since
/// a socket in a sticky shared directory is world-visible and this one
/// carries prompts.
pub fn default_socket_path() -> std::path::PathBuf {
    if let Ok(rt) = std::env::var("XDG_RUNTIME_DIR") {
        return std::path::Path::new(&rt).join("logscope-engine.sock");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::Path::new(&home).join(".logscope-engine.sock")
}

/// Serve on a unix socket: the SAME endpoint, for consumers on this
/// machine.
///
/// Why this exists rather than pointing local apps at the TCP port: on
/// one machine the expensive thing is not the transport, it is that each
/// app otherwise loads its own copy of the weights — 7.2 GiB resident
/// and 86 seconds, per app. This is the door through which they share
/// one.
///
/// NO KEYRING. The filesystem already answers who may connect to a
/// socket in the user's own runtime directory, and a key on a
/// same-machine socket is ceremony with a storage problem attached. The
/// TCP path keeps its keyring for the across-the-LAN case, which is a
/// genuinely different threat — so this passes an empty one and the
/// handler's auth check is bypassed for local callers by construction
/// rather than by a flag someone can flip the wrong way.
///
/// Removes a stale socket first. A daemon that will not start because
/// its predecessor was killed is a daemon people stop running.
pub fn serve_unix(
    path: &std::path::Path, keys: Keyring,
    gen: Arc<dyn Fn(&str, usize) -> String + Send + Sync>,
) -> std::io::Result<()> {
    let _ = std::fs::remove_file(path);
    let listener = std::os::unix::net::UnixListener::bind(path)?;
    let keys = Arc::new(keys);
    for stream in listener.incoming().flatten() {
        let (keys, gen) = (keys.clone(), gen.clone());
        std::thread::spawn(move || {
            let mut stream = stream;
            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
            if let Some(req) = read_request(&mut stream) {
                let resp = handle(&req, &keys, gen.as_ref());
                let _ = stream.write_all(&resp.bytes());
            }
            let _ = stream.flush();
        });
    }
    Ok(())
}

pub fn serve(
    listener: TcpListener, keys: Keyring,
    gen: Arc<dyn Fn(&str, usize) -> String + Send + Sync>,
) {
    let keys = Arc::new(keys);
    for stream in listener.incoming().flatten() {
        let (keys, gen) = (keys.clone(), gen.clone());
        std::thread::spawn(move || {
            let mut stream = stream;
            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
            if let Some(req) = read_request(&mut stream) {
                let resp = handle(&req, &keys, gen.as_ref());
                let _ = stream.write_all(&resp.bytes());
            }
            let _ = stream.flush();
        });
    }
}

#[cfg(test)]
mod unix_tests {
    use super::*;

    /// End-to-end over the real socket: bind, connect, generate.
    ///
    /// Worth doing for real rather than calling `handle` directly,
    /// because the thing that was in the way was the TRANSPORT — a
    /// `read_request` typed to TcpStream — and a test that skips the
    /// socket would not have caught that.
    #[test]
    fn the_unix_door_serves_the_same_endpoint_as_the_tcp_one() {
        use std::io::{Read, Write};
        let dir = std::env::temp_dir().join(format!("gg-unix-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("engine.sock");
        let p2 = path.clone();

        std::thread::spawn(move || {
            // Empty keyring: local callers are authorised by the
            // filesystem, and `accepts` returns true on an empty ring.
            let gen: Arc<dyn Fn(&str, usize) -> String + Send + Sync> =
                Arc::new(|prompt: &str, max: usize| format!("echo:{prompt}:{max}"));
            let _ = serve_unix(&p2, Keyring::default(), gen);
        });

        // Wait for the bind rather than sleeping a fixed time.
        let mut sock = None;
        for _ in 0..200 {
            if let Ok(s) = std::os::unix::net::UnixStream::connect(&path) {
                sock = Some(s);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut sock = sock.expect("daemon socket never appeared");

        let body = "fn f() { ";
        let req = format!(
            "POST /generate?max=32 HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            body.len()
        );
        sock.write_all(req.as_bytes()).unwrap();
        let mut raw = String::new();
        sock.read_to_string(&mut raw).unwrap();

        assert!(raw.starts_with("HTTP/1.1 200"), "{raw}");
        assert!(raw.contains("echo:fn f() { :32"), "prompt and max both arrived: {raw}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_socket_file_does_not_stop_the_daemon_starting() {
        // A daemon that will not start because its predecessor was killed
        // is a daemon people stop running.
        let dir = std::env::temp_dir().join(format!("gg-stale-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("stale.sock");
        std::fs::write(&path, b"not a socket").unwrap();
        assert!(path.exists());

        let p2 = path.clone();
        std::thread::spawn(move || {
            let gen: Arc<dyn Fn(&str, usize) -> String + Send + Sync> =
                Arc::new(|_: &str, _: usize| String::new());
            let _ = serve_unix(&p2, Keyring::default(), gen);
        });
        let mut ok = false;
        for _ in 0..200 {
            if std::os::unix::net::UnixStream::connect(&path).is_ok() { ok = true; break; }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(ok, "the stale file was not cleared");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
