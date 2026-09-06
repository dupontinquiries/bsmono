//! nat-probe — take the measurement the phone→cluster flow has been waiting on.
//!
//! `docs/phone-cluster-flow.md`: *"which pair your phone and Ultra form is a measurement
//! nobody has taken. Stage 5 answers it in one run."* This is that run. Two STUN Binding
//! requests from one socket, `reflexive::classify` on the answers, and `natpath`'s verdict
//! printed — punch or relay, and why.
//!
//! Run it on the phone and on each compute box. The classification is per-machine and the
//! decision is per-PAIR, so one run tells you half of the answer for every link.
//!
//! ```text
//!   nat-probe                      the two default servers
//!   nat-probe stun.example:3478 other.example:3478
//! ```
//!
//! Exit status is 0 when a classification was reached, 1 when it was not — so a script can
//! collect these from several machines without reading the prose.

use logscope_fan::natpath::Mapping;
use logscope_fan::{reflexive, stunwire};
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("nat-probe [server:port server:port]   — classify this machine's NAT");
        println!("nat-probe --selftest                  — prove the probe works, no network needed");
        return;
    }
    // A self-test, because the failure mode of this tool is indistinguishable from the
    // thing it measures: "no answer" looks the same whether the servers are unreachable,
    // UDP is blocked, or the binary is broken. Two STUN servers in this process on
    // loopback answer that — if the self-test classifies and a real run does not, the
    // network is the problem, not the code.
    if args.iter().any(|a| a == "--selftest") {
        selftest();
        return;
    }
    let servers: Vec<&str> = if args.is_empty() {
        stunwire::DEFAULT_SERVERS.to_vec()
    } else {
        args.iter().map(String::as_str).collect()
    };
    if servers.len() < 2 {
        eprintln!(
            "one server cannot classify anything — a single reply looks like success and \
             cannot tell endpoint-independent from symmetric. Give two, ideally at different \
             operators."
        );
        std::process::exit(1);
    }

    let probes = match stunwire::observe(&servers, Duration::from_secs(5)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("could not open a socket: {e}");
            std::process::exit(1);
        }
    };

    let mut obs = Vec::new();
    for p in &probes {
        match &p.result {
            Ok(r) => {
                let trust = match r.source {
                    stunwire::AddrSource::Xor => "",
                    // Worth saying out loud: a NAT that rewrites addresses inside packet
                    // bodies is exactly what this is measuring, so an unXORed answer may
                    // have been rewritten by the thing under test.
                    stunwire::AddrSource::Mapped => "  (MAPPED-ADDRESS, not XOR — treat with suspicion)",
                };
                println!("{:<28} sees us as {}:{}{}", p.server, r.addr, r.port, trust);
                obs.push(reflexive::Observation {
                    from: p.server.clone(),
                    addr: r.addr.clone(),
                    port: r.port,
                });
            }
            Err(e) => println!("{:<28} no answer: {e}", p.server),
        }
    }

    if obs.len() < 2 {
        println!(
            "\n{} of {} servers answered — not enough to classify. \
             `reflexive::classify` returns Unknown rather than guessing, because a guess \
             propagates into natpath and the failure then looks like a flaky connection.",
            obs.len(),
            servers.len()
        );
        std::process::exit(1);
    }

    // Local addresses are needed to spot the no-NAT case: the world seeing an address we
    // hold ourselves means there is nothing in the way.
    let locals = local_addrs();
    if !locals.is_empty() {
        println!("local: {}", locals.join(", "));
    }
    let mapping = reflexive::classify(&obs, &locals);
    let mapping = match obs.first() {
        Some(o) => reflexive::refine(mapping, &o.addr),
        None => mapping,
    };
    println!("\nthis machine: {}", describe(mapping));
    println!("\nfor a link to another machine, pair this with that machine's result:");
    for other in [Mapping::Open, Mapping::EndpointIndependent, Mapping::Symmetric, Mapping::Cgnat { symmetric: false }, Mapping::Cgnat { symmetric: true }] {
        let s = logscope_fan::natpath::strategy(mapping, other);
        println!("  vs {:<34} → {:?}{}", describe(other), s, if s.needs_relay() { "   ← a relay is required" } else { "" });
    }
}

