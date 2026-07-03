// src/track_server.rs
#![allow(dead_code)]
//
// Tiny HTTP server that exposes local audio files to networked speakers.
// The Beosound Stage (and most B&O networked devices) play audio from a
// URL via their network-audio source — they can't read the controller's
// local filesystem directly. This server bridges the gap:
//
//   1. The controller registers a file path with `expose(path)`.
//   2. We return a tokenized URL like http://<local_ip>:8473/t/<32-char-hex>
//   3. The device GETs that URL and streams the file.
//   4. After playback, the registration can be revoked (or left to expire).
//
// HONEST SCOPE:
//   - HTTP only (no HTTPS). Devices on the same LAN are trusted.
//   - Tokens are random 128-bit values; not easily guessable, but anyone
//     on the LAN who learns a token can fetch the file. That's fine for
//     home use — it's the same trust model as DLNA.
//   - We support Range requests because many DLNA/UPnP-style speakers
//     emit them for seek and even initial connect. Without Range support
//     the device often just plays a few seconds then stops.
//   - Content-Type is sniffed from the file extension. Most B&O players
//     accept MP3/FLAC/M4A/OGG/WAV out of the box.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Default TCP port for the server. Chosen out of band of anything
/// commonly used; the controller binds it on startup.
pub const DEFAULT_PORT: u16 = 8473;

/// Returns the local IP the host would use to reach `peer`. Doesn't
/// actually send any data — just lets the kernel decide which interface
/// would be used, then reads back the local addr. This is the only
/// reliable way to pick the "right" IP on a multi-NIC box.
pub fn local_ip_for(peer: IpAddr) -> Option<IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    // Port doesn't matter; we never send.
    sock.connect(SocketAddr::new(peer, 9)).ok()?;
    sock.local_addr().ok().map(|a| a.ip())
}

/// A registered route can be either a path on disk (a track file the
/// user wants to play) OR an in-memory blob (a synthesized WAV like
/// the device-identification ping or a cue). Same URL surface, same
/// Range support, but the in-memory branch sidesteps the temp-file
/// dance entirely.
#[derive(Debug, Clone)]
pub enum RouteEntry {
    File(PathBuf),
    Bytes { content_type: String, data: Arc<Vec<u8>> },
    /// Live OggOpus stream produced by piping a source WAV through
    /// the realtime mixer on the fly. Each GET opens a fresh per-
    /// connection Opus encoder + biquad state and reads parameters
    /// from the shared mixer on every audio frame, so slider moves
    /// affect the in-flight audio after the device's internal
    /// buffer (typically a few hundred ms) plays out. Pacing is
    /// wall-clock 20 ms/frame so the TCP send buffer stays close
    /// to empty — without that throttle, the encoder runs at ~50×
    /// realtime and slider moves would only affect audio seconds
    /// later when the buffer finally drained.
    MixerStream {
        source_path: PathBuf,
        mixer: Arc<std::sync::RwLock<crate::realtime_mixer::RealtimeMixer>>,
        bitrate_kbps: u32,
    },
}

/// Shared map: token → file path. Held behind a Mutex so the listener
/// thread can read it while the controller's main thread writes new
/// registrations.
type RouteTable = Arc<Mutex<HashMap<String, RouteEntry>>>;

pub struct TrackServer {
    routes: RouteTable,
    bound_addr: SocketAddr,
}

impl TrackServer {
    /// Bind a TCP listener on `0.0.0.0:port` and spawn a thread that
    /// accepts and handles connections. Returns Err if binding fails
    /// (port in use, no permissions, etc.).
    pub fn bind(port: u16) -> Result<Self, String> {
        let listener = TcpListener::bind(("0.0.0.0", port))
            .map_err(|e| format!("bind 0.0.0.0:{}: {}", port, e))?;
        let bound = listener.local_addr().map_err(|e| e.to_string())?;
        let routes: RouteTable = Arc::new(Mutex::new(HashMap::new()));
        let routes_for_thread = routes.clone();
        thread::spawn(move || run_listener(listener, routes_for_thread));
        eprintln!("track_server: listening on {}", bound);
        Ok(Self { routes, bound_addr: bound })
    }

    /// Register a file path. Returns the tokenized URL path component
    /// (e.g. "/t/abcd…"). The caller prepends scheme + host.
    pub fn expose(&self, path: PathBuf) -> String {
        let token = random_token();
        if let Ok(mut r) = self.routes.lock() {
            r.insert(token.clone(), RouteEntry::File(path));
        }
        format!("/t/{}", token)
    }

