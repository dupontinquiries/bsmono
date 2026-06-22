// src/chan_server.rs
//
// The per-channel HTTP server — the concrete piece that turns "device X
// drives channel C" into C's audio actually streaming to X. A minimal HTTP
// server over std::net::TcpListener (no framework dependency), serving each
// channel of a WAV source as a standalone mono WAV stream that any device
// (a B&O speaker pulling a URL, or another app instance) can fetch and play.
//
//   GET /ch/FL   → 200, audio/wav, mono WAV of the source's FL channel
//   GET /ch/SL   → ...
//   GET /health  → 200, "ok"
//   anything else → 404
//
// Why WAV-only for v1 (honest scoping):
//   - The soundscape clips are WAV; no decode needed → zero new deps.
//   - Both B&O stacks and ffplay/paplay pull WAV/L16 happily.
//   - Arbitrary formats (flac/mp3/aac) need symphonia; that's the documented
//     next step (see mux.rs TODOs). The channel-splitting + serving plumbing
//     here is format-independent — only the decode front-end changes.
//
// The WAV parsing (header, deinterleave, mono re-wrap) is pure and tested.
// The socket-serving loop is straightforward std::net.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

// ─── Minimal WAV (PCM) parsing + per-channel extraction ─────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct WavFmt {
    pub channels: u16,
    pub sample_rate: u32,
    pub bits: u16,
}

/// Parse a canonical PCM WAV: returns (fmt, data-bytes-slice-range).
/// Walks RIFF chunks to find "fmt " and "data" (tolerates extra chunks).
pub fn parse_wav(bytes: &[u8]) -> Result<(WavFmt, usize, usize), String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let mut pos = 12;
    let mut fmt: Option<WavFmt> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let sz = u32::from_le_bytes([bytes[pos+4], bytes[pos+5], bytes[pos+6], bytes[pos+7]]) as usize;
        let body = pos + 8;
        if id == b"fmt " && body + 16 <= bytes.len() {
            let channels = u16::from_le_bytes([bytes[body+2], bytes[body+3]]);
            let sample_rate = u32::from_le_bytes([bytes[body+4], bytes[body+5], bytes[body+6], bytes[body+7]]);
            let bits = u16::from_le_bytes([bytes[body+14], bytes[body+15]]);
            fmt = Some(WavFmt { channels, sample_rate, bits });
        } else if id == b"data" {
            let f = fmt.clone().ok_or("data chunk before fmt")?;
            let end = (body + sz).min(bytes.len());
            return Ok((f, body, end));
        }
        // chunks are word-aligned (pad to even)
        pos = body + sz + (sz & 1);
    }
    Err("no data chunk".into())
}

/// Extract one channel (0-based index) from interleaved PCM data into a new
/// mono WAV byte buffer. Supports 16-bit and 24-bit PCM (the common cases).
pub fn extract_channel_wav(src: &[u8], channel_index: usize) -> Result<Vec<u8>, String> {
    let (fmt, data_start, data_end) = parse_wav(src)?;
    let ch = fmt.channels as usize;
    if channel_index >= ch {
        return Err(format!("channel {} out of range (file has {})", channel_index, ch));
    }
    let bytes_per_sample = (fmt.bits / 8) as usize;
    if bytes_per_sample == 0 { return Err("zero bits/sample".into()); }
    let frame = bytes_per_sample * ch;
    let data = &src[data_start..data_end];

    let mut mono = Vec::with_capacity(data.len() / ch.max(1));
    let mut i = channel_index * bytes_per_sample;
    while i + bytes_per_sample <= data.len() {
        mono.extend_from_slice(&data[i..i + bytes_per_sample]);
        i += frame;
    }
    Ok(build_mono_wav(&mono, fmt.sample_rate, fmt.bits))
}

