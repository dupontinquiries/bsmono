//! dht_join_probe (r197) — drives the DHT join FSM and the dual-mailbox
//! transport through a complete two-peer handshake WITHOUT a live socket,
//! so the playtester world-join path is verifiable in-sandbox. Models:
//!   1. host + joiner each get a stable unendlich-id (Identity::derive);
//!   2. the joiner runs the join FSM against the host as bootstrap until
//!      Phase::Joined (the actual step() state machine);
//!   3. once joined, a rollback INPUT packet is mailboxed host<->joiner
//!      via Mailboxes (the dual mailbox), fetched, and verified delivered.
//! This is the transport contract the rollback netcode rides on.
use unendlich_engine::sim::dht_join::{self, DhtState, Envelope, Input, Phase};
use unendlich_engine::sim::mp_dht::{Mail, Mailboxes};
use unendlich_engine::sim::dht_join::NodeId as _NodeId;
use unendlich_engine::sim::unendlich_id::{Identity, NetMode};

fn drive_to_joined(host: &mut DhtState, joiner: &mut DhtState) -> u32 {
    // joiner starts discovery
    let (s, mut pending) = dht_join::step(joiner.clone(), Input::Start, 0.0);
    *joiner = s;
    let mut ticks = 0u32;
    // Pump envelopes between the two until the joiner reaches Joined or we
    // give up. Each envelope is delivered to its `to` peer; replies queue.
    while joiner.phase != Phase::Joined && ticks < 32 {
        ticks += 1;
        let mut next: Vec<Envelope> = Vec::new();
        for e in pending.drain(..) {
            let target_is_host = e.to == host.self_id;
            let (ns, reply) = dht_join::step(
                if target_is_host { host.clone() } else { joiner.clone() },
                Input::Deliver(e),
                ticks as f64,
            );
            if target_is_host {
                *host = ns;
            } else {
                *joiner = ns;
            }
            next.extend(reply);
        }
        if next.is_empty() {
            // nudge with a Poll so timers/retries can fire
            let (ns, more) = dht_join::step(joiner.clone(), Input::Poll, ticks as f64);
            *joiner = ns;
            next.extend(more);
        }
        pending = next;
    }
    ticks
}

fn main() {
    // 1. identities (rollback is the default netmode for playtesters)
    assert_eq!(NetMode::default(), NetMode::Rollback);
    let host_id = Identity::derive("host-anna", 100);
    let joiner_id = Identity::derive("joiner-ben", 200);
    assert_ne!(host_id, joiner_id);
    println!("host id  = {host_id:016x}");
    println!("joiner id= {joiner_id:016x}");

    // 2. join FSM: joiner bootstraps off the host
    let mut host = DhtState::new(host_id, vec![]);
    let mut joiner = DhtState::new(joiner_id, vec![host_id]);
    let ticks = drive_to_joined(&mut host, &mut joiner);
    let joined = joiner.phase == Phase::Joined;
    println!("join: phase={:?} after {ticks} tick(s) -> {}",
        joiner.phase, if joined { "JOINED" } else { "FAILED" });

    // 3. dual mailbox: a rollback input packet host<->joiner
    let mut boxes = Mailboxes::new(64);
    // joiner sends its tick-input to the host's box
    let put1 = boxes.put(Mail {
        to: host_id, from: joiner_id, topic: 1,
        body: vec![0xA5, 0x00, 0x10], // (toy: action bits + tick lo/hi)
        t: 1,
    });
    // host acks state back to the joiner's box
    let put2 = boxes.put(Mail {
        to: joiner_id, from: host_id, topic: 2,
        body: vec![0x01, 0x02, 0x03, 0x04],
        t: 2,
    });
    let host_in = boxes.fetch(host_id);
    let joiner_in = boxes.fetch(joiner_id);
    let mail_ok = put1 && put2 && host_in.len() == 1 && joiner_in.len() == 1
        && host_in[0].from == joiner_id && joiner_in[0].from == host_id;
    println!("mailbox: host_in={} joiner_in={} -> {}",
        host_in.len(), joiner_in.len(), if mail_ok { "DELIVERED" } else { "FAILED" });
    // boxes drain after fetch (each side reads once)
    let drained = boxes.pending(host_id) == 0 && boxes.pending(joiner_id) == 0;

    let pass = joined && mail_ok && drained;
    println!("dht join + dual mailbox: {}", if pass { "PASS" } else { "FAIL" });
    std::process::exit(if pass { 0 } else { 1 });
}
