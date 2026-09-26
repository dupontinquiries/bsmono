//! Two devices, nobody else.
//!
//! The bootstrap-independence claim through the app's own path: two
//! `NodeHost`s on loopback, one told the other's address, and that is all.
//! No reference node, no `bootstrap.jami.net`. A publishes, B resolves —
//! and B, which has never been told about anybody, learned A from A's own
//! queries. Then the reverse, through the raw payload path the frontend
//! uses.

use snapclone_presence::node_host::{Event, NodeHost};
use snapcore::routing::NodeId;
use std::time::{Duration, Instant};

fn id(n: u8) -> NodeId {
    let mut b = [n; 20];
    b[19] = 1;
    NodeId(b)
}

fn wait_for(h: &NodeHost, pred: impl Fn(&Event) -> bool, within: Duration) -> Event {
    let deadline = Instant::now() + within;
    loop {
        for e in h.poll() {
            if pred(&e) { return e; }
            if let Event::Stopped(why) = e { panic!("worker stopped: {why}"); }
        }
        assert!(Instant::now() < deadline, "nothing matched within {within:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_publishes_and_b_resolves_with_no_third_node() {
    let a = NodeHost::spawn(id(1), "127.0.0.1:0".parse().unwrap()).unwrap();
    let b = NodeHost::spawn(id(3), "127.0.0.1:0".parse().unwrap()).unwrap();

    // A knows B by address and a GUESSED id, as a bootstrap entry always
    // is; B knows nobody.
    assert!(a.bootstrap(b.local_addr(), NodeId([0xEE; 20])));
    wait_for(&a, |e| matches!(e, Event::Table { usable: 1 }), Duration::from_secs(5));

    assert!(a.publish("alice", "Alice", "10.0.0.1", 4444));
    match wait_for(&a, |e| matches!(e, Event::Published { sc_id, .. } if sc_id == "alice"), Duration::from_secs(20)) {
        Event::Published { peers, .. } => assert_eq!(peers, 1, "A should have told B"),
        _ => unreachable!(),
    }

    // B was never told about A, but A's queries taught it.
    assert!(b.resolve("alice"));
    match wait_for(&b, |e| matches!(e, Event::Resolved { .. }), Duration::from_secs(20)) {
        Event::Resolved { found, .. } => {
            assert_eq!(found.len(), 1, "{found:?}");
            assert_eq!(found[0].display_name, "Alice");
            assert_eq!(found[0].host, "10.0.0.1");
            assert_eq!(found[0].port, 4444);
        }
        _ => unreachable!(),
    }

    // And each learned its own address from the other's replies.
    assert_eq!(a.public_ip(), Some("127.0.0.1".parse().unwrap()));
}

#[test]
fn the_raw_path_works_the_same_way_in_reverse() {
    let a = NodeHost::spawn(id(5), "127.0.0.1:0".parse().unwrap()).unwrap();
    let b = NodeHost::spawn(id(7), "127.0.0.1:0".parse().unwrap()).unwrap();
    assert!(b.bootstrap(a.local_addr(), id(5)));
    wait_for(&b, |e| matches!(e, Event::Table { usable: 1 }), Duration::from_secs(5));

    let payload = b"snapclone\nbob\n10.0.0.2:5000:5001\n".to_vec();
    assert!(b.publish_raw("bob", 42, payload.clone()));
    wait_for(&b, |e| matches!(e, Event::Published { sc_id, .. } if sc_id == "bob"), Duration::from_secs(20));

    assert!(a.resolve_raw("bob"));
    match wait_for(&a, |e| matches!(e, Event::ResolvedRaw { .. }), Duration::from_secs(20)) {
        Event::ResolvedRaw { values, .. } => assert_eq!(values, vec![payload]),
        _ => unreachable!(),
    }
    // B resolving its own record finds it locally, as the reference would.
    assert!(b.resolve_raw("bob"));
    match wait_for(&b, |e| matches!(e, Event::ResolvedRaw { .. }), Duration::from_secs(20)) {
        Event::ResolvedRaw { values, .. } => assert_eq!(values.len(), 1),
        _ => unreachable!(),
    }
}

#[test]
fn an_idle_host_keeps_its_peers_warm_and_says_who_answered() {
    let b = NodeHost::spawn(id(9), "127.0.0.1:0".parse().unwrap()).unwrap();
    let a = NodeHost::spawn_with_keepalive(id(11), "127.0.0.1:0".parse().unwrap(),
                                           Duration::from_millis(150)).unwrap();
    assert!(a.bootstrap(b.local_addr(), id(9)));
    wait_for(&a, |e| matches!(e, Event::Table { usable: 1 }), Duration::from_secs(5));
    // With nothing asked of it, A pings B on its own and reports.
    match wait_for(&a, |e| matches!(e, Event::Keepalive { .. }), Duration::from_secs(5)) {
        Event::Keepalive { pinged, answered } => { assert_eq!(pinged, 1); assert_eq!(answered, 1); }
        _ => unreachable!(),
    }
    // B, gone: the next round says so rather than pretending.
    drop(b);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match wait_for(&a, |e| matches!(e, Event::Keepalive { .. }), Duration::from_secs(5)) {
            Event::Keepalive { answered: 0, pinged } => { assert_eq!(pinged, 1); break; }
            _ if Instant::now() < deadline => continue,
            other => panic!("B never went unanswered: {other:?}"),
        }
    }
}

/// A NODE THAT LOSES ITS ONLY PEER JOINS AGAIN. A's seed is B's address. B dies
/// and A's keepalive strikes it out; a new node comes up at the same address; A's
/// maintenance re-adds the seed, walks, and can resolve what the new node holds.
/// The node used to add its bootstrap list once, at spawn, and never again -- a
/// network change or a sleeping radio left it cut off until the app restarted.
#[test]
fn a_node_whose_table_empties_rejoins_from_its_seeds() {
    let b = NodeHost::spawn(id(21), "127.0.0.1:0".parse().unwrap()).unwrap();
    let b_addr = b.local_addr();
    let a = NodeHost::spawn_tuned(id(23), "127.0.0.1:0".parse().unwrap(),
                                  Duration::from_millis(150), Duration::from_millis(300)).unwrap();
    assert!(a.bootstrap(b_addr, NodeId([0xEE; 20])));
    wait_for(&a, |e| matches!(e, Event::Table { usable: 1 }), Duration::from_secs(5));
    drop(b);
    // Three unanswered keepalives: without a rejoin, B is now Bad and hidden, and
    // A knows nobody.
    for _ in 0..3 {
        wait_for(&a, |e| matches!(e, Event::Keepalive { answered: 0, .. }), Duration::from_secs(10));
    }

    // A new node at the same address, holding a record A has never seen.
    let c = NodeHost::spawn(id(25), b_addr).expect("rebind the dead node's port");
    assert!(c.publish("carol", "Carol", "10.0.0.9", 5555));
    wait_for(&c, |e| matches!(e, Event::Published { .. }), Duration::from_secs(10));

    // A rejoins by itself and finds it.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(Instant::now() < deadline, "A never rejoined and found carol");
        assert!(a.resolve("carol"));
        if let Event::Resolved { found, .. } = wait_for(&a, |e| matches!(e, Event::Resolved { .. }), Duration::from_secs(20)) {
            if found.iter().any(|p| p.display_name == "Carol") { break; }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}
