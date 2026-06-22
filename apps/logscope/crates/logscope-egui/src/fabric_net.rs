//! fabric_net.rs — the networked half of the LAN fabric: entry-point ranking (the scheduler),
//! a hand-rolled binary wire framing (the signaling), a tagged compression codec for file payloads,
//! and a `RemoteWorker` that speaks all three over a pluggable `Transport`. The real HTTPS transport
//! plugs in behind `Transport` on your boxes; everything here is validated against a `MockTransport`.
#![allow(dead_code)]
use crate::fabric::{GenRequest, GenResult, ModelInfo, Worker, WorkerId};

// ───────────────────────── entry-point ranking (the scheduler) ─────────────────────────
// "Optimal entry point at all times" = the smallest model that still meets the task's capability,
// on the least-loaded, lowest-latency box. Which dimension dominates is a per-system Policy — that's
// the "couple dimensions a system can rank its per-model entrypoints by."

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum SizeClass { Tiny, Small, Mid, Large, XL }
impl SizeClass {
    pub fn from_params(b: f32) -> SizeClass {
        if b < 0.5 { SizeClass::Tiny } else if b < 2.0 { SizeClass::Small }
        else if b < 5.0 { SizeClass::Mid } else if b < 10.0 { SizeClass::Large } else { SizeClass::XL }
    }
    pub fn rank(self) -> i32 { self as i32 }
}

/// A runtime view of one (box, model) the scheduler can route to.
#[derive(Clone, Debug)]
pub struct EntryPoint {
    pub worker: WorkerId,
    pub model: String,
    pub class: SizeClass,
    pub load: f32,          // 0=idle .. 1=saturated
    pub est_ms_per_tok: f32,
    pub public: bool,
}

/// How a system weighs the dimensions. `min_class` = the task's required capability floor.
#[derive(Clone, Copy)]
pub struct Policy { pub min_class: SizeClass, pub w_fit: f32, pub w_load: f32, pub w_latency: f32 }
impl Policy {
    /// Default "smallest sufficient, then idle, then fast."
    pub fn for_task(min_class: SizeClass) -> Self { Self { min_class, w_fit: 1.0, w_load: 0.6, w_latency: 0.4 } }
}

/// Score an entry point (higher = better). None if it can't serve the task (too small or private).
pub fn score(e: &EntryPoint, p: &Policy) -> Option<f32> {
    if !e.public || e.class < p.min_class { return None; }
    let overkill = (e.class.rank() - p.min_class.rank()) as f32;   // 0 = perfect fit
    let fit = 1.0 / (1.0 + overkill);                              // smallest-sufficient wins
    let load = 1.0 - e.load.clamp(0.0, 1.0);
    let latency = 1.0 / (1.0 + e.est_ms_per_tok / 50.0);          // ~50ms/tok normalizer
    Some(p.w_fit * fit + p.w_load * load + p.w_latency * latency)
}

/// Entry points ranked best-first under a policy (insufficient/private ones dropped).
pub fn rank_entries<'a>(entries: &'a [EntryPoint], p: &Policy) -> Vec<(&'a EntryPoint, f32)> {
    let mut scored: Vec<_> = entries.iter().filter_map(|e| score(e, p).map(|s| (e, s))).collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored
}
pub fn best_entry<'a>(entries: &'a [EntryPoint], p: &Policy) -> Option<&'a EntryPoint> {
    rank_entries(entries, p).into_iter().next().map(|(e, _)| e)
}

// ───────────────────────── compression codec (tagged) ─────────────────────────
// File payloads carry a 1-byte codec tag so the receiver knows how to decode. Raw + a real dep-free
// RLE ship now; Deflate(2)/Zstd are a one-function swap (flate2/zstd) once a dep is acceptable.

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Codec { Raw = 0, Rle = 1 } // Deflate = 2, Zstd = 3 in production

/// byte-run RLE: [count:u8][byte] pairs. Real and lossless; shrinks the long indent/whitespace runs
/// in source files. Production swaps Deflate in behind the same tag.
fn rle_compress(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        let mut run = 1usize;
        while i + run < data.len() && data[i + run] == b && run < 255 { run += 1; }
        out.push(run as u8); out.push(b);
        i += run;
    }
    out
}
fn rle_decompress(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < data.len() { let (n, b) = (data[i], data[i + 1]); out.extend(std::iter::repeat(b).take(n as usize)); i += 2; }
    out
}
pub fn encode_payload(codec: Codec, data: &[u8]) -> Vec<u8> {
    let body = match codec { Codec::Raw => data.to_vec(), Codec::Rle => rle_compress(data) };
    let mut out = vec![codec as u8];
    out.extend(body);
    out
}
pub fn decode_payload(framed: &[u8]) -> Vec<u8> {
    if framed.is_empty() { return Vec::new(); }
    match framed[0] { 1 => rle_decompress(&framed[1..]), _ => framed[1..].to_vec() }
}
/// Pick the codec that yields the smaller payload (cheap heuristic for the file-passing path).
pub fn best_codec(data: &[u8]) -> Codec {
    if rle_compress(data).len() < data.len() { Codec::Rle } else { Codec::Raw }
}