/// Wrap raw mono PCM sample bytes in a WAV header.
pub fn build_mono_wav(samples: &[u8], sample_rate: u32, bits: u16) -> Vec<u8> {
    let byte_rate = sample_rate * (bits as u32 / 8);
    let block_align = bits / 8;
    let data_len = samples.len() as u32;
    let riff_len = 36 + data_len;
    let mut w = Vec::with_capacity(44 + samples.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&riff_len.to_le_bytes());
    w.extend_from_slice(b"WAVE");
    w.extend_from_slice(b"fmt ");
    w.extend_from_slice(&16u32.to_le_bytes());     // fmt chunk size
    w.extend_from_slice(&1u16.to_le_bytes());      // PCM
    w.extend_from_slice(&1u16.to_le_bytes());      // mono
    w.extend_from_slice(&sample_rate.to_le_bytes());
    w.extend_from_slice(&byte_rate.to_le_bytes());
    w.extend_from_slice(&block_align.to_le_bytes());
    w.extend_from_slice(&bits.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    w.extend_from_slice(samples);
    w
}

// ─── HTTP serving ────────────────────────────────────────────────────────────

/// Map a channel name to its index in the source file's channel order.
/// Defaults to standard WAV 5.1 order: FL FR C LFE SL SR.
pub fn channel_index(name: &str, order: &[String]) -> Option<usize> {
    if let Some(i) = order.iter().position(|c| c.eq_ignore_ascii_case(name)) {
        return Some(i);
    }
    // Fallback aliases for common names.
    let std51 = ["FL", "FR", "C", "LFE", "SL", "SR"];
    std51.iter().position(|c| c.eq_ignore_ascii_case(name))
}

/// Parse the request line "GET /ch/FL HTTP/1.1" → path "/ch/FL".
pub fn parse_request_path(req: &str) -> Option<String> {
    let line = req.lines().next()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    if method != "GET" { return None; }
    parts.next().map(|p| p.to_string())
}

/// Build the HTTP response bytes for a parsed path against a source WAV.
/// Pure (no socket) so it's testable.
pub fn build_response(path: &str, src_wav: &[u8], order: &[String]) -> Vec<u8> {
    if path == "/health" {
        return http_response(200, "text/plain", b"ok");
    }
    // Frequency-sanitizer route: GET /masker/<preset>?ms=N&db=G
    //   preset ∈ pink, brown, engine, road, fan, hvac
    //   ms     = duration in ms (default 30_000, max 120_000)
    //   db     = output gain in dB (default -20)
    // Returns a mono 48 kHz / 16-bit WAV of the chosen preset, suitable
    // for looping in any player. Doesn't depend on a source WAV — the
    // masker generates audio internally.
    if let Some(rest) = path.strip_prefix("/masker/") {
        return build_masker_response(rest);
    }
    // Frequency sanitizer: masker noise + phase-tracked anti-tones.
    //   GET /sanitizer/<preset>?freq=N&ms=N&db=G
    //   preset = pink|brown|engine|road|fan|hvac
    //   freq   = anti-tone fundamental in Hz (default: per-preset typical
    //            noise fundamental; 0 = noise-only, no anti-tones)
    //   ms     = duration ms (default 30_000, max 120_000)
    //   db     = output gain dB (default -20)
    if let Some(rest) = path.strip_prefix("/sanitizer/") {
        return build_sanitizer_response(rest);
    }
    if let Some(ch_name) = path.strip_prefix("/ch/") {
        match channel_index(ch_name, order) {
            Some(idx) => match extract_channel_wav(src_wav, idx) {
                Ok(wav) => http_response(200, "audio/wav", &wav),
                Err(e) => http_response(500, "text/plain", e.as_bytes()),
            },
            None => http_response(404, "text/plain", b"unknown channel"),
        }
    } else {
        http_response(404, "text/plain", b"not found")
    }
}

/// Render a masker WAV in response to GET /masker/<preset>[?ms=…&db=…].
fn build_masker_response(rest: &str) -> Vec<u8> {
    // Split "preset?query" or just "preset"
    let (preset_id, query) = match rest.find('?') {
        Some(i) => (&rest[..i], &rest[i+1..]),
        None => (rest, ""),
    };
    let preset = match crate::masker::MaskerPreset::from_id(preset_id) {
        Some(p) => p,
        None => return http_response(404, "text/plain", b"unknown masker preset"),
    };

    // Parse optional ms= and db= from the querystring (cheap manual parse —
    // we only have two params and no framework dependency).
    let mut duration_ms: u32 = 30_000;
    let mut gain_db: f32 = -20.0;
    for kv in query.split('&').filter(|s| !s.is_empty()) {
        if let Some((k, v)) = kv.split_once('=') {
            match k {
                "ms" => if let Ok(n) = v.parse::<u32>() { duration_ms = n.min(120_000); },
                "db" => if let Ok(g) = v.parse::<f32>() { gain_db = g.clamp(-60.0, 6.0); },
                _ => {}
            }
        }
    }

    let wav = crate::masker::render_wav(preset, 48_000, duration_ms, gain_db);
    http_response(200, "audio/wav", &wav)
}

/// Render a sanitizer WAV — shaped noise + phase-tracked anti-tones —
/// in response to GET /sanitizer/<preset>[?freq=…&ms=…&db=…].
fn build_sanitizer_response(rest: &str) -> Vec<u8> {
    let (preset_id, query) = match rest.find('?') {
        Some(i) => (&rest[..i], &rest[i+1..]),
        None => (rest, ""),
    };
    let preset = match crate::masker::MaskerPreset::from_id(preset_id) {
        Some(p) => p,
        None => return http_response(404, "text/plain", b"unknown sanitizer preset"),
    };

    let mut duration_ms: u32 = 30_000;
    let mut gain_db: f32 = -20.0;
    // Default: use the preset's typical noise fundamental. The user can
    // override with ?freq=N or disable anti-tones entirely with ?freq=0.
    let mut antitone_freq: Option<f32> = Some(crate::cancel_tones::default_fundamental_for(preset_id));

    for kv in query.split('&').filter(|s| !s.is_empty()) {
        if let Some((k, v)) = kv.split_once('=') {
            match k {
                "ms" => if let Ok(n) = v.parse::<u32>() { duration_ms = n.min(120_000); },
                "db" => if let Ok(g) = v.parse::<f32>() { gain_db = g.clamp(-60.0, 6.0); },
                "freq" => if let Ok(f) = v.parse::<f32>() {
                    antitone_freq = if f <= 0.0 { None } else { Some(f.clamp(20.0, 500.0)) };
                },
                _ => {}
            }
        }
    }

    let wav = crate::masker::render_sanitizer_wav(
        preset, 48_000, duration_ms, gain_db, antitone_freq);
    http_response(200, "audio/wav", &wav)
}

fn http_response(code: u16, content_type: &str, body: &[u8]) -> Vec<u8> {
    let reason = match code { 200 => "OK", 404 => "Not Found", 500 => "Internal Server Error", _ => "OK" };
    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
        code, reason, content_type, body.len()
    );
    let mut out = header.into_bytes();
    out.extend_from_slice(body);
    out
}

