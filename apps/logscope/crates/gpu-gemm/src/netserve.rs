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

/// HTTP/1.1 chunked transfer, so a reply can arrive as it is produced.
///
/// The daemon exists to hold one copy of the weights, but its endpoint
/// returned the whole reply at once — and at the ~4 tok/s this engine
/// reports, a 170-token answer is forty seconds of a still UI followed
/// by a paragraph. That would make the daemon feel WORSE than loading
/// your own model, in exactly the app where its memory saving is
/// largest.
///
/// Chunked rather than a second framed protocol on the unix socket: the
/// endpoint already speaks HTTP/1.1, this is the conventional mechanism,
/// and it keeps the unix door the SAME endpoint as the TCP one rather
/// than a parallel one that can drift.
pub mod chunked {
    /// The response head. No Content-Length — that is what tells the
    /// client to read chunks, and sending both is the classic way to
    /// produce a response nobody can parse.
    pub fn head(status: u16, reason: &str, content_type: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\n\
             Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
        )
        .into_bytes()
    }

    /// One chunk: hex length, CRLF, payload, CRLF.
    ///
    /// An EMPTY piece encodes to nothing rather than to `0\r\n\r\n`,
    /// because a zero-length chunk is the terminator — a generator that
    /// emitted an empty token would otherwise end the response early and
    /// truncate its own reply.
    pub fn chunk(piece: &str) -> Vec<u8> {
        if piece.is_empty() {
            return Vec::new();
        }
        let mut out = format!("{:x}\r\n", piece.len()).into_bytes();
        out.extend_from_slice(piece.as_bytes());
        out.extend_from_slice(b"\r\n");
        out
    }

    /// The terminator.
    pub fn end() -> Vec<u8> {
        b"0\r\n\r\n".to_vec()
    }

    /// Decode a chunked body. Returns None on a malformed length or a
    /// truncated chunk — a connection cut mid-reply must read as an
    /// error, not as a short answer, or a half-generated completion
    /// looks like a finished one.
    pub fn decode(body: &str) -> Option<String> {
        let mut out = String::new();
        let mut rest = body;
        loop {
            let (len_s, tail) = rest.split_once("\r\n")?;
            // Chunk extensions (`1a;foo=bar`) are legal and ignored.
            let len_s = len_s.split(';').next()?.trim();
            let n = usize::from_str_radix(len_s, 16).ok()?;
            if n == 0 {
                return Some(out);
            }
            if tail.len() < n {
                return None; // truncated: the sender died mid-chunk
            }
            out.push_str(&tail[..n]);
            rest = tail.get(n + 2..)?; // skip the payload's trailing CRLF
        }
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
    serve_unix_tracked(path, keys, gen, None)
}

/// Shared lifecycle state, if the caller wants idle-unload.
pub type LifeHandle = Arc<std::sync::Mutex<crate::daemonlife::Life>>;

/// Same, but reporting each request to a [`crate::daemonlife::Life`] so a
/// janitor can decide when the weights may be released.
///
/// `begin` and `end` bracket the HANDLER, not the connection: the window
/// that must not be interrupted is the generation, and a client that
/// opens a socket and never sends anything should not keep 7.2 GiB
/// resident for it.
///
/// `end` runs even if the handler panics, via a guard — a panicked
/// request that left `in_flight` raised would make the daemon immortal,
/// which is the failure `Life`'s saturating decrement guards against
/// from the other side.
pub fn serve_unix_tracked(
    path: &std::path::Path, keys: Keyring,
    gen: Arc<dyn Fn(&str, usize) -> String + Send + Sync>,
    life: Option<LifeHandle>,
) -> std::io::Result<()> {
    let _ = std::fs::remove_file(path);
    let listener = std::os::unix::net::UnixListener::bind(path)?;
    let keys = Arc::new(keys);
    let start = std::time::Instant::now();
    for stream in listener.incoming().flatten() {
        let (keys, gen, life) = (keys.clone(), gen.clone(), life.clone());
        std::thread::spawn(move || {
            let mut stream = stream;
            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
            if let Some(req) = read_request(&mut stream) {
                let _busy = life.as_ref().map(|l| Busy::begin(l.clone(), start));
                let resp = handle(&req, &keys, gen.as_ref());
                let _ = stream.write_all(&resp.bytes());
            }
            let _ = stream.flush();
        });
    }
    Ok(())
}

/// Raises `in_flight` for as long as it lives. A guard rather than a
/// matched pair of calls, so an early return or a panic in the handler
/// cannot leave the count raised forever.
struct Busy {
    life: LifeHandle,
    start: std::time::Instant,
}

impl Busy {
    fn begin(life: LifeHandle, start: std::time::Instant) -> Busy {
        if let Ok(mut l) = life.lock() {
            l.begin();
        }
        Busy { life, start }
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        // Poison ignored: if another request's thread panicked while
        // holding this lock, the count still has to come down or the
        // daemon never unloads again.
        let mut l = match self.life.lock() {
            Ok(l) => l,
            Err(e) => e.into_inner(),
        };
        l.end(self.start.elapsed());
    }
}

pub fn serve(
    listener: TcpListener, keys: Keyring,
    gen: Arc<dyn Fn(&str, usize) -> String + Send + Sync>,
) {
    serve_tracked(listener, keys, gen, None)
}