// ───────────────────────── hand-rolled wire framing (signaling) ─────────────────────────
// Length-prefixed binary, no serde. A request carries the prefix tokens + model/sampler + any files
// (each file's bytes go through the codec). This is the message a RemoteWorker sends.

fn put_u32(b: &mut Vec<u8>, v: u32) { b.extend_from_slice(&v.to_le_bytes()); }
fn put_u64(b: &mut Vec<u8>, v: u64) { b.extend_from_slice(&v.to_le_bytes()); }
fn put_f32(b: &mut Vec<u8>, v: f32) { b.extend_from_slice(&v.to_le_bytes()); }
fn put_bytes(b: &mut Vec<u8>, s: &[u8]) { put_u32(b, s.len() as u32); b.extend_from_slice(s); }
fn put_str(b: &mut Vec<u8>, s: &str) { put_bytes(b, s.as_bytes()); }

struct Rd<'a> { b: &'a [u8], i: usize }
impl<'a> Rd<'a> {
    fn u32(&mut self) -> u32 { let v = u32::from_le_bytes(self.b[self.i..self.i + 4].try_into().unwrap()); self.i += 4; v }
    fn u64(&mut self) -> u64 { let v = u64::from_le_bytes(self.b[self.i..self.i + 8].try_into().unwrap()); self.i += 8; v }
    fn f32(&mut self) -> f32 { let v = f32::from_le_bytes(self.b[self.i..self.i + 4].try_into().unwrap()); self.i += 4; v }
    fn bytes(&mut self) -> Vec<u8> { let n = self.u32() as usize; let v = self.b[self.i..self.i + n].to_vec(); self.i += n; v }
    fn str(&mut self) -> String { String::from_utf8_lossy(&self.bytes()).into_owned() }
}

pub fn encode_request(req: &GenRequest, files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut b = Vec::new();
    put_str(&mut b, &req.model);
    put_u32(&mut b, req.max_new as u32);
    put_f32(&mut b, req.temperature);
    put_u64(&mut b, req.seed);
    put_u32(&mut b, req.prefix.len() as u32);
    for &t in &req.prefix { put_u32(&mut b, t); }
    put_u32(&mut b, files.len() as u32);
    for (name, data) in files { put_str(&mut b, name); put_bytes(&mut b, &encode_payload(best_codec(data), data)); }
    b
}
pub fn decode_request(b: &[u8]) -> (GenRequest, Vec<(String, Vec<u8>)>) {
    let mut r = Rd { b, i: 0 };
    let model = r.str(); let max_new = r.u32() as usize; let temperature = r.f32(); let seed = r.u64();
    let n = r.u32() as usize; let prefix: Vec<u32> = (0..n).map(|_| r.u32()).collect();
    let nf = r.u32() as usize;
    let files = (0..nf).map(|_| { let name = r.str(); let data = decode_payload(&r.bytes()); (name, data) }).collect();
    (GenRequest { prefix, model, max_new, temperature, seed }, files)
}
pub fn encode_result(res: &GenResult) -> Vec<u8> {
    let mut b = Vec::new();
    put_str(&mut b, &res.worker.device); put_str(&mut b, &res.worker.author); put_str(&mut b, &res.model);
    put_u32(&mut b, res.tokens.len() as u32);
    for &t in &res.tokens { put_u32(&mut b, t); }
    b
}
pub fn decode_result(b: &[u8]) -> GenResult {
    let mut r = Rd { b, i: 0 };
    let (device, author, model) = (r.str(), r.str(), r.str());
    let n = r.u32() as usize; let tokens = (0..n).map(|_| r.u32()).collect();
    GenResult { tokens, worker: WorkerId::new(&device, &author), model }
}

// ───────────────────────── transport + RemoteWorker ─────────────────────────

/// Request bytes → response bytes. Real impl is an HTTPS POST to a box; here it's mocked.
pub trait Transport { fn send(&self, request: &[u8]) -> Vec<u8>; }

