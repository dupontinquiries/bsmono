//! net_skirmish.rs — slice 9: LAN skirmishes on the monorepo's net-kit.
//!
//! Two machines fight the same deterministic skirmish in LOCKSTEP: after a
//! token-authenticated handshake agrees the matchup, each peer runs the sim
//! locally and they exchange state hashes at checkpoints — the classic
//! desync detector every lockstep RTS carries. Nothing but the handshake
//! and 8-byte digests cross the wire.
//!
//! net-kit provides exactly the plumbing this needs and nothing more:
//!   auth::Keyring / frame / unframe — LAN shared-token auth with
//!       constant-time compare (wrong key → clean deny, no oracle)
//!   wire::Record — the percent-escaped tab-delimited line codec the rest
//!       of the fleet speaks (logscope enlist, voiceterm, unendlich)
//!
//! Protocol (framed, line-per-message):
//!   C→H  hello|<proto>|<fac>
//!   H→C  match|<facA>|<facB>|<max_secs>      (or deny|<reason>)
//!   both sync|<t>|<hash16>                   every 5 sim-seconds
//!   both result|<winner|->|<t>
//!
//! A hash mismatch aborts with Desync{t} — surfacing it beats silently
//! diverging, which is the entire point of lockstep verification.

use crate::data::Faction;
use crate::skirmish::SkirmishSim;
use net_kit::auth::{frame, unframe, Keyring};
use net_kit::wire::Record;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};

pub const PROTO: &str = "pb-lockstep-1";
pub const SYNC_EVERY: f64 = 5.0;

#[derive(Debug, PartialEq)]
pub enum NetOutcome {
    Finished { winner: Option<usize>, t: f64, syncs: usize },
    Desync { t: f64 },
    Denied(String),
}

fn send(stream: &mut TcpStream, key: &str, rec: &Record) -> std::io::Result<()> {
    let line = frame(key, &rec.to_line());
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")
}

fn recv(reader: &mut BufReader<TcpStream>, key: &str) -> Option<Record> {
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let line = line.trim_end();
    if line.is_empty() {
        return None;
    }
    Record::parse(&unframe(key, line)?)
}

/// Run the lockstep body once the handshake has agreed a matchup. Both peers
/// call this with identical (a, b) — the determinism does the rest.
fn lockstep(
    book: &BTreeMap<String, Faction>,
    a: &str,
    b: &str,
    max_secs: f64,
    stream: &mut TcpStream,
    reader: &mut BufReader<TcpStream>,
    key: &str,
) -> NetOutcome {
    let mut sim = SkirmishSim::new(book, a, b);
    let dt = 1.0 / 30.0;
    let mut next_sync = SYNC_EVERY;
    let mut syncs = 0;
    while sim.world.winner.is_none() && sim.world.t < max_secs {
        sim.step(dt);
        if sim.world.t >= next_sync {
            next_sync += SYNC_EVERY;
            let mine = format!("{:016x}", sim.state_hash());
            let t = format!("{:.2}", sim.world.t);
            if send(stream, key, &Record::new("sync", &[&t, &mine])).is_err() {
                return NetOutcome::Desync { t: sim.world.t };
            }
            let Some(theirs) = recv(reader, key) else {
                return NetOutcome::Desync { t: sim.world.t };
            };
            if theirs.tag != "sync" || theirs.fields.get(1).map(|s| s.as_str()) != Some(&mine) {
                return NetOutcome::Desync { t: sim.world.t };
            }
            syncs += 1;
        }
    }
    let w = sim
        .world
        .winner
        .map(|x| x.to_string())
        .unwrap_or_else(|| "-".into());
    let t = format!("{:.2}", sim.world.t);
    let _ = send(stream, key, &Record::new("result", &[&w, &t]));
    let theirs = recv(reader, key);
    match theirs {
        Some(r) if r.tag == "result" && r.fields.first() == Some(&w) => NetOutcome::Finished {
            winner: sim.world.winner,
            t: sim.world.t,
            syncs,
        },
        _ => NetOutcome::Desync { t: sim.world.t },
    }
}