    /// Register an in-memory byte buffer (a synthesized WAV, a small
    /// cue, the device-identification ping, etc.) under a tokenized
    /// URL. `content_type` is sent literally in the response — typically
    /// `audio/wav` for our use. Holds an Arc to the data so multiple
    /// fetches don't copy.
    pub fn expose_bytes(&self, content_type: &str, data: Vec<u8>) -> String {
        let token = random_token();
        if let Ok(mut r) = self.routes.lock() {
            r.insert(token.clone(), RouteEntry::Bytes {
                content_type: content_type.to_string(),
                data: Arc::new(data),
            });
        }
        format!("/t/{}", token)
    }

    /// Register a live mixer stream. The route holds a clone of the
    /// shared `Arc<RwLock<RealtimeMixer>>`; per-connection state (the
    /// Opus encoder + a local copy of the biquad delay lines) is
    /// created when the device GETs the URL. Mixer parameters are
    /// pulled from the shared lock on every 20 ms frame, so slider
    /// moves take effect after the device's internal buffer drains.
    pub fn expose_mixer_stream(
        &self,
        source_path: PathBuf,
        mixer: Arc<std::sync::RwLock<crate::realtime_mixer::RealtimeMixer>>,
        bitrate_kbps: u32,
    ) -> String {
        let token = random_token();
        if let Ok(mut r) = self.routes.lock() {
            r.insert(token.clone(), RouteEntry::MixerStream {
                source_path,
                mixer,
                bitrate_kbps,
            });
        }
        format!("/t/{}", token)
    }

    /// Build a full URL the device can fetch. `peer_ip` is the device IP;
    /// we pick the matching local interface IP for the host portion.
    pub fn url_for(&self, peer_ip: IpAddr, path_segment: &str) -> String {
        let host = match local_ip_for(peer_ip) {
            Some(ip) if !ip.is_loopback() => ip.to_string(),
            Some(ip) => {
                // local_ip_for returned loopback. This happens when
                // the peer is itself loopback (test loopback to
                // a controller running on the same box) — in that
                // case it's correct. In ALL OTHER cases this is a
                // misconfig: we have no route to the device, or the
                // OS picked the wrong interface, and serving them a
                // 127.0.0.1 URL guarantees fetch failure. Log
                // loudly so the user has something to grep for.
                if !peer_ip.is_loopback() {
                    eprintln!("track_server: WARNING — local_ip_for({}) \
                               returned loopback {}; the device will \
                               try to fetch from itself and fail. \
                               Network may be misconfigured.",
                              peer_ip, ip);
                }
                ip.to_string()
            }
            None => {
                eprintln!("track_server: WARNING — local_ip_for({}) \
                           returned None (UDP probe failed). Falling \
                           back to 127.0.0.1 which the device cannot \
                           reach; playback will fail.",
                          peer_ip);
                "127.0.0.1".to_string()
            }
        };
        format!("http://{}:{}{}", host, self.bound_addr.port(), path_segment)
    }

    /// Remove all registrations. Useful between sessions or when the
    /// active device changes. Active connections aren't disturbed.
    pub fn clear(&self) {
        if let Ok(mut r) = self.routes.lock() { r.clear(); }
    }

    pub fn port(&self) -> u16 { self.bound_addr.port() }

    /// Verify the server is reachable from the LAN-side, not just
    /// loopback. Opens a TCP connection FROM the controller's
    /// real LAN IP (as discovered via `local_ip_for(peer_ip)`) TO
    /// our own bound port. If this fails, an outbound firewall —
    /// or more often, an INPUT chain blocking the non-standard
    /// port — is the cause. The device will see the same failure
    /// when it tries to fetch a track URL.
    ///
    /// On postmarketOS / Alpine the default policy varies between
    /// images; this probe is the single most useful diagnostic
    /// when "music doesn't play" with no other clear reason.
    ///
    /// Returns Ok(()) if reachable, or an Err with a human-readable
    /// description when not. Doesn't fail the bind on error — that
    /// would prevent the controller from ever starting on a host
    /// where the firewall is restrictive — instead the caller logs
    /// the warning and surfaces it to the UI's diagnostic page.
    pub fn check_reachable_from_lan(&self, peer_ip: IpAddr) -> Result<(), String> {
        let local = local_ip_for(peer_ip)
            .ok_or_else(|| "no route to peer (no NICs?)".to_string())?;
        if local.is_loopback() {
            return Err(format!(
                "only loopback ({}) reachable to peer {} — \
                 device is on a different segment or routing is broken",
                local, peer_ip));
        }
        // Connect to the bound port from any source.
        // Short timeout — if the firewall is blocking, the TCP
        // handshake won't return in 200ms.
        let target = SocketAddr::new(local, self.bound_addr.port());
        let timeout = Duration::from_millis(500);
        match TcpStream::connect_timeout(&target, timeout) {
            Ok(_) => Ok(()),
            Err(e) => Err(format!(
                "TCP connect to own bound port {} failed: {} \
                 (firewall likely blocking port {} — see \
                 sudo nft list ruleset OR sudo iptables -L INPUT)",
                target, e, self.bound_addr.port())),
        }
    }
}