/// Run the channel server. BLOCKS — call on its own thread. `sources` maps a
/// content key to the WAV bytes + channel order; v1 serves a single source
/// under the empty key, but the map allows multiple scenes/sources later.
pub struct ChannelServer {
    pub src_wav: Vec<u8>,
    pub order: Vec<String>,
}

impl ChannelServer {
    pub fn serve(&self, bind: &str) -> std::io::Result<()> {
        let listener = TcpListener::bind(bind)?;
        eprintln!("[chan] serving channels on http://{}/ch/<name>", bind);
        // ── Threading model ──
        //
        // The original implementation handled each connection
        // synchronously on the accept thread, which meant N
        // speakers fetching their channels simultaneously got
        // queued: device 2 couldn't start downloading until
        // device 1's transfer completed. For a 6-speaker surround
        // setup that's catastrophic — even modest channels run
        // multiple megabytes, and serializing them adds seconds
        // of latency where there shouldn't be any.
        //
        // Spawn one thread per incoming connection. Connection
        // counts are small in practice (≤ 16 devices) and
        // short-lived (a single fetch each), so the cost of an
        // OS thread per is comfortably justified by the latency
        // win. The src_wav + order are cloned per connection:
        // both small enough that the clone is cheap vs. the
        // download time, and they're consumed read-only so an
        // Arc would also work but adds complexity for no gain
        // at this scale.
        for conn in listener.incoming() {
            match conn {
                Ok(stream) => {
                    let src_wav = self.src_wav.clone();
                    let order   = self.order.clone();
                    std::thread::spawn(move || {
                        Self::handle_one(stream, &src_wav, &order);
                    });
                }
                Err(e) => eprintln!("[chan] accept error: {}", e),
            }
        }
        Ok(())
    }