/// Two loopback STUN servers and a probe against them.
fn selftest() {
    let mut addrs = Vec::new();
    let mut threads = Vec::new();
    for _ in 0..2 {
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind a local server");
        addrs.push(sock.local_addr().unwrap().to_string());
        threads.push(std::thread::spawn(move || {
            let mut buf = [0u8; 1500];
            if let Ok((n, from)) = sock.recv_from(&mut buf) {
                if n >= 20 {
                    let mut tx = [0u8; 12];
                    tx.copy_from_slice(&buf[8..20]);
                    let _ = sock.send_to(&xor_mapped(&tx, from), from);
                }
            }
        }));
    }
    println!("self-test: two STUN servers in this process, on {} and {}", addrs[0], addrs[1]);
    let refs: Vec<&str> = addrs.iter().map(String::as_str).collect();
    let probes = stunwire::observe(&refs, Duration::from_secs(2)).expect("socket");
    let mut obs = Vec::new();
    for p in &probes {
        match &p.result {
            Ok(r) => {
                println!("  {} sees us as {}:{}", p.server, r.addr, r.port);
                obs.push(reflexive::Observation { from: p.server.clone(), addr: r.addr.clone(), port: r.port });
            }
            Err(e) => println!("  {} no answer: {e}", p.server),
        }
    }
    for t in threads {
        let _ = t.join();
    }
    let m = reflexive::classify(&obs, &local_addrs());
    println!("classified as: {}", describe(m));
    // Over loopback there is nothing in the way, so both servers see the same port: the
    // right answer is endpoint-independent, and anything else means the parse or the
    // socket is wrong rather than the network being interesting.
    if obs.len() == 2 && m == Mapping::EndpointIndependent {
        println!("self-test PASSED — the probe works; a real run that fails is the network.");
    } else {
        println!("self-test FAILED — the probe itself is broken.");
        std::process::exit(1);
    }
}

/// A Binding success carrying the source address, XOR-mapped. The server half of the
/// protocol, which is four lines because that is all STUN's core is.
fn xor_mapped(tx: &[u8; 12], from: std::net::SocketAddr) -> Vec<u8> {
    let cookie = stunwire::MAGIC_COOKIE.to_be_bytes();
    let ip = match from.ip() {
        std::net::IpAddr::V4(v4) => v4.octets(),
        std::net::IpAddr::V6(_) => [0, 0, 0, 0],
    };
    let mut v = vec![0x00, 0x01];
    v.extend_from_slice(&(from.port() ^ (stunwire::MAGIC_COOKIE >> 16) as u16).to_be_bytes());
    for k in 0..4 {
        v.push(ip[k] ^ cookie[k]);
    }
    let mut msg = vec![0x01, 0x01];
    msg.extend_from_slice(&((4 + v.len()) as u16).to_be_bytes());
    msg.extend_from_slice(&cookie);
    msg.extend_from_slice(tx);
    msg.extend_from_slice(&0x0020u16.to_be_bytes());
    msg.extend_from_slice(&(v.len() as u16).to_be_bytes());
    msg.extend_from_slice(&v);
    msg
}

fn describe(m: Mapping) -> String {
    match m {
        Mapping::Open => "open (no NAT, or a forwarded port)".into(),
        Mapping::EndpointIndependent => "endpoint-independent (punchable)".into(),
        Mapping::Symmetric => "symmetric (punching cannot work)".into(),
        Mapping::Cgnat { symmetric: false } => "CGNAT, endpoint-independent".into(),
        Mapping::Cgnat { symmetric: true } => "CGNAT, symmetric".into(),
        Mapping::Unknown => "unknown".into(),
    }
}

/// This machine's own addresses, by asking the routing table where it would send a packet.
///
/// No `getifaddrs`: this crate is std-only by rule. A connected UDP socket sends nothing —
/// it only resolves a route — and its local address is the one the kernel would use, which
/// is the address that matters for the no-NAT comparison. Two destinations because a
/// machine with several interfaces has several answers.
fn local_addrs() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for dest in ["8.8.8.8:53", "1.1.1.1:53"] {
        if let Ok(s) = std::net::UdpSocket::bind("0.0.0.0:0") {
            if s.connect(dest).is_ok() {
                if let Ok(a) = s.local_addr() {
                    let ip = a.ip().to_string();
                    if !out.contains(&ip) {
                        out.push(ip);
                    }
                }
            }
        }
    }
    out
}
