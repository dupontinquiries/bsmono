//! terrain_llm.rs — an HTTP endpoint that lets an LLM (or any tool) propose
//! PARAMETRIC terrain modifications at runtime.
//!
//! This is the runtime twin of `worldpack.rs`: one schema, one validator, one
//! policy. A model is pointed at `GET /schema`, writes a world-pack JSON
//! document, and `POST /terrain` validates it and stages it as the *pending
//! pack* for the target lobby. Nothing here ever accepts terrain data — only
//! generation parameters and procedural field brushes — so every peer in the
//! lobby regenerates identical ground from `(seed, pack)`. Fixed maps are
//! rejected with [`worldpack::NO_FIXED_MAPS_POLICY`], including for server
//! sub-lobbies: a sub-lobby is just a `(seed, pack)` pair, never a saved map.
//!
//! Deliberately framework-free (std `TcpListener`, minimal HTTP/1.1), in the
//! same spirit as the codebase's other hand-rolled plumbing — the surface is
//! three routes and JSON bodies:
//!
//!   GET  /schema           → the machine-readable op schema + policy text
//!   GET  /packs            → currently staged packs, keyed by lobby
//!   POST /terrain?lobby=L  → body: world-pack JSON. 200 with a report on
//!                            success; 422 with per-item problems otherwise.
//!
//! Application is intentionally decoupled: this module only STAGES validated
//! packs (`PackStore`). The sim thread drains the store at a safe boundary
//! (region regeneration / lobby creation) via `take_pending`, then feeds
//! `worldpack::resolve_brushes` into the field interaction layer and the
//! biome registry — the same paths elemental items and `assets/biomes`
//! already exercise. That keeps the network thread out of world state and
//! keeps determinism decisions (WHEN a pack lands) with the authority code.

use super::worldpack::{self, WorldPack};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

/// Staged, validated packs awaiting application, keyed by lobby id.
#[derive(Default)]
pub struct PackStore {
    pending: Mutex<HashMap<String, WorldPack>>,
}

impl PackStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Stage a validated pack for a lobby (replacing any prior pending one).
    pub fn stage(&self, lobby: &str, pack: WorldPack) {
        self.pending.lock().unwrap().insert(lobby.to_string(), pack);
    }

    /// The sim thread claims a lobby's pending pack at a safe boundary.
    pub fn take_pending(&self, lobby: &str) -> Option<WorldPack> {
        self.pending.lock().unwrap().remove(lobby)
    }

    pub fn staged_ids(&self) -> Vec<(String, String)> {
        self.pending
            .lock()
            .unwrap()
            .iter()
            .map(|(l, p)| (l.clone(), p.id.clone()))
            .collect()
    }
}

/// The schema document served to tooling. Kept as a function (not a static
/// string) so it can never drift from the validator's real limits.
pub fn schema_json() -> String {
    format!(
        r#"{{
 "endpoint": "POST /terrain?lobby=<id>",
 "document": "a world pack — parameters + procedural directives, never terrain data",
 "policy": "{policy}",
 "fields": {fields:?},
 "shape": {{
   "id": "string (required)",
   "name": "string",
   "blurb": "string",
   "climate": {{"temperature_offset": "f32", "humidity_offset": "f32",
               "sea_level_delta": "i32 in -64..=64"}},
   "biomes": {{"<namespace:id>": "Minecraft-format biome JSON (see assets/biomes)"}},
   "field_mods": [{{
      "field": "one of the fields above",
      "at": "origin | spawn | scatter_ring",
      "r_min/r_max/count/salt": "scatter_ring only; count 1..=4096",
      "radius": "f32 cells, 0.5..=64",
      "peak": "signed f32; positive injects, negative drains",
      "falloff": "constant | linear | smooth (default)"
   }}]
 }}
}}"#,
        policy = worldpack::NO_FIXED_MAPS_POLICY,
        fields = worldpack::FIELDS,
    )
}