/// Generate a 128-bit random token rendered as hex. Uses /dev/urandom
/// via std::fs to avoid a dep on the `rand` crate.
fn random_token() -> String {
    let mut buf = [0u8; 16];
    if let Ok(mut f) = File::open("/dev/urandom") {
        let _ = f.read_exact(&mut buf);
    } else {
        // Fallback — non-cryptographic but better than zero entropy.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).ok()
            .map(|d| d.as_nanos()).unwrap_or(0);
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (now >> (i * 4)) as u8;
        }
    }
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

fn run_listener(listener: TcpListener, routes: RouteTable) {
    for stream in listener.incoming() {
        if let Ok(s) = stream {
            // Disable Nagle's algorithm. Without TCP_NODELAY, small
            // initial writes (the response headers) get coalesced with
            // subsequent writes for up to ~200 ms — the device sees the
            // headers + first audio bytes arrive in a burst rather than
            // immediately, delaying when it can start decoding. For an
            // identification ping or a short cue this is the single
            // biggest source of preventable latency on the controller
            // side.
            let _ = s.set_nodelay(true);
            let r = routes.clone();
            thread::spawn(move || {
                let _ = s.set_read_timeout(Some(Duration::from_secs(15)));
                let _ = s.set_write_timeout(Some(Duration::from_secs(60)));
                handle_connection(s, r);
            });
        }
    }
}

fn handle_connection(mut stream: TcpStream, routes: RouteTable) {
    let mut buf = [0u8; 2048];
    let n = match stream.read(&mut buf) {
        Ok(0) => return,
        Ok(n) => n,
        Err(_) => return,
    };
    let req = String::from_utf8_lossy(&buf[..n]);
    let (method, path, range) = parse_request(&req);
    let path = path.split('?').next().unwrap_or(path);

    let peer = stream.peer_addr().ok();
    eprintln!("track_server: {} {} from {:?} range={:?}",
              method, path, peer, range);

    if method != "GET" && method != "HEAD" {
        let _ = write_status(&mut stream, 405, "Method Not Allowed", "");
        return;
    }
    if path == "/health" {
        let _ = write_status(&mut stream, 200, "OK", "ok");
        return;
    }
    let token = match path.strip_prefix("/t/") {
        Some(t) => t,
        None => { let _ = write_status(&mut stream, 404, "Not Found", ""); return; }
    };
    let entry = match routes.lock().ok().and_then(|r| r.get(token).cloned()) {
        Some(e) => e,
        None => { let _ = write_status(&mut stream, 404, "Not Found", "unknown token"); return; }
    };
    match entry {
        RouteEntry::File(path) => serve_file(&mut stream, &path, range, method == "HEAD"),
        RouteEntry::Bytes { content_type, data } => {
            serve_bytes(&mut stream, &content_type, &data, range, method == "HEAD");
        }
        RouteEntry::MixerStream { source_path, mixer, bitrate_kbps } => {
            serve_mixer_stream(&mut stream, &source_path, mixer, bitrate_kbps,
                               method == "HEAD");
        }
    }
}

/// Parse the request line + look for a Range header. Returns
/// (method, path, optional_range_byte_start).
fn parse_request<'a>(req: &'a str) -> (&'a str, &'a str, Option<(u64, Option<u64>)>) {
    let mut lines = req.lines();
    let first = lines.next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");
    let mut range = None;
    for line in lines {
        let low = line.to_ascii_lowercase();
        if let Some(v) = low.strip_prefix("range:") {
            let v = v.trim();
            if let Some(spec) = v.strip_prefix("bytes=") {
                let (s, e) = match spec.split_once('-') {
                    Some((s, e)) => (s, e),
                    None => continue,
                };
                let start: u64 = s.trim().parse().unwrap_or(0);
                let end: Option<u64> = if e.trim().is_empty() { None }
                                      else { e.trim().parse().ok() };
                range = Some((start, end));
            }
        }
    }
    (method, path, range)
}

