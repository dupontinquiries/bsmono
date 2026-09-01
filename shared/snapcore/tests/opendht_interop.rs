//! Two programs agreeing.
//!
//! Every other test of the OpenDHT path checks that our encoder and our
//! decoder agree with each other, or with bytes hand-assembled from reading
//! the reference's packer. This one puts `dhtnode` — the reference's own
//! binary, built from `vendor/opendht` — on the far end of the socket and
//! asks it. `maintenance/DHT_CONVERGENCE.md` called this "the one test that
//! cannot be faked", and it went a year without being run.
//!
//! ## Running it
//!
//! It is `#[ignore]` because it needs a binary the sandbox has to build
//! (`vendor/opendht`, see `maintenance/OPENDHT_INTEROP.md` for the four
//! commands), and a test that quietly passes when the binary is missing is
//! the fixture-gated trap CLAUDE.md §5b describes. So it FAILS if `dhtnode`
//! is not on `PATH` or in `DHTNODE`, and is opted into:
//!
//! ```sh
//! DHTNODE=/path/to/dhtnode cargo test -p snapcore --test opendht_interop -- --ignored
//! ```
//!
//! ## What each step proves
//!
//! 1. `ping` answered — our query envelope parses at the reference and its
//!    `pong` parses here. Before the codec was fixed this step failed
//!    silently: the reference threw on our integer token and sent nothing.
//! 2. The responder's real id replaces the guessed one — `contacts_of`.
//! 3. `find` against node A lists node B, which A learned by bootstrap —
//!    the `n4` blob is decoded correctly.
//! 4. `get` hands us a token; `put` with it is STORED, and the reference's
//!    own `g` command reads the bytes back — the value nesting is right and
//!    the token round-trips as a blob.
//! 5. A value the reference wrote (`p`, with its `user_type` and whatever
//!    else that version packs) decodes here with the right bytes.
//! 6. `sa` taught us our own address.

use snapcore::node::{FindValue, Transport};
use snapcore::routing::{Contact, NodeId};
use snapcore::wire::content_id;
use snapcore::wire_udp::WireUdpTransport;
use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const PORT_A: u16 = 24222;
const PORT_B: u16 = 24223;

struct RefNode {
    child: Child,
    stdin: std::process::ChildStdin,
    lines: mpsc::Receiver<String>,
    id: NodeId,
}

impl RefNode {
    fn spawn(port: u16, bootstrap: Option<u16>) -> RefNode {
        let bin = std::env::var("DHTNODE").unwrap_or_else(|_| "dhtnode".into());
        let mut cmd = Command::new(&bin);
        cmd.arg("-p").arg(port.to_string()).arg("-n").arg("0");
        if let Some(b) = bootstrap {
            cmd.arg("-b").arg(format!("127.0.0.1:{b}"));
        }
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("cannot run {bin}: {e}. Build vendor/opendht and set DHTNODE"));
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() { break; }
            }
        });
        let mut node = RefNode { child, stdin, lines: rx, id: NodeId([0; 20]) };
        // "OpenDHT node <hex> running on IPv4 port N"
        let banner = node.wait_for("OpenDHT node ", Duration::from_secs(10));
        let hex = banner.split_whitespace().nth(2).expect("id in banner");
        let mut id = [0u8; 20];
        for (i, b) in id.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap();
        }
        node.id = NodeId(id);
        node
    }

    fn send(&mut self, cmd: &str) {
        self.stdin.write_all(cmd.as_bytes()).unwrap();
        self.stdin.write_all(b"\n").unwrap();
        self.stdin.flush().unwrap();
    }

    /// Lines until one contains `pat`; that line is returned. Panics on
    /// timeout with everything seen, so a failure says what the reference
    /// actually printed.
    fn wait_for(&mut self, pat: &str, within: Duration) -> String {
        let deadline = Instant::now() + within;
        let mut seen = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(l) => {
                    if l.contains(pat) { return l; }
                    seen.push(l);
                }
                Err(_) => panic!("never saw {pat:?} from dhtnode; saw:\n{}", seen.join("\n")),
            }
        }
    }

    /// Everything printed within a window — for commands that answer with
    /// several lines and no terminator.
    fn drain(&mut self, within: Duration) -> Vec<String> {
        let deadline = Instant::now() + within;
        let mut out = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(l) => out.push(l),
                Err(_) => return out,
            }
        }
    }
}

impl Drop for RefNode {
    fn drop(&mut self) {
        let _ = self.send("x");
        let _ = self.child.wait();
    }
}

fn hex(id: &NodeId) -> String {
    id.0.iter().map(|b| format!("{b:02x}")).collect()
}

