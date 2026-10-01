//! lan_dht — run the real snapcore DHT node over a real network and measure it.
//!
//! The kit's own tests put two nodes on 127.0.0.1. This puts them on different machines (laptop and
//! phone on one LAN) so what is measured is the engine, the OpenDHT wire format and the radio — not
//! loopback. std only; no flags library.
//!
//!   serve   lan_dht serve --bind 0.0.0.0:4300 --seed laptop [--secs 120]
//!   probe   lan_dht probe --bind 0.0.0.0:4301 --seed phone --peer 192.168.2.228:4300 --peer-seed laptop [--pings 20]
//!
//! Node ids are derived from the seed string, so a prober can name a peer it has never spoken to
//! (the wire corrects a guessed id from the first reply, but a right guess skips that).
//!
//! `probe` prints `RESULT key=value` lines and exits non-zero if the round trip FAILED: nothing
//! answered, the store was refused, or the value read back differs from what was stored.
use snapcore::node::{FindValue, Node, Transport};
use snapcore::routing::{Contact, NodeId};
use snapcore::wire_udp::WireUdpTransport;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn id_from(seed: &str) -> NodeId {
    // Deterministic 160 bits from a string: FNV-1a over the seed, expanded with the byte index.
    let mut out = [0u8; 20];
    for (i, o) in out.iter_mut().enumerate() {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ (i as u64).wrapping_mul(0x100_0000_01b3);
        for b in seed.bytes() { h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3); }
        *o = (h >> 32) as u8 ^ h as u8;
    }
    NodeId(out)
}

fn hex(id: &NodeId) -> String { id.0.iter().map(|b| format!("{b:02x}")).collect() }

fn arg(args: &[String], key: &str) -> Option<String> {
    args.iter().position(|a| a == key).and_then(|i| args.get(i + 1)).cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("");
    let bind: SocketAddr = arg(&args, "--bind").unwrap_or_else(|| "0.0.0.0:4300".into()).parse().expect("--bind ADDR:PORT");
    let seed = arg(&args, "--seed").unwrap_or_else(|| "node".into());
    let me = id_from(&seed);
    let t = Arc::new(WireUdpTransport::bind(me, bind).expect("bind"));
    let local = t.local_addr().expect("local addr");
    let node = Arc::new(Mutex::new(Node::new(me, local)));
    let handle = t.serve(node.clone()).expect("serve");
    eprintln!("lan_dht {mode}: seed={seed} id={} listening on {local}", &hex(&me)[..12]);

    match mode {
        "serve" => {
            let secs: u64 = arg(&args, "--secs").and_then(|s| s.parse().ok()).unwrap_or(120);
            let key = id_from("lan-test");
            let end = Instant::now() + Duration::from_secs(secs);
            while Instant::now() < end {
                std::thread::sleep(Duration::from_secs(2));
                let n = node.lock().unwrap();
                println!("serving: routing_table={} stored_values_for_lan-test={}", n.rt.len(), n.store.value_count(&key));
            }
            let n = node.lock().unwrap();
            println!("RESULT serve_done routing_table={} stored={}", n.rt.len(), n.store.value_count(&key));
        }
        "probe" => {
            let peer: SocketAddr = arg(&args, "--peer").expect("--peer ADDR:PORT").parse().expect("--peer ADDR:PORT");
            let peer_id = id_from(&arg(&args, "--peer-seed").expect("--peer-seed"));
            let pings: usize = arg(&args, "--pings").and_then(|s| s.parse().ok()).unwrap_or(20);
            let (from, to) = (Contact::new(me, local, 0), Contact::new(peer_id, peer, 0));
            let mut rtts = Vec::new();
            let mut lost = 0usize;
            for _ in 0..pings {
                let t0 = Instant::now();
                if t.ping(&from, &to) { rtts.push(t0.elapsed().as_secs_f64() * 1000.0); } else { lost += 1; }
                std::thread::sleep(Duration::from_millis(100));
            }
            rtts.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let pct = |p: f64| rtts.get(((rtts.len() as f64 - 1.0) * p).round() as usize).copied().unwrap_or(f64::NAN);
            println!("RESULT pings sent={pings} answered={} lost={lost}", rtts.len());
            println!("RESULT rtt_ms min={:.1} p50={:.1} p90={:.1} max={:.1}", pct(0.0), pct(0.5), pct(0.9), pct(1.0));
            let key = id_from("lan-test");
            let value = format!("hello from {seed} at {:?}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)).into_bytes();
            let t0 = Instant::now();
            t.store(&from, &to, key, value.clone(), 600);
            let store_ms = t0.elapsed().as_secs_f64() * 1000.0;
            let t0 = Instant::now();
            let got = t.find_value(&from, &to, &key);
            let find_ms = t0.elapsed().as_secs_f64() * 1000.0;
            let ok = match &got { FindValue::Values(v) => v.iter().any(|x| *x == value), _ => false };
            println!("RESULT store_ms={store_ms:.1} find_value_ms={find_ms:.1}");
            println!("RESULT roundtrip={}", if ok { "OK value read back equals value stored" } else { "FAILED" });
            if !ok { eprintln!("find_value returned {got:?}"); }
            t.stop_serving();
            drop(t);
            let _ = handle.join();
            std::process::exit(if rtts.is_empty() || !ok { 1 } else { 0 });
        }
        _ => { eprintln!("usage: lan_dht serve|probe --bind ADDR --seed S [--peer ADDR --peer-seed S] [--secs N] [--pings N]"); std::process::exit(2); }
    }
    t.stop_serving();
    drop(t);
    let _ = handle.join();
}