/// Loopback transport: decodes the request and runs it on a local Worker — proves the wire path
/// round-trips without a network.
pub struct MockTransport<W: Worker> { pub backend: W }
impl<W: Worker> Transport for MockTransport<W> {
    fn send(&self, request: &[u8]) -> Vec<u8> {
        let (req, _files) = decode_request(request);
        match self.backend.generate(&req) { Some(r) => encode_result(&r), None => Vec::new() }
    }
}

/// A box reached over a Transport. To the rest of the fabric it's just a `Worker`.
pub struct RemoteWorker<T: Transport> {
    pub id: WorkerId,
    pub models: Vec<ModelInfo>,
    pub transport: T,
}
impl<T: Transport> Worker for RemoteWorker<T> {
    fn id(&self) -> &WorkerId { &self.id }
    fn models(&self) -> &[ModelInfo] { &self.models }
    fn generate(&self, req: &GenRequest) -> Option<GenResult> {
        let wire = encode_request(req, &[]);
        let resp = self.transport.send(&wire);
        if resp.is_empty() { None } else { Some(decode_result(&resp)) }
    }
}

/// Self-test (no GPU/network): ranking, codec round-trip+shrink, wire round-trip with a file,
/// and RemoteWorker-over-MockTransport matching a local worker. `--gate-net` (run via a small bin).
pub fn gate_net() {
    use crate::fabric::MockWorker;
    // 1) scheduler: task needs >= Small; smallest-sufficient idle box should win, Tiny excluded
    let eps = vec![
        EntryPoint { worker: WorkerId::new("laptop", "d"), model: "smollm2".into(), class: SizeClass::Tiny, load: 0.0, est_ms_per_tok: 20.0, public: true },
        EntryPoint { worker: WorkerId::new("studio", "d"), model: "llama-1b".into(), class: SizeClass::Small, load: 0.1, est_ms_per_tok: 45.0, public: true },
        EntryPoint { worker: WorkerId::new("studio", "d"), model: "ds-8b".into(), class: SizeClass::Large, load: 0.9, est_ms_per_tok: 200.0, public: true },
    ];
    let pol = Policy::for_task(SizeClass::Small);
    let best = best_entry(&eps, &pol).unwrap();
    let tiny_excluded = score(&eps[0], &pol).is_none();
    let picks_small = best.class == SizeClass::Small;
    // under a load-dominant policy with two equal-fit boxes, the idle one wins
    let eps2 = vec![
        EntryPoint { worker: WorkerId::new("a", "d"), model: "m".into(), class: SizeClass::Small, load: 0.9, est_ms_per_tok: 40.0, public: true },
        EntryPoint { worker: WorkerId::new("b", "d"), model: "m".into(), class: SizeClass::Small, load: 0.0, est_ms_per_tok: 40.0, public: true },
    ];
    let load_aware = best_entry(&eps2, &Policy { min_class: SizeClass::Small, w_fit: 0.0, w_load: 1.0, w_latency: 0.0 }).unwrap().worker.device == "b";

    // 2) codec: a run-heavy payload (deep indentation blocks) round-trips AND shrinks under RLE.
    // Real source has fewer long runs — that's why production swaps Deflate behind the same tag.
    let file = [&b" ".repeat(120)[..], b"fn a(){}\n", &b" ".repeat(120)[..], b"fn b(){}\n"].concat().repeat(4);
    let enc = encode_payload(best_codec(&file), &file);
    let codec_ok = decode_payload(&enc) == file && enc.len() < file.len();

    // 3) wire: request with a file survives encode→decode
    let req = GenRequest { prefix: vec![10, 20, 30], model: "llama-1b".into(), max_new: 8, temperature: 0.8, seed: 7 };
    let wire = encode_request(&req, &[("world.rs".into(), file.clone())]);
    let (rq, files) = decode_request(&wire);
    let wire_ok = rq.prefix == req.prefix && rq.model == req.model && rq.seed == req.seed
        && files.len() == 1 && files[0].0 == "world.rs" && files[0].1 == file;

    // 4) RemoteWorker over MockTransport == the same local worker's output
    let backend = MockWorker::new("studio-m2", "dustin", &[("llama-1b", true, 1.0)]);
    let local = backend.generate(&req).unwrap();
    let remote = RemoteWorker { id: WorkerId::new("studio-m2", "dustin"),
        models: vec![ModelInfo { name: "llama-1b".into(), public: true, params_b: 1.0 }],
        transport: MockTransport { backend: MockWorker::new("studio-m2", "dustin", &[("llama-1b", true, 1.0)]) } };
    let rgen = remote.generate(&req).unwrap();
    let remote_ok = rgen.tokens == local.tokens && rgen.worker == local.worker && rgen.model == local.model;

    for (label, ok) in [("Tiny excluded (< task floor)", tiny_excluded), ("picks smallest-sufficient", picks_small),
        ("load-aware policy picks idle box", load_aware), ("codec round-trips & shrinks", codec_ok),
        ("wire request+file round-trips", wire_ok), ("RemoteWorker == LocalWorker", remote_ok)] {
        println!("  [{}] {}", if ok { "\u{2713}" } else { "\u{2717} FAIL" }, label);
    }
    let all = tiny_excluded && picks_small && load_aware && codec_ok && wire_ok && remote_ok;
    println!("gate-net: file ratio {}→{} bytes | {}", file.len(), enc.len(),
        if all { "fabric net checks \u{2713}" } else { "FAIL \u{2717}" });
}