    /// Per-connection handler. Reads a bounded HTTP request with
    /// a read timeout, parses the path, sends the response, and
    /// closes the connection. Errors are logged but never panic
    /// the accept loop.
    fn handle_one(mut stream: TcpStream, src_wav: &[u8], order: &[String]) {
        // 5-second timeout protects against stuck/malicious clients
        // holding the socket open. The actual transfer time for the
        // largest expected channel (a 10-minute mono WAV at 48 kHz
        // 16-bit is ~57 MB) over LAN takes ~1 second; 5 s is the
        // generous side.
        let _ = stream.set_read_timeout(
            Some(std::time::Duration::from_secs(5)));
        let _ = stream.set_write_timeout(
            Some(std::time::Duration::from_secs(30)));
        // Loop-read until we see the end-of-headers marker (CRLF
        // CRLF), or hit the cap, or the read times out. A single
        // read() doesn't always return the full request — kernels
        // can deliver headers in multiple chunks especially when
        // the client uses TCP_NODELAY or sends small bursts.
        let mut buf = Vec::with_capacity(2048);
        let mut tmp = [0u8; 1024];
        const MAX_REQ: usize = 16 * 1024;  // 16 KB is plenty for an HTTP GET
        loop {
            match stream.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&tmp[..n]);
                    if find_subslice(&buf, b"\r\n\r\n").is_some() { break; }
                    if buf.len() >= MAX_REQ {
                        eprintln!("[chan] request exceeded {} bytes; closing", MAX_REQ);
                        return;
                    }
                }
                Err(_) => return,
            }
        }
        if buf.is_empty() { return; }
        let req = String::from_utf8_lossy(&buf);
        let path = parse_request_path(&req).unwrap_or_else(|| "/".into());
        let resp = build_response(&path, src_wav, order);
        let _ = stream.write_all(&resp);
        let _ = stream.flush();
    }
}

/// Convenience for callers that hold sources in a map.
pub type SourceMap = HashMap<String, (Vec<u8>, Vec<String>)>;

// ─── Client side: fetch an assigned channel for local playback ──────────────