fn me() -> NodeId {
    let mut b = [0x5Cu8; 20];
    b[19] = 1;
    NodeId(b)
}

#[test]
#[ignore = "needs dhtnode (vendor/opendht) on PATH or in DHTNODE; run with --ignored"]
fn our_transport_interoperates_with_the_reference_node() {
    let mut a = RefNode::spawn(PORT_A, None);
    let mut b = RefNode::spawn(PORT_B, Some(PORT_A));
    // B bootstraps by querying A. A lists a node in its replies only once
    // it has CONFIRMED it — pinged it and been answered — so wait until A
    // itself says it has a good node rather than guessing a delay.
    let started = Instant::now();
    loop {
        a.send("ll");
        let out = a.drain(Duration::from_millis(400)).join("\n");
        if out.contains("Known nodes: 1 good") || out.contains("Known nodes: 2 good") { break; }
        assert!(started.elapsed() < Duration::from_secs(20), "A never confirmed B:\n{out}");
    }

    let t = WireUdpTransport::bind(me(), "127.0.0.1:0".parse().unwrap()).unwrap();
    let addr_a: SocketAddr = format!("127.0.0.1:{PORT_A}").parse().unwrap();
    let guessed = Contact::new(NodeId([0xEE; 20]), addr_a, 0);
    let self_c = Contact::new(me(), t.local_addr().unwrap(), 0);

    // 1. The reference answers us at all.
    assert!(t.ping(&self_c, &guessed), "the reference did not answer a ping: our envelope is not its envelope");

    // 6. And in answering, told us where we are.
    assert_eq!(t.public_ip(), Some("127.0.0.1".parse().unwrap()), "sa was not learned");

    // 2 + 3. A find returns A under its REAL id, and lists B.
    let found = t.find_node(&self_c, &guessed, &NodeId([0x11; 20]));
    assert!(!found.is_empty(), "find got no reply");
    assert_eq!(found[0].addr, addr_a);
    assert_eq!(found[0].id, a.id, "the responder's real id was not learned from the reply");
    let lists_b = found.iter().any(|c| c.id == b.id);
    assert!(lists_b, "A did not list B in n4; contacts: {found:?}");

    // 4. get → token → put, and the reference stores it.
    let key = NodeId([0x42; 20]);
    let a_real = Contact::new(a.id, addr_a, 0);
    match t.find_value(&self_c, &a_real, &key) {
        FindValue::Closest(cs) => assert!(!cs.is_empty(), "get before put should return closest nodes"),
        FindValue::Values(v) => panic!("a fresh key had values: {v:?}"),
    }
    let record = b"snapclone-interop-record".to_vec();
    t.store(&self_c, &a_real, key.clone(), record.clone(), 600);
    a.send(&format!("g {}", hex(&key)));
    let line = a.wait_for("Get: found 1 value", Duration::from_secs(5));
    let printed = a.drain(Duration::from_millis(300)).join("\n");
    assert!(printed.contains(&format!("id:{:x}", content_id(&record))),
            "the reference stored a different value id than we sent:\n{line}\n{printed}");

    // And our own get sees it back through the reference's encoder.
    match t.find_value(&self_c, &a_real, &key) {
        FindValue::Values(vs) => assert_eq!(vs, vec![record.clone()]),
        other => panic!("our put did not come back from the reference: {other:?}"),
    }

    // 5. A value the reference wrote decodes here.
    let key2 = NodeId([0x43; 20]);
    a.send(&format!("p {} written-by-the-reference", hex(&key2)));
    a.wait_for("Put:", Duration::from_secs(5));
    match t.find_value(&self_c, &a_real, &key2) {
        FindValue::Values(vs) => assert_eq!(vs, vec![b"written-by-the-reference".to_vec()]),
        other => panic!("the reference's value did not decode: {other:?}"),
    }

    // A put through the FULL lookup path — what presence::publish does —
    // reaches both nodes, since both are closest to anything.
    let mut node = snapcore::node::Node::new(me(), t.local_addr().unwrap());
    node.rt.saw(a.id, addr_a, 1);
    let key3 = NodeId([0x44; 20]);
    let closest = snapcore::lookup::lookup_nodes(&self_c, &mut node.rt, &key3, &t, 2, 1);
    assert!(closest.len() >= 2, "lookup did not walk from A to B: {closest:?}");
    for c in &closest {
        // The token for each comes from asking it first.
        let _ = t.find_value(&self_c, c, &key3);
        t.store(&self_c, c, key3.clone(), b"via-lookup".to_vec(), 600);
    }
    b.send(&format!("g {}", hex(&key3)));
    b.wait_for("Get: found", Duration::from_secs(5));
}