// ───────────────────────── HTTPS transport (real framing, mock-validated) ─────────────────────────
// The binary fabric frame rides as the body of an HTTP/1.1 POST over any Read+Write stream — a rustls
// TlsStream on a box, a mock here. Only the TLS connect needs a dep + your LAN; the framing is tested.
use std::io::{Read, Write};

pub fn http_post<S: Read + Write>(s: &mut S, host: &str, path: &str, body: &[u8]) -> std::io::Result<Vec<u8>> {
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len());
    s.write_all(head.as_bytes())?;
    s.write_all(body)?;
    s.flush()?;
    let mut resp = Vec::new();
    s.read_to_end(&mut resp)?;
    let body_at = resp.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4).unwrap_or(0);
    Ok(resp[body_at..].to_vec())
}

pub trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}

/// Opens a stream to a box. Real impl wraps a TcpStream in rustls — feature-gated on your boxes, so
/// the TLS dep never reaches the rest of the codebase. The fabric only knows this trait.
pub trait Connect { fn open(&self) -> std::io::Result<Box<dyn ReadWrite>>; }

pub struct HttpTransport<C: Connect> { pub host: String, pub path: String, pub connect: C }
impl<C: Connect> Transport for HttpTransport<C> {
    fn send(&self, request: &[u8]) -> Vec<u8> {
        match self.connect.open() {
            Ok(mut s) => http_post(&mut s, &self.host, &self.path, request).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }
}

// mock stream + connector: captures the written request, replays a canned HTTP response
struct MockStream { resp: std::io::Cursor<Vec<u8>>, written: Vec<u8> }
impl Read for MockStream { fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> { self.resp.read(b) } }
impl Write for MockStream {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> { self.written.extend_from_slice(b); Ok(b.len()) }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}
struct MockConnect { resp: Vec<u8> }
impl Connect for MockConnect {
    fn open(&self) -> std::io::Result<Box<dyn ReadWrite>> { Ok(Box::new(MockStream { resp: std::io::Cursor::new(self.resp.clone()), written: Vec::new() })) }
}

pub fn gate_http() {
    let res = GenResult { tokens: vec![5, 6, 7], worker: WorkerId::new("studio-m2", "dustin"), model: "llama-1b".into() };
    let mut canned = b"HTTP/1.1 200 OK\r\nContent-Length: 99\r\nConnection: close\r\n\r\n".to_vec();
    canned.extend(encode_result(&res));

    // HttpTransport over the mock connector returns the canned result, decoded
    let t = HttpTransport { host: "box.lan".into(), path: "/fabric/gen".into(), connect: MockConnect { resp: canned.clone() } };
    let req = GenRequest { prefix: vec![1, 2], model: "llama-1b".into(), max_new: 4, temperature: 0.8, seed: 1 };
    let got = decode_result(&t.send(&encode_request(&req, &[])));
    let roundtrip = got.tokens == res.tokens && got.worker == res.worker && got.model == res.model;

    // the POST it writes is well-formed (inspect a directly-owned mock)
    let mut ms = MockStream { resp: std::io::Cursor::new(canned), written: Vec::new() };
    let _ = http_post(&mut ms, "box.lan", "/fabric/gen", b"PAYLOAD7");
    let head = String::from_utf8_lossy(&ms.written);
    let framed = head.starts_with("POST /fabric/gen HTTP/1.1") && head.contains("Host: box.lan")
        && head.contains("Content-Length: 8") && ms.written.ends_with(b"PAYLOAD7");

    for (label, ok) in [("POST framing well-formed", framed), ("HttpTransport decodes canned result", roundtrip)] {
        println!("  [{}] {}", if ok { "\u{2713}" } else { "\u{2717} FAIL" }, label);
    }
    println!("gate-http: {}", if framed && roundtrip { "transport framing checks \u{2713} (TLS connect = your boxes)" } else { "FAIL \u{2717}" });
}
