//! `sim::net::client_join` — the client-side join driver.
//!
//! Sequences the entire join into one object the bin drives with I/O only:
//!
//! 1. [`request`](ClientJoin::request) — drop a join request into the host's
//!    mailbox (`dht_mailbox::send_request`).
//! 2. [`poll`](ClientJoin::poll) — drain our mailbox; on an `Accept`, parse the
//!    host address and start the hole-punch (`rendezvous_join::begin_punch`); on a
//!    `Deny`, stop.
//! 3. [`drive`](ClientJoin::drive) / [`on_packet`](ClientJoin::on_packet) — run the
//!    punch FSM to `Connected`, emitting the datagrams the bin must send.
//!
//! Composes `dht_mailbox` (handshake, IP-protection invariant), `rendezvous_join`
//! (Accept→punch), and `net_rendezvous` (the punch FSM). The bin supplies the DHT
//! put/get, the UDP send/recv, and the clock — **no protocol logic in the bin**.
//! On [`Phase::Connected`] the joiner rebuilds its world for [`host_seed`] (the
//! seed it took from the selected advert — the r346 seed-coherence rule).
//!
//! Pure and deterministic given its inputs.

use crate::sim::net::dht_mailbox::{poll_response, send_request, JoinOutcome, MailboxDht};
use crate::sim::net::identity::UnendlichId;
use crate::sim::net::rendezvous_join::begin_punch;
use crate::sim::net_rendezvous::{Action, Candidate, HolePunch, PacketKind, PunchConfig, PunchState};

/// Where a join is in its lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Request not yet sent.
    Requesting,
    /// Request sent; waiting for the host's mailbox reply.
    Waiting,
    /// Accepted; hole-punching toward the host.
    Punching,
    /// Direct path established.
    Connected,
    /// Host denied the join.
    Denied,
    /// Accepted but unusable (no address) or the punch failed.
    Failed,
}

/// Drives one client→host join to completion.
pub struct ClientJoin {
    my_id: UnendlichId,
    host_id: UnendlichId,
    requested_name: String,
    host_seed: u64,
    phase: Phase,
    punch: Option<HolePunch>,
    assigned_name: Option<String>,
}