/// Host a LAN skirmish: bind, accept one challenger, agree the matchup,
/// fight in lockstep. `fac` is the host's faction.
pub fn host(
    book: &BTreeMap<String, Faction>,
    listener: TcpListener,
    fac: &str,
    max_secs: f64,
    key: &str,
) -> NetOutcome {
    let ring = Keyring::new(vec![key.to_string()]);
    let (stream, _) = match listener.accept() {
        Ok(x) => x,
        Err(_) => return NetOutcome::Denied("accept failed".into()),
    };
    let mut stream = stream;
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
    // handshake: the hello must unframe under OUR key (constant-time inside)
    let mut raw = String::new();
    if reader.read_line(&mut raw).is_err() {
        return NetOutcome::Denied("no hello".into());
    }
    let hello = raw
        .trim_end()
        .split('\0')
        .next()
        .and_then(|l| unframe(key, l.trim_end()))
        .filter(|_| ring.accepts(key.as_bytes()))
        .and_then(|l| Record::parse(&l));
    let Some(h) = hello else {
        // deliberately frame the denial too — an unauthenticated peer
        // learns nothing but "no"
        let _ = send(&mut stream, key, &Record::new("deny", &["bad token or hello"]));
        return NetOutcome::Denied("bad token or hello".into());
    };
    if h.tag != "hello" || h.fields.first().map(|s| s.as_str()) != Some(PROTO) {
        let _ = send(&mut stream, key, &Record::new("deny", &["wrong proto"]));
        return NetOutcome::Denied("wrong proto".into());
    }
    let their_fac = h.fields.get(1).cloned().unwrap_or_default();
    if !book.contains_key(&their_fac) {
        let _ = send(&mut stream, key, &Record::new("deny", &["unknown faction"]));
        return NetOutcome::Denied("unknown faction".into());
    }
    let secs = format!("{max_secs}");
    if send(&mut stream, key, &Record::new("match", &[fac, &their_fac, &secs])).is_err() {
        return NetOutcome::Denied("send failed".into());
    }
    lockstep(book, fac, &their_fac, max_secs, &mut stream, &mut reader, key)
}

/// Join a hosted skirmish at `addr` with faction `fac`.
pub fn join(
    book: &BTreeMap<String, Faction>,
    addr: &str,
    fac: &str,
    key: &str,
) -> NetOutcome {
    let Ok(stream) = TcpStream::connect(addr) else {
        return NetOutcome::Denied(format!("connect {addr}"));
    };
    let mut stream = stream;
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
    if send(&mut stream, key, &Record::new("hello", &[PROTO, fac])).is_err() {
        return NetOutcome::Denied("hello send".into());
    }
    let Some(m) = recv(&mut reader, key) else {
        return NetOutcome::Denied("no reply (bad key?)".into());
    };
    if m.tag == "deny" {
        return NetOutcome::Denied(m.fields.first().cloned().unwrap_or_default());
    }
    if m.tag != "match" || m.fields.len() < 3 {
        return NetOutcome::Denied("bad match msg".into());
    }
    let (a, b) = (m.fields[0].clone(), m.fields[1].clone());
    let max_secs: f64 = m.fields[2].parse().unwrap_or(900.0);
    lockstep(book, &a, &b, max_secs, &mut stream, &mut reader, key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::load_factbook;
    use std::path::Path;

    fn book() -> BTreeMap<String, Faction> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        load_factbook(&dir).unwrap()
    }

    #[test]
    fn a_lan_match_stays_in_lockstep_to_the_same_crown() {
        let b = book();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let key = "fold-the-page";
        let bh = b.clone();
        let host_side =
            std::thread::spawn(move || host(&bh, listener, "swarm", 600.0, key));
        let client = join(&b, &addr, "rootbound", key);
        let hosted = host_side.join().unwrap();
        match (hosted, client) {
            (
                NetOutcome::Finished { winner: hw, t: ht, syncs: hs },
                NetOutcome::Finished { winner: cw, t: ct, syncs: cs },
            ) => {
                assert_eq!(hw, cw, "both peers must crown the same side");
                assert!((ht - ct).abs() < 1e-9);
                assert!(hs >= 3 && hs == cs, "several verified checkpoints: {hs}");
            }
            other => panic!("expected two Finished outcomes, got {other:?}"),
        }
    }

    #[test]
    fn a_wrong_key_is_denied_and_learns_nothing() {
        let b = book();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let bh = b.clone();
        let host_side =
            std::thread::spawn(move || host(&bh, listener, "court", 60.0, "right-key"));
        let client = join(&b, &addr, "ember", "wrong-key");
        let hosted = host_side.join().unwrap();
        assert!(matches!(client, NetOutcome::Denied(_)), "{client:?}");
        assert!(matches!(hosted, NetOutcome::Denied(_)), "{hosted:?}");
    }
}