fn write_status(stream: &mut TcpStream, code: u16, reason: &str, body: &str) -> std::io::Result<()> {
    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        code, reason, body.len()
    );
    stream.write_all(header.as_bytes())?;
    if !body.is_empty() { stream.write_all(body.as_bytes())?; }
    Ok(())
}

fn content_type_for(path: &std::path::Path) -> &'static str {
    let ext = path.extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "m4a" | "mp4" | "aac" => "audio/mp4",
        "aiff" | "aif" => "audio/aiff",
        _ => "application/octet-stream",
    }
}

/// Resolve a (possibly absent, possibly inverted, possibly open-ended)
/// HTTP Range header against a resource of `total` bytes, returning
/// the concrete (start, end) inclusive byte indices. Returns Err when
/// the range is unsatisfiable per RFC 7233:
///
///   • zero-length resource with any range
///   • start ≥ total
///   • explicit end < start (inverted)
///   • explicit end ≥ total (out of bounds)
///
/// Caller should respond 416 Range Not Satisfiable on Err. None (no
/// Range header) on a zero-length resource is special-cased to
/// Ok((0, 0)) so the caller's len calculation still works.
///
/// Pure function — extracted from the panicking old serve_bytes /
/// silently-broken serve_file logic so we can unit-test it without
/// a real TCP socket.
fn validate_range(
    range: Option<(u64, Option<u64>)>,
    total: u64,
) -> Result<(u64, u64), ()> {
    match range {
        Some((s, Some(e))) => {
            if total == 0 || s >= total || e < s || e >= total {
                Err(())
            } else {
                Ok((s, e))
            }
        }
        Some((s, None)) => {
            if total == 0 || s >= total {
                Err(())
            } else {
                Ok((s, total - 1))
            }
        }
        None => {
            if total == 0 { Ok((0, 0)) } else { Ok((0, total - 1)) }
        }
    }
}

fn serve_file(
    stream: &mut TcpStream,
    path: &std::path::Path,
    range: Option<(u64, Option<u64>)>,
    head_only: bool,
) {
    let f = match File::open(path) {
        Ok(f) => f,
        Err(e) => { let _ = write_status(stream, 500, "Internal Server Error", &e.to_string()); return; }
    };
    let total = match f.metadata() { Ok(m) => m.len(), Err(_) => 0 };
    let ctype = content_type_for(path);

    let (start, end) = match validate_range(range, total) {
        Ok(r) => r,
        Err(()) => {
            let _ = write_416(stream, total);
            return;
        }
    };
    let len = if total == 0 { 0 } else { end - start + 1 };

    let (code, reason) = if range.is_some() { (206, "Partial Content") } else { (200, "OK") };
    let mut header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nConnection: close\r\n",
        code, reason, ctype, len
    );
    if range.is_some() {
        header.push_str(&format!("Content-Range: bytes {}-{}/{}\r\n", start, end, total));
    }
    header.push_str("\r\n");
    if stream.write_all(header.as_bytes()).is_err() { return; }
    if head_only || len == 0 { return; }

    let mut reader = BufReader::new(f);
    if start > 0 {
        if reader.seek(SeekFrom::Start(start)).is_err() { return; }
    }
    let mut sent: u64 = 0;
    let mut chunk = [0u8; 32 * 1024];
    while sent < len {
        let want = ((len - sent) as usize).min(chunk.len());
        let n = match reader.read(&mut chunk[..want]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        if stream.write_all(&chunk[..n]).is_err() { break; }
        sent += n as u64;
    }
}

