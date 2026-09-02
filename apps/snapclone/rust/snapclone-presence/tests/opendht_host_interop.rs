//! The reference node, bootstrapping off one of ours.
//!
//! `shared/snapcore/tests/opendht_interop.rs` proves our transport can ASK
//! a real OpenDHT node. This proves a `NodeHost` can ANSWER one: `dhtnode`
//! is started with our host as its only bootstrap, and the reference's own
//! `ll` reports a good node — which it does only after its find was
//! answered in a form its parser and its checks accepted. Then a record
//! published through the app's raw path is read by the reference's `g`,
//! and a value the reference put lands in our store and comes back through
//! `resolve_raw`.
//!
//! `#[ignore]`: needs `dhtnode` (see maintenance/OPENDHT_INTEROP.md).
//!
//! ```sh
//! DHTNODE=/path/to/dhtnode cargo test -p snapclone-presence \
//!     --test opendht_host_interop -- --ignored
//! ```

use snapclone_presence::node_host::{Event, NodeHost};
use snapclone_presence::presence::key_for_scid;
use snapcore::routing::NodeId;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

struct RefNode {
    child: Child,
    stdin: std::process::ChildStdin,
    lines: mpsc::Receiver<String>,
}

impl RefNode {
    fn spawn(port: u16, bootstrap: std::net::SocketAddr) -> RefNode {
        let bin = std::env::var("DHTNODE").unwrap_or_else(|_| "dhtnode".into());
        let mut child = Command::new(&bin)
            .arg("-p").arg(port.to_string()).arg("-n").arg("0")
            .arg("-b").arg(bootstrap.to_string())
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
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
        let mut n = RefNode { child, stdin, lines: rx };
        n.wait_for("OpenDHT node ", Duration::from_secs(10));
        n
    }
    fn send(&mut self, cmd: &str) {
        self.stdin.write_all(cmd.as_bytes()).unwrap();
        self.stdin.write_all(b"\n").unwrap();
        self.stdin.flush().unwrap();
    }
    fn wait_for(&mut self, pat: &str, within: Duration) -> String {
        let deadline = Instant::now() + within;
        let mut seen = Vec::new();
        loop {
            match self.lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(l) => { if l.contains(pat) { return l; } seen.push(l); }
                Err(_) => panic!("never saw {pat:?}; saw:\n{}", seen.join("\n")),
            }
        }
    }
    fn drain(&mut self, within: Duration) -> String {
        let deadline = Instant::now() + within;
        let mut out = Vec::new();
        while let Ok(l) = self.lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            out.push(l);
        }
        out.join("\n")
    }
}

impl Drop for RefNode {
    fn drop(&mut self) {
        let _ = self.send("x");
        let _ = self.child.wait();
    }
}

fn hex(h: &[u8]) -> String { h.iter().map(|b| format!("{b:02x}")).collect() }

fn wait_event(h: &NodeHost, pred: impl Fn(&Event) -> bool, within: Duration) -> Event {
    let deadline = Instant::now() + within;
    loop {
        for e in h.poll() {
            if pred(&e) { return e; }
            if let Event::Stopped(why) = e { panic!("worker stopped: {why}"); }
        }
        assert!(Instant::now() < deadline, "no matching event within {within:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "needs dhtnode (vendor/opendht); run with --ignored"]
fn the_reference_bootstraps_off_our_host_and_exchanges_records() {
    let mut id = [0x5Au8; 20];
    id[19] = 9;
    let host = NodeHost::spawn(NodeId(id), "127.0.0.1:0".parse().unwrap()).unwrap();
    let mut r = RefNode::spawn(24240, host.local_addr());

    // The reference only counts a node as good once it answered a query
    // in a form the reference accepted.
    let started = Instant::now();
    loop {
        r.send("ll");
        let out = r.drain(Duration::from_millis(400));
        if out.contains("Known nodes: 1 good") { break; }
        assert!(started.elapsed() < Duration::from_secs(20), "the reference never confirmed us:\n{out}");
    }

    // We learned the reference from its own bootstrap queries — nobody
    // told our host about it.
    let payload = b"snapclone\ncarol\n10.0.0.3:6000:6001\n".to_vec();
    assert!(host.publish_raw("carol", 1, payload.clone()));
    match wait_event(&host, |e| matches!(e, Event::Published { sc_id, .. } if sc_id == "carol"), Duration::from_secs(20)) {
        Event::Published { peers, .. } => assert_eq!(peers, 1, "the reference should be our one peer"),
        _ => unreachable!(),
    }
    r.send(&format!("g {}", hex(&key_for_scid("carol").0)));
    r.wait_for("Get: found 1 value", Duration::from_secs(5));

    // A value the reference puts is stored by us and resolves here.
    r.send(&format!("p {} from-the-reference", hex(&key_for_scid("dave").0)));
    r.wait_for("Put: success", Duration::from_secs(10));
    assert!(host.resolve_raw("dave"));
    match wait_event(&host, |e| matches!(e, Event::ResolvedRaw { .. }), Duration::from_secs(20)) {
        Event::ResolvedRaw { values, .. } => {
            assert!(values.contains(&b"from-the-reference".to_vec()), "{values:?}");
        }
        _ => unreachable!(),
    }
    assert_eq!(host.public_ip(), Some("127.0.0.1".parse().unwrap()));
}