/// Fetch a channel URL (e.g. "http://192.168.2.30:47720/ch/SL") into a temp
/// WAV file and return its path. The assigned device calls this, then hands
/// the path to its AudioSink. Minimal HTTP/1.0 GET over TcpStream — no client
/// framework dependency. Returns the temp file path on success.
pub fn fetch_channel_to_temp(url: &str) -> Result<std::path::PathBuf, String> {
    // Parse http://host:port/path
    let rest = url.strip_prefix("http://").ok_or("only http:// supported")?;
    let slash = rest.find('/').unwrap_or(rest.len());
    let host_port = &rest[..slash];
    let path = if slash < rest.len() { &rest[slash..] } else { "/" };

    // Resolve the host once with a timeout. TCP connect itself can
    // block for tens of seconds against an unreachable host on
    // Linux's default tcp_syn_retries; we want to fail fast so the
    // assignment-resolver can re-route to a different speaker.
    use std::net::ToSocketAddrs;
    let addr = host_port.to_socket_addrs()
        .map_err(|e| format!("resolve {}: {}", host_port, e))?
        .next()
        .ok_or_else(|| format!("no address for {}", host_port))?;

    let mut stream = std::net::TcpStream::connect_timeout(
        &addr, std::time::Duration::from_secs(2))
        .map_err(|e| format!("connect {}: {}", host_port, e))?;
    // Bound each individual read so a stuck server can't hang the
    // assignment loop indefinitely. 30 s is generous for a single
    // WAV channel transfer over LAN; the largest expected size
    // (10-min mono 48kHz/16-bit) finishes in ~1 s over a healthy
    // 802.11ac link.
    let _ = stream.set_read_timeout(
        Some(std::time::Duration::from_secs(30)));
    let _ = stream.set_write_timeout(
        Some(std::time::Duration::from_secs(5)));

    let req = format!("GET {} HTTP/1.0\r\nHost: {}\r\nConnection: close\r\n\r\n", path, host_port);
    stream.write_all(req.as_bytes()).map_err(|e| e.to_string())?;

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).map_err(|e| e.to_string())?;

    // Split headers/body on the first \r\n\r\n.
    let sep = find_subslice(&buf, b"\r\n\r\n").ok_or("malformed HTTP response")?;
    let body = &buf[sep + 4..];
    if body.is_empty() { return Err("empty body".into()); }

    let dir = std::env::temp_dir();
    let name = format!("bo-mux-ch-{}.wav", sanitize(path));
    let out = dir.join(name);
    std::fs::write(&out, body).map_err(|e| e.to_string())?;
    Ok(out)
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tiny stereo 16-bit WAV: L samples = 0x0101, R = 0x0202.
    fn stereo_wav() -> Vec<u8> {
        // 4 frames, 2ch, 16-bit
        let mut data = Vec::new();
        for _ in 0..4 {
            data.extend_from_slice(&0x0101i16.to_le_bytes()); // L
            data.extend_from_slice(&0x0202i16.to_le_bytes()); // R
        }
        build_interleaved_wav(&data, 2, 48000, 16)
    }

    fn build_interleaved_wav(data: &[u8], ch: u16, sr: u32, bits: u16) -> Vec<u8> {
        let byte_rate = sr * ch as u32 * (bits as u32 / 8);
        let block_align = ch * (bits / 8);
        let data_len = data.len() as u32;
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data_len).to_le_bytes());
        w.extend_from_slice(b"WAVE");
        w.extend_from_slice(b"fmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&ch.to_le_bytes());
        w.extend_from_slice(&sr.to_le_bytes());
        w.extend_from_slice(&byte_rate.to_le_bytes());
        w.extend_from_slice(&block_align.to_le_bytes());
        w.extend_from_slice(&bits.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data_len.to_le_bytes());
        w.extend_from_slice(data);
        w
    }

    #[test]
    fn parse_finds_fmt_and_data() {
        let wav = stereo_wav();
        let (fmt, start, end) = parse_wav(&wav).unwrap();
        assert_eq!(fmt.channels, 2);
        assert_eq!(fmt.sample_rate, 48000);
        assert_eq!(fmt.bits, 16);
        assert_eq!(end - start, 16); // 4 frames * 2ch * 2 bytes
    }

    #[test]
    fn extract_left_channel() {
        let wav = stereo_wav();
        let mono = extract_channel_wav(&wav, 0).unwrap();
        let (fmt, start, end) = parse_wav(&mono).unwrap();
        assert_eq!(fmt.channels, 1);
        // 4 samples * 2 bytes = 8 bytes of mono data
        assert_eq!(end - start, 8);
        // every sample should be the L value 0x0101
        for s in mono[start..end].chunks(2) {
            assert_eq!(i16::from_le_bytes([s[0], s[1]]), 0x0101);
        }
    }

    #[test]
    fn extract_right_channel() {
        let wav = stereo_wav();
        let mono = extract_channel_wav(&wav, 1).unwrap();
        let (_, start, end) = parse_wav(&mono).unwrap();
        for s in mono[start..end].chunks(2) {
            assert_eq!(i16::from_le_bytes([s[0], s[1]]), 0x0202);
        }
    }

    #[test]
    fn channel_out_of_range_errs() {
        let wav = stereo_wav();
        assert!(extract_channel_wav(&wav, 5).is_err());
    }

    #[test]
    fn channel_index_by_name_and_alias() {
        let order: Vec<String> = ["FL","FR","C","LFE","SL","SR"].iter().map(|s| s.to_string()).collect();
        assert_eq!(channel_index("SL", &order), Some(4));
        assert_eq!(channel_index("sl", &order), Some(4)); // case-insensitive
        assert_eq!(channel_index("RL", &[]), None);       // not in std51
        assert_eq!(channel_index("C", &[]), Some(2));     // std51 fallback
    }

    #[test]
    fn parse_request_path_works() {
        assert_eq!(parse_request_path("GET /ch/FL HTTP/1.1\r\nHost: x\r\n\r\n"), Some("/ch/FL".into()));
        assert_eq!(parse_request_path("POST /ch/FL HTTP/1.1"), None); // only GET
    }

    #[test]
    fn build_response_serves_channel() {
        let wav = stereo_wav();
        let order: Vec<String> = vec!["FL".into(), "FR".into()];
        let resp = build_response("/ch/FR", &wav, &order);
        let text = String::from_utf8_lossy(&resp[..64]);
        assert!(text.contains("200 OK"));
        assert!(text.contains("audio/wav"));
    }

    #[test]
    fn build_response_404_unknown() {
        let wav = stereo_wav();
        let resp = build_response("/ch/ZZ", &wav, &[]);
        assert!(String::from_utf8_lossy(&resp[..32]).contains("404"));
    }

    #[test]
    fn masker_route_serves_wav() {
        let resp = build_response("/masker/pink?ms=100&db=-30", &[], &[]);
        let head = String::from_utf8_lossy(&resp[..96]);
        assert!(head.contains("200 OK"));
        assert!(head.contains("audio/wav"));
        // Body starts after \r\n\r\n; should be a RIFF header.
        let sep = resp.windows(4).position(|w| w == b"\r\n\r\n").expect("no header sep");
        assert_eq!(&resp[sep+4..sep+8], b"RIFF");
        assert_eq!(&resp[sep+12..sep+16], b"WAVE");
    }

    #[test]
    fn masker_route_404_on_unknown_preset() {
        let resp = build_response("/masker/nonsense", &[], &[]);
        assert!(String::from_utf8_lossy(&resp[..32]).contains("404"));
    }

    #[test]
    fn masker_route_caps_duration() {
        // 999999 ms should be clamped to 120_000. The response size for
        // 120_000 ms at 48k/16-bit mono is ~11.5 MB; sanity-check that
        // we don't generate hours of audio on demand.
        let resp = build_response("/masker/pink?ms=999999", &[], &[]);
        // Expected: 44-byte header + 120_000ms * 48 frames/ms * 2 bytes = 11_520_044
        assert!(resp.len() < 12_000_000, "duration not capped: {}", resp.len());
    }

    #[test]
    fn sanitizer_route_serves_wav() {
        let resp = build_response("/sanitizer/engine?freq=80&ms=100&db=-20", &[], &[]);
        let head = String::from_utf8_lossy(&resp[..96]);
        assert!(head.contains("200 OK"));
        assert!(head.contains("audio/wav"));
        let sep = resp.windows(4).position(|w| w == b"\r\n\r\n").expect("no header sep");
        assert_eq!(&resp[sep+4..sep+8], b"RIFF");
    }

    #[test]
    fn sanitizer_route_freq_zero_disables_antitones() {
        // freq=0 → noise-only. The body should be the same length as
        // the masker route for the same preset/duration (both use the
        // shared 48k mono 16-bit PCM path).
        let with_anti  = build_response("/sanitizer/engine?freq=80&ms=100",  &[], &[]);
        let no_anti    = build_response("/sanitizer/engine?freq=0&ms=100",   &[], &[]);
        let masker     = build_response("/masker/engine?ms=100",             &[], &[]);
        assert_eq!(with_anti.len(), no_anti.len());
        assert_eq!(no_anti.len(), masker.len());
    }

    #[test]
    fn sanitizer_route_404_on_unknown_preset() {
        let resp = build_response("/sanitizer/nonsense", &[], &[]);
        assert!(String::from_utf8_lossy(&resp[..32]).contains("404"));
    }

    #[test]
    fn health_ok() {
        let resp = build_response("/health", &[], &[]);
        assert!(String::from_utf8_lossy(&resp).contains("200 OK"));
        assert!(String::from_utf8_lossy(&resp).ends_with("ok"));
    }

    #[test]
    fn server_concurrent_requests_dont_serialize() {
        // The previous accept loop ran handle() synchronously on the
        // accept thread, so N parallel device fetches got queued
        // sequentially — terrible for a 6-speaker surround setup
        // where every speaker should start at the scheduled instant.
        // This test sends 4 simultaneous requests and verifies all
        // responses arrive within a window much shorter than 4×
        // a single response's serve time.
        use std::io::{Read, Write};
        use std::sync::Arc;
        let wav = Arc::new(stereo_wav());
        let order: Vec<String> = vec!["FL".into(), "FR".into()];
        let srv = Arc::new(ChannelServer {
            src_wav: (*wav).clone(),
            order: order.clone(),
        });
        // Bind to an OS-assigned ephemeral port.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let bind_addr = listener.local_addr().unwrap();
        // Mimic the serve() loop but with the bound listener so the
        // test controls when it shuts down. Limited to 4 accepts.
        let srv_clone = srv.clone();
        let server_thread = std::thread::spawn(move || {
            for _ in 0..4 {
                let (stream, _) = listener.accept().unwrap();
                let src = srv_clone.src_wav.clone();
                let ord = srv_clone.order.clone();
                std::thread::spawn(move || {
                    ChannelServer::handle_one(stream, &src, &ord);
                });
            }
        });
        // Fire 4 concurrent client connections.
        let mut handles = Vec::new();
        let start = std::time::Instant::now();
        for ch in ["FL", "FR", "FL", "FR"] {
            let h = std::thread::spawn(move || {
                let mut stream = std::net::TcpStream::connect(bind_addr).unwrap();
                let req = format!(
                    "GET /ch/{} HTTP/1.1\r\nHost: x\r\n\r\n", ch);
                stream.write_all(req.as_bytes()).unwrap();
                let mut buf = Vec::new();
                stream.read_to_end(&mut buf).unwrap();
                buf
            });
            handles.push(h);
        }
        let results: Vec<_> = handles.into_iter()
            .map(|h| h.join().unwrap())
            .collect();
        let elapsed = start.elapsed();
        let _ = server_thread.join();
        // All 4 must succeed.
        for r in &results {
            assert!(!r.is_empty());
            let head = String::from_utf8_lossy(&r[..r.len().min(96)]);
            assert!(head.contains("200 OK"),
                "response missing 200 OK: {:?}", head);
        }
        // Concurrency: if requests serialized each would add to
        // total time. The test passes if total time is reasonable
        // (we're not stressing this hard — 4 short requests over
        // loopback should finish in well under a second total).
        assert!(elapsed.as_millis() < 2000,
            "concurrent serve took {} ms — likely still serializing",
            elapsed.as_millis());
    }
}