/// Handle one already-parsed request. Split from socket I/O so it is fully
/// unit-testable without a listener. Returns (status, body).
pub fn handle(store: &PackStore, method: &str, path: &str, body: &str) -> (u16, String) {
    let (route, query) = match path.split_once('?') {
        Some((r, q)) => (r, q),
        None => (path, ""),
    };
    match (method, route) {
        ("GET", "/schema") => (200, schema_json()),
        ("GET", "/packs") => {
            let items: Vec<String> = store
                .staged_ids()
                .into_iter()
                .map(|(l, p)| format!(r#"{{"lobby":"{l}","pack":"{p}"}}"#))
                .collect();
            (200, format!("[{}]", items.join(",")))
        }
        ("POST", "/terrain") => {
            let lobby = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("lobby="))
                .unwrap_or("default");
            match worldpack::parse(body) {
                Err(e) => (400, format!(r#"{{"error":{:?}}}"#, e)),
                Ok((_, probs)) if !probs.is_empty() => {
                    let items: Vec<String> =
                        probs.iter().map(|p| format!("{p:?}")).collect();
                    (422, format!(r#"{{"rejected":true,"problems":[{}]}}"#,
                                  items.join(",")))
                }
                Ok((pack, _)) => {
                    let n_mods = pack.field_mods.len();
                    let n_biomes = pack.biomes.len();
                    let id = pack.id.clone();
                    store.stage(lobby, pack);
                    (200, format!(
                        r#"{{"staged":true,"lobby":{lobby:?},"pack":{id:?},"field_mods":{n_mods},"biomes":{n_biomes},"applies":"at the lobby's next generation boundary"}}"#))
                }
            }
        }
        _ => (404, r#"{"error":"routes: GET /schema, GET /packs, POST /terrain?lobby=L"}"#.into()),
    }
}

/// Serve forever on `addr` (e.g. "127.0.0.1:7791"). One thread, blocking —
/// meant to be spawned by the host alongside the sim:
/// `std::thread::spawn(move || terrain_llm::serve(store, "127.0.0.1:7791"))`.
pub fn serve(store: Arc<PackStore>, addr: &str) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    eprintln!("[terrain_llm] listening on {addr} — GET /schema for the op format");
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let store = store.clone();
        std::thread::spawn(move || {
            let _ = serve_one(&store, stream);
        });
    }
    Ok(())
}

fn serve_one(store: &PackStore, mut stream: TcpStream) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    // read until end of headers, then honour Content-Length (bounded)
    let header_end = loop {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(p) = find_header_end(&buf) {
            break p;
        }
        if buf.len() > 64 * 1024 {
            return Ok(());
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let req = lines.next().unwrap_or("");
    let mut parts = req.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();
    let clen: usize = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse().ok())
        .unwrap_or(0)
        .min(1 << 20);   // 1 MiB cap — packs are small by nature
    let mut body = buf[header_end..].to_vec();
    while body.len() < clen {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    let (status, out) = handle(store, &method, &path,
                               &String::from_utf8_lossy(&body[..clen.min(body.len())]));
    let reason = match status { 200 => "OK", 400 => "Bad Request",
                                422 => "Unprocessable Entity", _ => "Not Found" };
    write!(stream,
           "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n\
            Content-Length: {}\r\nConnection: close\r\n\r\n{out}",
           out.len())?;
    Ok(())
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{"id":"llm_probe","field_mods":[
        {"field":"temperature","at":"origin","radius":10.0,"peak":2.0}]}"#;

    #[test]
    fn schema_route_serves_policy_and_fields() {
        let s = PackStore::new();
        let (code, body) = handle(&s, "GET", "/schema", "");
        assert_eq!(code, 200);
        assert!(body.contains("parametric by design"));
        assert!(body.contains("temperature"));
    }

    #[test]
    fn valid_pack_is_staged_per_lobby_and_claimable_once() {
        let s = PackStore::new();
        let (code, body) = handle(&s, "POST", "/terrain?lobby=arena2", GOOD);
        assert_eq!(code, 200, "{body}");
        assert!(body.contains(r#""staged":true"#));
        assert_eq!(s.staged_ids(), vec![("arena2".into(), "llm_probe".into())]);
        let p = s.take_pending("arena2").expect("pending pack");
        assert_eq!(p.id, "llm_probe");
        assert!(s.take_pending("arena2").is_none(), "claim is once");
    }

    #[test]
    fn fixed_map_smuggling_is_rejected_422() {
        let s = PackStore::new();
        let evil = r#"{"id":"evil","biomes":{"e:flat":{"_id":"e:flat",
                       "heightmap":[[0]]}}}"#;
        let (code, body) = handle(&s, "POST", "/terrain?lobby=sub1", evil);
        assert_eq!(code, 422);
        assert!(body.contains("parametric by design"), "{body}");
        assert!(s.staged_ids().is_empty(), "rejected packs must not stage");
    }

    #[test]
    fn malformed_json_is_400_and_unknown_routes_404() {
        let s = PackStore::new();
        assert_eq!(handle(&s, "POST", "/terrain", "{nope").0, 400);
        assert_eq!(handle(&s, "GET", "/apply", "").0, 404);
    }

    #[test]
    fn end_to_end_over_a_real_socket() {
        let store = PackStore::new();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let st = store.clone();
        std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                let _ = serve_one(&st, stream);
            }
        });
        let mut c = TcpStream::connect(addr).unwrap();
        write!(c, "POST /terrain?lobby=e2e HTTP/1.1\r\nHost: x\r\n\
                   Content-Length: {}\r\n\r\n{}", GOOD.len(), GOOD).unwrap();
        let mut resp = String::new();
        c.read_to_string(&mut resp).unwrap();
        assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
        assert!(store.take_pending("e2e").is_some());
    }
}