/// Send a 416 Range Not Satisfiable response with a Content-Range
/// header naming the actual resource length (per RFC 7233 §4.4).
/// The Content-Range form `bytes */<len>` tells the client the only
/// thing they're allowed to know: how big the resource actually is.
fn write_416(stream: &mut TcpStream, total: u64) -> std::io::Result<()> {
    let body = "Range Not Satisfiable";
    let header = format!(
        "HTTP/1.1 416 Range Not Satisfiable\r\n\
         Content-Type: text/plain\r\n\
         Content-Range: bytes */{}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        total,
        body.len(),
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    Ok(())
}

fn serve_bytes(
    stream: &mut TcpStream,
    content_type: &str,
    data: &[u8],
    range: Option<(u64, Option<u64>)>,
    head_only: bool,
) {
    let total = data.len() as u64;
    let (start, end) = match validate_range(range, total) {
        Ok(r) => r,
        Err(()) => {
            let _ = write_416(stream, total);
            return;
        }
    };
    let len = if total == 0 { 0 } else { end - start + 1 };
    let (code, reason) = if range.is_some() { (206, "Partial Content") } else { (200, "OK") };
    let mut header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nConnection: close\r\n",
        code, reason, content_type, len
    );
    if range.is_some() {
        header.push_str(&format!("Content-Range: bytes {}-{}/{}\r\n", start, end, total));
    }
    header.push_str("\r\n");
    if stream.write_all(header.as_bytes()).is_err() { return; }
    if head_only || len == 0 { return; }
    let s = start as usize;
    let e = (end as usize) + 1;
    let _ = stream.write_all(&data[s..e]);
}