/// Same, reporting to a [`crate::daemonlife::Life`].
///
/// BOTH DOORS MUST REPORT or idle-unload is a bug rather than a feature:
/// one process serves TCP and unix together, so a `Life` fed only by the
/// unix socket would see a busy LAN client as silence and exit
/// mid-conversation. The idle window is about the PROCESS being unused,
/// not about one of its listeners being unused.
pub fn serve_tracked(
    listener: TcpListener, keys: Keyring,
    gen: Arc<dyn Fn(&str, usize) -> String + Send + Sync>,
    life: Option<LifeHandle>,
) {
    let keys = Arc::new(keys);
    let start = std::time::Instant::now();
    for stream in listener.incoming().flatten() {
        let (keys, gen, life) = (keys.clone(), gen.clone(), life.clone());
        std::thread::spawn(move || {
            let mut stream = stream;
            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
            if let Some(req) = read_request(&mut stream) {
                let _busy = life.as_ref().map(|l| Busy::begin(l.clone(), start));
                let resp = handle(&req, &keys, gen.as_ref());
                let _ = stream.write_all(&resp.bytes());
            }
            let _ = stream.flush();
        });
    }
}

#[cfg(test)]
mod chunk_tests {
    use super::chunked;

    #[test]
    fn tokens_round_trip_in_the_order_they_were_produced() {
        let mut body = Vec::new();
        for tok in ["let ", "x ", "= ", "1;"] {
            body.extend_from_slice(&chunked::chunk(tok));
        }
        body.extend_from_slice(&chunked::end());
        let text = String::from_utf8(body).unwrap();
        assert_eq!(chunked::decode(&text).as_deref(), Some("let x = 1;"));
    }

    #[test]
    fn an_empty_token_does_not_terminate_the_reply() {
        // A zero-length chunk IS the terminator, so a generator emitting
        // an empty token would end its own response early and the reply
        // would silently truncate. Empty pieces encode to nothing.
        assert!(chunked::chunk("").is_empty());
        let mut body = Vec::new();
        body.extend_from_slice(&chunked::chunk("a"));
        body.extend_from_slice(&chunked::chunk(""));
        body.extend_from_slice(&chunked::chunk("b"));
        body.extend_from_slice(&chunked::end());
        let text = String::from_utf8(body).unwrap();
        assert_eq!(chunked::decode(&text).as_deref(), Some("ab"));
    }

    #[test]
    fn a_truncated_body_is_an_error_not_a_short_answer() {
        // The failure that matters: a daemon killed mid-generation must
        // not look like it finished. A half-written completion accepted
        // as complete is worse than a visible error.
        let full = {
            let mut b = Vec::new();
            b.extend_from_slice(&chunked::chunk("hello world"));
            b.extend_from_slice(&chunked::end());
            String::from_utf8(b).unwrap()
        };
        assert!(chunked::decode(&full).is_some());
        // Cut inside the payload.
        assert_eq!(chunked::decode(&full[..8]), None, "truncation read as success");
        // A length that is not hex at all.
        assert_eq!(chunked::decode("zz\r\nhi\r\n0\r\n\r\n"), None);
    }

    #[test]
    fn multibyte_text_is_measured_in_bytes_not_characters() {
        // The length prefix is bytes. Counting chars would desynchronise
        // the stream on the first non-ASCII token and garble everything
        // after it.
        let s = "héllo → ok";
        let mut b = chunked::chunk(s);
        b.extend_from_slice(&chunked::end());
        let text = String::from_utf8(b).unwrap();
        assert!(text.starts_with(&format!("{:x}\r\n", s.len())));
        assert_eq!(chunked::decode(&text).as_deref(), Some(s));
    }

    #[test]
    fn the_head_does_not_promise_a_length_it_will_not_send() {
        // Content-Length AND Transfer-Encoding together is the classic
        // way to produce a response nobody can parse.
        let h = String::from_utf8(chunked::head(200, "OK", "text/plain")).unwrap();
        assert!(h.contains("Transfer-Encoding: chunked"), "{h}");
        assert!(!h.contains("Content-Length"), "{h}");
    }

    #[test]
    fn chunk_extensions_are_tolerated() {
        // Legal HTTP that a hand-rolled parser typically chokes on.
        assert_eq!(chunked::decode("2;foo=bar\r\nhi\r\n0\r\n\r\n").as_deref(), Some("hi"));
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
    fn a_tcp_request_keeps_the_daemon_alive_too() {
        // THE BUG THIS PREVENTS: one process serves both doors, so a
        // `Life` fed only by the unix socket sees a busy LAN client as
        // silence and exits mid-conversation. Idle means the PROCESS is
        // unused, not that one listener is.
        use std::io::{Read, Write};
        let life: LifeHandle =
            Arc::new(std::sync::Mutex::new(crate::daemonlife::Life::new(Duration::from_secs(600))));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let l2 = life.clone();
        std::thread::spawn(move || {
            let gen: Arc<dyn Fn(&str, usize) -> String + Send + Sync> =
                Arc::new(|_: &str, _: usize| {
                    // Hold the handler open long enough to observe
                    // in_flight from the test thread.
                    std::thread::sleep(Duration::from_millis(120));
                    "ok".into()
                });
            serve_tracked(listener, Keyring::new(vec![]), gen, Some(l2));
        });

        let mut sock = None;
        for _ in 0..200 {
            if let Ok(s) = std::net::TcpStream::connect(addr) { sock = Some(s); break; }
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut sock = sock.expect("tcp listener never came up");
        let body = "hi";
        let req = format!(
            "POST /generate?max=4 HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}", body.len());
        sock.write_all(req.as_bytes()).unwrap();

        // While the handler sleeps, the TCP request must be visible.
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(life.lock().unwrap().in_flight, 1,
                   "a TCP request did not register: the daemon could unload under it");

        let mut raw = String::new();
        sock.read_to_string(&mut raw).unwrap();
        assert!(raw.contains("ok"), "{raw}");
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(life.lock().unwrap().in_flight, 0, "the guard did not release");
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