impl ClientJoin {
    /// Begin a join toward `host_id` for the world advertised with `host_seed`
    /// (taken from the selected advert; the joiner rebuilds for it on Connected).
    pub fn new(
        my_id: UnendlichId,
        host_id: UnendlichId,
        requested_name: impl Into<String>,
        host_seed: u64,
    ) -> ClientJoin {
        ClientJoin {
            my_id,
            host_id,
            requested_name: requested_name.into(),
            host_seed,
            phase: Phase::Requesting,
            punch: None,
            assigned_name: None,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn host_seed(&self) -> u64 {
        self.host_seed
    }
    pub fn assigned_name(&self) -> Option<&str> {
        self.assigned_name.as_deref()
    }

    /// Step 1 (idempotent): drop the join request into the host's mailbox. The bin
    /// performs the actual DHT put via the adapter.
    pub fn request<D: MailboxDht>(&mut self, dht: &D) {
        if self.phase == Phase::Requesting {
            send_request(dht, &self.host_id, &self.my_id, &self.requested_name);
            self.phase = Phase::Waiting;
        }
    }

    /// Step 2 (poll until it transitions): drain our mailbox. On `Accept`, parse the
    /// host's address(es) and start the punch with our `local` candidates; on `Deny`,
    /// stop. The punch params (`cfg`, `txid_seed`, `peer_id`, `session`, `nonce`)
    /// come from the session context.
    #[allow(clippy::too_many_arguments)]
    pub fn poll<D: MailboxDht>(
        &mut self,
        dht: &D,
        local: Vec<Candidate>,
        cfg: PunchConfig,
        txid_seed: u64,
        peer_id: u64,
        session: u64,
        nonce: u64,
    ) {
        if self.phase != Phase::Waiting {
            return;
        }
        match poll_response(dht, &self.my_id) {
            Some(JoinOutcome::Accepted { assigned_name, addr }) => {
                self.assigned_name = Some(assigned_name.clone());
                let outcome = JoinOutcome::Accepted { assigned_name, addr };
                match begin_punch(&outcome, local, cfg, txid_seed, peer_id, session, nonce) {
                    Some(hp) => {
                        self.punch = Some(hp);
                        self.phase = Phase::Punching;
                    }
                    None => self.phase = Phase::Failed, // accepted but no usable address
                }
            }
            Some(JoinOutcome::Denied) => self.phase = Phase::Denied,
            None => {} // nothing yet — keep waiting
        }
    }

    /// Step 3 (drive each tick): advance the punch, returning the datagrams the bin
    /// must send. Transitions to Connected / Failed when the FSM does.
    pub fn drive(&mut self, now_ms: u64) -> Vec<Action> {
        let acts = match &mut self.punch {
            Some(hp) => hp.poll(now_ms),
            None => return Vec::new(),
        };
        if let Some(hp) = &self.punch {
            match hp.state() {
                PunchState::Connected => self.phase = Phase::Connected,
                PunchState::Failed => self.phase = Phase::Failed,
                _ => {}
            }
        }
        acts
    }

    /// Feed a received punch datagram (the bin's UDP recv).
    pub fn on_packet(&mut self, pair: usize, kind: PacketKind, txid: u64, now_ms: u64) {
        if let Some(hp) = &mut self.punch {
            hp.on_packet(pair, kind, txid, now_ms);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::net::dht_mailbox::{mailbox_key, LocalDht, MailMsg};

    fn id(s: &str) -> UnendlichId {
        UnendlichId::new(s)
    }
    fn local() -> Vec<Candidate> {
        use crate::sim::net_rendezvous::{candidate_priority, CandKind};
        vec![Candidate {
            ip: [192, 168, 1, 9],
            port: 7777,
            kind: CandKind::Host,
            priority: candidate_priority(CandKind::Host, 0),
        }]
    }

    fn drop_into_my_box(dht: &LocalDht, me: &str, msg: MailMsg) {
        dht.put(mailbox_key(&id(me)), msg.encode());
    }

    #[test]
    fn full_join_reaches_connected() {
        let dht = LocalDht::default();
        let mut cj = ClientJoin::new(id("me"), id("host"), "guest", 0x5EED_BEEF);
        assert_eq!(cj.phase(), Phase::Requesting);

        cj.request(&dht);
        assert_eq!(cj.phase(), Phase::Waiting);

        // Host hasn't replied yet → poll is a no-op.
        cj.poll(&dht, local(), PunchConfig::default(), 1, 1, 1, 1);
        assert_eq!(cj.phase(), Phase::Waiting);

        // Host accepts: an Accept lands in our mailbox carrying its address.
        drop_into_my_box(
            &dht,
            "me",
            MailMsg::Accept {
                from: id("host"),
                assigned_name: "guest".into(),
                addr: "192.168.1.5:7777".into(),
            },
        );
        cj.poll(&dht, local(), PunchConfig::default(), 1, 1, 1, 1);
        assert_eq!(cj.phase(), Phase::Punching);
        assert_eq!(cj.assigned_name(), Some("guest"));

        // Drive the punch → it pings; simulate the matching Pong → Connected.
        let acts = cj.drive(0);
        let (pair, txid) = acts
            .iter()
            .find_map(|a| match a {
                Action::SendPing { pair, txid } => Some((*pair, *txid)),
                _ => None,
            })
            .expect("punch sends a ping");
        cj.on_packet(pair, PacketKind::Pong, txid, 10);
        let _ = cj.drive(10);
        assert_eq!(cj.phase(), Phase::Connected);
        assert_eq!(cj.host_seed(), 0x5EED_BEEF, "joiner keeps the host's seed to rebuild");
    }

    #[test]
    fn denied_join_stops() {
        let dht = LocalDht::default();
        let mut cj = ClientJoin::new(id("me"), id("host"), "guest", 1);
        cj.request(&dht);
        drop_into_my_box(&dht, "me", MailMsg::Deny { from: id("host") });
        cj.poll(&dht, local(), PunchConfig::default(), 1, 1, 1, 1);
        assert_eq!(cj.phase(), Phase::Denied);
    }

    #[test]
    fn addressless_accept_fails() {
        let dht = LocalDht::default();
        let mut cj = ClientJoin::new(id("me"), id("host"), "guest", 1);
        cj.request(&dht);
        drop_into_my_box(
            &dht,
            "me",
            MailMsg::Accept { from: id("host"), assigned_name: "g".into(), addr: "dht:onlyakey".into() },
        );
        cj.poll(&dht, local(), PunchConfig::default(), 1, 1, 1, 1);
        assert_eq!(cj.phase(), Phase::Failed);
    }

    #[test]
    fn request_is_idempotent() {
        let dht = LocalDht::default();
        let mut cj = ClientJoin::new(id("me"), id("host"), "guest", 1);
        cj.request(&dht);
        cj.request(&dht); // second call is a no-op (already Waiting)
        assert_eq!(cj.phase(), Phase::Waiting);
        // exactly one request sits in the host's mailbox
        assert_eq!(dht.get(mailbox_key(&id("host"))).len(), 1);
    }
}