/// Serve a live OggOpus stream produced by piping `source_path`
/// through the shared mixer in 20 ms frames, with wall-clock pacing
/// so the device's internal buffer stays the only meaningful source
/// of latency. Slider moves on the controller side reach the speaker
/// within ~one buffer's worth of audio (typical: a few hundred ms).
///
/// Architecture per connection:
///   • Read the WAV ONCE up front (small files: the whole thing in
///     RAM; for large files we'd switch to incremental decode, but
///     ambient / podcast / per-track material fits comfortably).
///   • Resample to 48 kHz upfront — one-shot work, keeps the
///     per-frame loop simple.
///   • Build a per-connection RealtimeMixer (its own biquad state)
///     and sync its params from the shared mixer once before the
///     loop and again on every frame. set_params preserves the
///     biquad delay lines so sweeps don't pop.
///   • For each 960-sample (20 ms) frame: snapshot params, process,
///     encode via the per-connection Opus encoder, write the packet,
///     sleep until 20 ms wall-clock has elapsed since stream start.
///   • Broken pipe (device disconnects, user stops playback) is
///     handled by checking write results — we just bail out of the
///     loop without further error reporting.
fn serve_mixer_stream(
    stream: &mut TcpStream,
    source_path: &Path,
    mixer: Arc<std::sync::RwLock<crate::realtime_mixer::RealtimeMixer>>,
    bitrate_kbps: u32,
    head_only: bool,
) {
    use std::io::Write;
    use std::time::Instant;

    // Headers. We don't include Content-Length because the stream
    // is generated on the fly; many DLNA renderers handle this
    // fine with Connection: close. Content-Type is the standard
    // OggOpus MIME type.
    let header = "HTTP/1.1 200 OK\r\n\
                  Content-Type: audio/ogg\r\n\
                  Cache-Control: no-store\r\n\
                  Connection: close\r\n\r\n";
    if stream.write_all(header.as_bytes()).is_err() { return; }
    if head_only { return; }

    // ── Decode + resample once ─────────────────────────────────
    let buf = match crate::looper::read_wav(source_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("track_server: mixer_stream read_wav failed: {}", e);
            return;
        }
    };
    let pcm_48k = crate::looper::linear_resample_to_48k(
        &buf.samples_lr, buf.sample_rate);
    if pcm_48k.is_empty() {
        eprintln!("track_server: mixer_stream got empty pcm");
        return;
    }

    // ── Per-connection mixer with shared params ────────────────
    let mut local_mixer = crate::realtime_mixer::RealtimeMixer::new(48_000.0);
    if let Ok(m) = mixer.read() {
        local_mixer.set_params(m.params());
    }

    // ── Opus encoder ───────────────────────────────────────────
    const FRAME: usize = 960;   // 20 ms at 48 kHz
    let mut encoder = match opus::Encoder::new(
        48_000, opus::Channels::Stereo, opus::Application::Audio)
    {
        Ok(e) => e,
        Err(e) => {
            eprintln!("track_server: mixer_stream opus init failed: {}", e);
            return;
        }
    };
    let _ = encoder.set_bitrate(opus::Bitrate::Bits(
        (bitrate_kbps.max(24) * 1000) as i32));

    // ── Ogg writer wrapping the TCP stream ─────────────────────
    // Wrap in a BufWriter so small Opus packets coalesce into
    // larger TCP writes — reduces syscall count and lets Nagle
    // (still off via set_nodelay earlier) not be the bottleneck.
    let mut writer = ogg::writing::PacketWriter::new(
        std::io::BufWriter::new(stream.try_clone()
            .expect("track_server: TcpStream clone for mixer_stream")));
    let serial: u32 = 0xB0_C0_BE_EF;   // arbitrary fixed serial — single stream

    // OpusHead per RFC 7845 §5.1.
    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");
    head.push(1);                                  // version
    head.push(2);                                  // channels
    head.extend_from_slice(&3840u16.to_le_bytes()); // pre-skip
    head.extend_from_slice(&48_000u32.to_le_bytes()); // input sr
    head.extend_from_slice(&0i16.to_le_bytes());   // gain
    head.push(0);                                  // mapping family
    if writer.write_packet(head, serial,
        ogg::writing::PacketWriteEndInfo::EndPage, 0).is_err()
    { return; }

    // OpusTags.
    let mut tags = Vec::new();
    tags.extend_from_slice(b"OpusTags");
    let vendor = b"bo-controller/stream";
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes());
    if writer.write_packet(tags, serial,
        ogg::writing::PacketWriteEndInfo::EndPage, 0).is_err()
    { return; }

    // ── Stream loop ────────────────────────────────────────────
    let start = Instant::now();
    let mut pos: usize = 0;
    let mut frame_idx: u64 = 0;
    let mut granule: u64 = 3840;
    let mut packet_buf = vec![0u8; 4096];
    let mut chunk_buf = vec![(0.0_f32, 0.0_f32); FRAME];
    let mut interleaved = [0.0_f32; FRAME * 2];

    while pos < pcm_48k.len() {
        // Snapshot the current shared params. Held briefly; the
        // read lock contends with QtState's write lock only when
        // the user is actively dragging a slider.
        if let Ok(m) = mixer.read() {
            local_mixer.set_params(m.params());
        }

        // Copy chunk into the processing buffer (padding with zeros
        // at end-of-file so the encoder always receives full frames).
        let take = (pcm_48k.len() - pos).min(FRAME);
        for i in 0..take {
            chunk_buf[i] = pcm_48k[pos + i];
        }
        for i in take..FRAME {
            chunk_buf[i] = (0.0, 0.0);
        }
        local_mixer.process_buffer(&mut chunk_buf);

        // Interleave + encode.
        for i in 0..FRAME {
            interleaved[i * 2]     = chunk_buf[i].0;
            interleaved[i * 2 + 1] = chunk_buf[i].1;
        }
        let n_bytes = match encoder.encode_float(&interleaved, &mut packet_buf) {
            Ok(n) => n,
            Err(e) => {
                eprintln!("track_server: mixer_stream encode failed: {}", e);
                break;
            }
        };

        pos += FRAME;
        granule += FRAME as u64;
        let end_info = if pos >= pcm_48k.len() {
            ogg::writing::PacketWriteEndInfo::EndStream
        } else {
            ogg::writing::PacketWriteEndInfo::NormalPacket
        };
        if writer.write_packet(packet_buf[..n_bytes].to_vec(),
                               serial, end_info, granule).is_err()
        {
            // Client disconnected mid-stream — bail without noise.
            break;
        }

        // Wall-clock pacing. Sleeping until 20 ms × frame_idx have
        // elapsed since `start` keeps the TCP write buffer near-empty,
        // which means slider moves are reflected in audible audio
        // within roughly the device's playout buffer (a few hundred
        // ms typically). Without this throttle the encoder runs at
        // ~50× realtime and the full file would fill the TCP buffer
        // in seconds, making slider moves audibly lag for many bars.
        frame_idx += 1;
        let target = start + std::time::Duration::from_millis(frame_idx * 20);
        let now = Instant::now();
        if target > now {
            std::thread::sleep(target - now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(name: &str, body: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(name);
        let mut f = File::create(&path).unwrap();
        f.write_all(body).unwrap();
        path
    }

    #[test]
    fn token_is_32_hex_chars() {
        let t = random_token();
        assert_eq!(t.len(), 32);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn two_tokens_differ() {
        assert_ne!(random_token(), random_token());
    }

    #[test]
    fn parse_request_extracts_method_and_path() {
        let req = "GET /t/abc123 HTTP/1.1\r\nHost: foo\r\n\r\n";
        let (m, p, r) = parse_request(req);
        assert_eq!(m, "GET");
        assert_eq!(p, "/t/abc123");
        assert!(r.is_none());
    }

    #[test]
    fn parse_request_extracts_range() {
        let req = "GET /t/x HTTP/1.1\r\nRange: bytes=128-256\r\n\r\n";
        let (_, _, r) = parse_request(req);
        assert_eq!(r, Some((128, Some(256))));
    }

    #[test]
    fn parse_request_range_open_ended() {
        let req = "GET /t/x HTTP/1.1\r\nRange: bytes=1000-\r\n\r\n";
        let (_, _, r) = parse_request(req);
        assert_eq!(r, Some((1000, None)));
    }

    // ── validate_range ──────────────────────────────────────────────

    #[test]
    fn validate_range_no_header_full_resource() {
        assert_eq!(validate_range(None, 100), Ok((0, 99)));
    }

    #[test]
    fn validate_range_zero_length_resource() {
        // No-range request on an empty resource: synthesize (0,0) so
        // caller's len = 0 calculation works without panicking.
        assert_eq!(validate_range(None, 0), Ok((0, 0)));
        // Any explicit range on an empty resource is unsatisfiable.
        assert_eq!(validate_range(Some((0, Some(10))), 0), Err(()));
        assert_eq!(validate_range(Some((5, None)), 0), Err(()));
    }

    #[test]
    fn validate_range_normal_partial() {
        assert_eq!(validate_range(Some((10, Some(20))), 100), Ok((10, 20)));
        assert_eq!(validate_range(Some((0, Some(0))), 100), Ok((0, 0)));
        assert_eq!(validate_range(Some((99, Some(99))), 100), Ok((99, 99)));
    }

    #[test]
    fn validate_range_open_ended_normal() {
        assert_eq!(validate_range(Some((50, None)), 100), Ok((50, 99)));
        assert_eq!(validate_range(Some((0, None)), 100), Ok((0, 99)));
    }

    #[test]
    fn validate_range_inverted_rejected() {
        // bytes=100-50 — end before start. Old code returned len=1
        // and would have read 1 byte from offset 100 with Content-Range
        // bytes 100-50/total. PANIC for serve_bytes.
        assert_eq!(validate_range(Some((100, Some(50))), 1000), Err(()));
        assert_eq!(validate_range(Some((10, Some(9))), 100), Err(()));
    }

    #[test]
    fn validate_range_start_at_or_past_end_rejected() {
        // bytes=100- on a 100-byte file. Start equals length: there's
        // no first byte to send. Must 416.
        assert_eq!(validate_range(Some((100, None)), 100), Err(()));
        // bytes=200- on a 100-byte file: clearly past.
        assert_eq!(validate_range(Some((200, None)), 100), Err(()));
        // Same for explicit end.
        assert_eq!(validate_range(Some((100, Some(150))), 100), Err(()));
        assert_eq!(validate_range(Some((200, Some(250))), 100), Err(()));
    }

    #[test]
    fn validate_range_end_past_total_rejected() {
        // bytes=50-200 on a 100-byte file: end is past the resource.
        // RFC 7233 says reject.
        assert_eq!(validate_range(Some((50, Some(200))), 100), Err(()));
        assert_eq!(validate_range(Some((0, Some(100))), 100), Err(()));
    }

    #[test]
    fn validate_range_single_byte_at_boundary_ok() {
        // bytes=99-99 on a 100-byte file: last byte. Legal.
        assert_eq!(validate_range(Some((99, Some(99))), 100), Ok((99, 99)));
    }

    #[test]
    fn content_type_dispatches_by_extension() {
        use std::path::Path;
        assert_eq!(content_type_for(Path::new("a.mp3")), "audio/mpeg");
        assert_eq!(content_type_for(Path::new("b.FLAC")), "audio/flac");
        assert_eq!(content_type_for(Path::new("c.m4a")), "audio/mp4");
        assert_eq!(content_type_for(Path::new("d.unknown")), "application/octet-stream");
    }

    #[test]
    fn end_to_end_serves_a_file() {
        let body = b"BO-CONTROLLER-TRACK-SERVER-TEST-PAYLOAD-1234567890";
        let tmp = write_tmp("bo_ts_e2e.mp3", body);
        let srv = TrackServer::bind(0).expect("bind");  // 0 = OS-assign
        let token_path = srv.expose(tmp.clone());
        // GET /t/<token>
        let mut sock = TcpStream::connect(("127.0.0.1", srv.port())).unwrap();
        let req = format!("GET {} HTTP/1.1\r\nHost: localhost\r\n\r\n", token_path);
        sock.write_all(req.as_bytes()).unwrap();
        let mut resp = Vec::new();
        sock.read_to_end(&mut resp).unwrap();
        let s = String::from_utf8_lossy(&resp);
        assert!(s.contains("200 OK") || s.contains("206 Partial Content"));
        assert!(s.contains("Content-Type: audio/mpeg"));
        // Body should be at the end.
        let sep = resp.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        assert_eq!(&resp[sep + 4..], body);
    }

    #[test]
    fn end_to_end_range_request() {
        let body = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";  // 36 bytes
        let tmp = write_tmp("bo_ts_range.mp3", body);
        let srv = TrackServer::bind(0).expect("bind");
        let token_path = srv.expose(tmp.clone());
        let mut sock = TcpStream::connect(("127.0.0.1", srv.port())).unwrap();
        let req = format!(
            "GET {} HTTP/1.1\r\nHost: localhost\r\nRange: bytes=10-20\r\n\r\n",
            token_path
        );
        sock.write_all(req.as_bytes()).unwrap();
        let mut resp = Vec::new();
        sock.read_to_end(&mut resp).unwrap();
        let s = String::from_utf8_lossy(&resp);
        assert!(s.contains("206 Partial Content"));
        assert!(s.contains("Content-Range: bytes 10-20/36"));
        let sep = resp.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        // bytes 10..=20 = "ABCDEFGHIJK" (11 bytes)
        assert_eq!(&resp[sep + 4..], b"ABCDEFGHIJK");
    }

    #[test]
    fn unknown_token_returns_404() {
        let srv = TrackServer::bind(0).expect("bind");
        let mut sock = TcpStream::connect(("127.0.0.1", srv.port())).unwrap();
        sock.write_all(b"GET /t/notreal HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut resp = Vec::new();
        sock.read_to_end(&mut resp).unwrap();
        assert!(String::from_utf8_lossy(&resp).contains("404"));
    }

    #[test]
    fn mixer_stream_serves_valid_oggopus() {
        // End-to-end: write a tiny WAV → expose as a mixer stream →
        // GET it → verify the response is an OggOpus payload.
        // Kept small (≈ 160 ms of audio at 48 kHz) so the wall-clock
        // pacing inside serve_mixer_stream doesn't drag the test.
        let sr = 48_000_u32;
        let n_frames = (sr as usize / 50) * 8;       // 8 × 20 ms = 160 ms
        let wav_path = std::env::temp_dir().join("bo_ts_mixer_stream.wav");
        {
            let spec = hound::WavSpec {
                channels: 2, sample_rate: sr,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut w = hound::WavWriter::create(&wav_path, spec).unwrap();
            for i in 0..n_frames {
                // Some non-silent content so the encoder produces
                // non-trivial packets.
                let v = ((i as f32 * 2.0 * std::f32::consts::PI * 440.0)
                         / sr as f32).sin() * 0.3;
                let s = (v * 32767.0) as i16;
                w.write_sample(s).unwrap();
                w.write_sample(s).unwrap();
            }
            w.finalize().unwrap();
        }

        let mixer = std::sync::Arc::new(std::sync::RwLock::new(
            crate::realtime_mixer::RealtimeMixer::new(48_000.0)));

        let srv = TrackServer::bind(0).expect("bind");
        let token_path = srv.expose_mixer_stream(wav_path.clone(), mixer, 96);

        let mut sock = TcpStream::connect(("127.0.0.1", srv.port())).unwrap();
        sock.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        let req = format!("GET {} HTTP/1.1\r\nHost: x\r\n\r\n", token_path);
        sock.write_all(req.as_bytes()).unwrap();
        let mut resp = Vec::new();
        sock.read_to_end(&mut resp).unwrap();

        let head_end = resp.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&resp[..head_end]);
        assert!(head.contains("200 OK"), "expected 200, got: {}", head);
        assert!(head.contains("Content-Type: audio/ogg"),
            "expected audio/ogg, got: {}", head);

        let body = &resp[head_end + 4..];
        assert!(body.starts_with(b"OggS"),
            "expected OggS magic at body start, got bytes: {:?}",
            &body[..body.len().min(16)]);
        assert!(body.windows(8).any(|w| w == b"OpusHead"),
            "OpusHead not found in streamed body");
        // Should be tens to hundreds of bytes per 20 ms frame at
        // 96 kbps — for 160 ms we expect ≥ 1 KB total.
        assert!(body.len() > 1_000,
            "stream body too short: {} bytes", body.len());

        std::fs::remove_file(&wav_path).ok();
    }
}
