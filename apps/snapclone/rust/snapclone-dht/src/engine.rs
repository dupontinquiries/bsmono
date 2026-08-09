//! engine — the sans-io DHT node: a routing table + storage driven by incoming packets and
//! a clock, emitting outgoing packets. Faithful to OpenDHT's search grammar (dht.cpp):
//! iterative lookups keep the `SEARCH_NODES` closest candidates seen so far and keep at most
//! `MAX_REQUESTED_SEARCH_NODES` queries in flight, terminating when the closest set stops
//! improving. `get`/`put`/`listen` ride the same lookup; `put` first `get`s to obtain the
//! write token each storing node demands.
//!
//! No sockets, no threads: `handle_packet` and `tick` return `Vec<Out>` for the host to send.
//! `virtual_network_builder` in the reference validates via simulation; `sim.rs` does the same.

use crate::core::*;
use crate::wire::{Msg, Q};
use std::collections::HashMap;

pub struct Out { pub to: Addr, pub bytes: Vec<u8> }

/// How often a stored value is re-published. Ten minutes, comfortably inside
/// the TTLs the group feed uses.
pub const REANNOUNCE_MS: u64 = 600_000;
/// How often quiet buckets are touched. Fifteen minutes: staleness costs a
/// slower lookup, never a failure.
pub const REFRESH_MS: u64 = 900_000;

#[derive(Clone, Copy, PartialEq)]
enum Op { Find, Get, Put, Listen }

struct Search {
    target: InfoHash,
    op: Op,
    /// candidates sorted by XOR distance; (id, addr, queried, replied, token)
    cand: Vec<(InfoHash, Addr, bool, bool, u32)>,
    in_flight: usize,
    put_value: Option<Value>,
    got_values: Vec<Value>,
    done: bool,
    /// best distance seen; used to detect "no longer improving"
    best_bits: u32,
    stale_rounds: u32,
}

fn send(to: Addr, m: Msg) -> Out { Out { to, bytes: m.encode() } }

pub struct Engine {
    pub id: InfoHash,
    pub table: RoutingTable,
    pub store: Storage,
    next_tid: u32,
    /// tid -> (search index, node addr) so a reply advances the right search
    pending: HashMap<u32, (usize, Addr)>,
    searches: Vec<Search>,
    token_secret: u32,
    /// When maintenance last ran, in the caller's clock. Zero means "never",
    /// which makes both jobs due on the first tick — correct for a node that
    /// has just joined and should publish before waiting a window.
    last_reannounce_ms: u64,
    last_refresh_ms: u64,
    /// completed search results the host can poll: target -> values
    pub results: HashMap<InfoHash, Vec<Value>>,
    /// values delivered by listener pushes (target -> values), for the app layer
    pub pushed: Vec<(InfoHash, Value)>,
}

impl Engine {
    pub fn new(id: InfoHash) -> Self {
        Engine {
            id, table: RoutingTable::new(id), store: Storage::default(),
            next_tid: 1, pending: HashMap::new(), searches: Vec::new(),
            last_reannounce_ms: 0, last_refresh_ms: 0,
            token_secret: 0x5EED ^ (id.0[0] as u32) << 8 | id.0[1] as u32,
            results: HashMap::new(), pushed: Vec::new(),
        }
    }

    fn tid(&mut self) -> u32 { let t = self.next_tid; self.next_tid = self.next_tid.wrapping_add(1).max(1); t }

    /// A write token bound to (who, key) — the reference hands this out in get replies and
    /// requires it back on announce, so a node can't be told to store for an address it never
    /// queried from. Deterministic here (hash of who+key+secret) so both sides agree.
    fn token_for(&self, who: Addr, key: &InfoHash) -> u32 {
        let mut h = self.token_secret ^ (who as u32).wrapping_mul(0x9E3779B1);
        for b in &key.0[..4] { h = h.wrapping_mul(31).wrapping_add(*b as u32); }
        h | 1 // never 0 (0 = "no token" on the wire)
    }

    pub fn bootstrap(&mut self, addr: Addr, now: u64) -> Vec<Out> {
        // ping the bootstrap node; its reply seeds the table, then a self-find populates buckets
        let out = send(addr, Msg::Query {
            tid: self.tid(), from: self.id, q: Q::Ping, target: self.id, token: 0, value: None });
        let mut v = vec![out];
        v.extend(self.start(self.id, Op::Find, None, now));
        v
    }

    pub fn get(&mut self, key: InfoHash, now: u64) -> Vec<Out> { self.start(key, Op::Get, None, now) }
    pub fn listen(&mut self, key: InfoHash, now: u64) -> Vec<Out> { self.start(key, Op::Listen, None, now) }
    pub fn put(&mut self, key: InfoHash, value: Value, now: u64) -> Vec<Out> {
        // put rides a get: we need each storing node's write token first
        self.start(key, Op::Put, Some(value), now)
    }

    fn start(&mut self, target: InfoHash, op: Op, put_value: Option<Value>, now: u64) -> Vec<Out> {
        let mut cand: Vec<_> = self.table.closest(&target, SEARCH_NODES).into_iter()
            .map(|e| (e.id, e.addr, false, false, 0u32)).collect();
        cand.sort_by_key(|(id, ..)| id.xor(&target).0);
        let best_bits = cand.first().map(|(id, ..)| InfoHash::common_bits(id, &target)).unwrap_or(0);
        let si = self.searches.len();
        self.searches.push(Search { target, op, cand, in_flight: 0, put_value,
            got_values: Vec::new(), done: false, best_bits, stale_rounds: 0 });
        self.pump(si, now)
    }

    /// Fire queries for a search up to the in-flight cap, in distance order.
    fn pump(&mut self, si: usize, now: u64) -> Vec<Out> {
        let mut outs = Vec::new();
        let (target, op) = { let s = &self.searches[si]; (s.target, s.op) };
        loop {
            let (addr, tid);
            {
                let s = &mut self.searches[si];
                if s.done || s.in_flight >= MAX_REQUESTED_SEARCH_NODES { break; }
                let Some(idx) = s.cand.iter().position(|(_, _, queried, ..)| !queried) else { break; };
                s.cand[idx].2 = true;
                s.in_flight += 1;
                addr = s.cand[idx].1;
            }
            tid = self.tid();
            self.pending.insert(tid, (si, addr));
            let q = match op { Op::Find => Q::Find, Op::Listen => Q::Listen, _ => Q::Get };
            outs.push(send(addr, Msg::Query { tid, from: self.id, q, target, token: 0, value: None }));
            let _ = now;
        }
        // termination: nothing in flight and nothing left to query
        {
            let s = &mut self.searches[si];
            if !s.done && s.in_flight == 0 && s.cand.iter().all(|(_, _, queried, ..)| *queried) {
                s.done = true;
            }
        }
        if self.searches[si].done { outs.extend(self.finish(si, now)); }
        outs
    }

    fn finish(&mut self, si: usize, now: u64) -> Vec<Out> {
        let mut outs = Vec::new();
        let s = &self.searches[si];
        match s.op {
            Op::Get | Op::Find => {
                self.results.entry(s.target).or_default().extend(s.got_values.iter().cloned());
            }
            Op::Listen => {
                // register listen at the closest replied nodes so future puts push to us
                let closest: Vec<(Addr, u32)> = s.cand.iter()
                    .filter(|(_, _, _, replied, _)| *replied)
                    .take(TARGET_NODES).map(|(_, a, _, _, tok)| (*a, *tok)).collect();
                let target = s.target;
                for (addr, _tok) in closest {
                    let tid = self.tid();
                    outs.push(send(addr, Msg::Query {
                        tid, from: self.id, q: Q::Listen, target, token: 0, value: None }));
                }
            }
            Op::Put => {
                // announce to the closest replied nodes, each with the token IT gave us
                let value = s.put_value.clone();
                let closest: Vec<(Addr, u32)> = s.cand.iter()
                    .filter(|(_, _, _, replied, tok)| *replied && *tok != 0)
                    .take(TARGET_NODES).map(|(_, a, _, _, tok)| (*a, *tok)).collect();
                let target = s.target;
                if let Some(v) = value {
                    for (addr, tok) in closest {
                        let tid = self.tid();
                        outs.push(send(addr, Msg::Query {
                            tid, from: self.id, q: Q::Put, target, token: tok, value: Some(v.clone()) }));
                    }
                }
            }
        }
        outs
    }

    pub fn handle_packet(&mut self, from: Addr, bytes: &[u8], now: u64) -> Vec<Out> {
        let Some(msg) = Msg::decode(bytes) else { return vec![]; };
        match msg {
            Msg::Query { tid, from: fid, q, target, token, value } => {
                self.table.on_seen(fid, from, now);
                self.answer(from, tid, fid, q, target, token, value, now)
            }
            Msg::Reply { tid, from: fid, nodes, values, token } => {
                self.table.on_seen(fid, from, now);
                self.on_reply(tid, from, nodes, values, token, now)
            }
            Msg::Push { from: fid, target, values, .. } => {
                self.table.on_seen(fid, from, now);
                for v in values { self.pushed.push((target, v)); }
                vec![]
            }
            Msg::Error { .. } => vec![],
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn answer(&mut self, from: Addr, tid: u32, _fid: InfoHash, q: Q, target: InfoHash,
              token: u32, value: Option<Value>, now: u64) -> Vec<Out> {
        match q {
            Q::Ping => vec![send(from, Msg::Reply {
                tid, from: self.id, nodes: vec![], values: vec![], token: 0 })],
            Q::Find | Q::Get | Q::Listen => {
                let nodes: Vec<(InfoHash, Addr)> = self.table.closest(&target, TARGET_NODES)
                    .into_iter().map(|e| (e.id, e.addr)).collect();
                let values = if matches!(q, Q::Get | Q::Listen) { self.store.get(&target, now) } else { vec![] };
                let tok = self.token_for(from, &target);
                self.store.issue_token(from, target, tok);
                if matches!(q, Q::Listen) { self.store.listen(target, from, tid); }
                vec![send(from, Msg::Reply { tid, from: self.id, nodes, values, token: tok })]
            }
            Q::Put => {
                // enforce the write token the reference requires
                if token == 0 || !self.store.check_token(from, &target, token) {
                    return vec![send(from, Msg::Error { tid, code: 401 })]; // unauthorized
                }
                let mut outs = vec![];
                if let Some(v) = value {
                    let listeners = self.store.put(target, v.clone(), now);
                    // push to registered listeners (reference ValueData)
                    for (who, _lt) in listeners {
                        if who == from { continue; }
                        outs.push(send(who, Msg::Push {
                            tid: self.tid(), from: self.id, target, values: vec![v.clone()] }));
                    }
                }
                outs.push(send(from, Msg::Reply { tid, from: self.id, nodes: vec![], values: vec![], token: 0 }));
                outs
            }
        }
    }

    fn on_reply(&mut self, tid: u32, from: Addr, nodes: Vec<(InfoHash, Addr)>,
                values: Vec<Value>, token: u32, now: u64) -> Vec<Out> {
        let Some((si, _addr)) = self.pending.remove(&tid) else { return vec![]; };
        if si >= self.searches.len() { return vec![]; }
        {
            let s = &mut self.searches[si];
            s.in_flight = s.in_flight.saturating_sub(1);
            // mark the responder replied + record its token
            if let Some(c) = s.cand.iter_mut().find(|(_, a, ..)| *a == from) {
                c.3 = true; c.4 = token;
            }
            // fold in returned nodes, keeping the SEARCH_NODES closest, de-duped
            let target = s.target;
            for (id, addr) in nodes {
                if id == self.id { continue; }
                if !s.cand.iter().any(|(cid, ..)| *cid == id) {
                    s.cand.push((id, addr, false, false, 0));
                }
            }
            s.cand.sort_by_key(|(id, ..)| id.xor(&target).0);
            s.cand.truncate(SEARCH_NODES);
            // improvement check (dht.cpp: search ends when the closest set stops improving)
            let new_best = s.cand.first().map(|(id, ..)| InfoHash::common_bits(id, &target)).unwrap_or(0);
            if new_best > s.best_bits { s.best_bits = new_best; s.stale_rounds = 0; }
            else { s.stale_rounds += 1; }
            for v in values { if !s.got_values.contains(&v) { s.got_values.push(v); } }
        }
        self.pump(si, now)
    }

    /// Periodic upkeep. Expiry always runs; network maintenance runs only when
    /// `wake_kit` says this is a wake-up worth spending.
    ///
    /// The comment that used to sit here said reannounce and bucket refresh
    /// "would live" in this function. They now do, and they are gated rather
    /// than unconditional — which is the whole point. A DHT node that
    /// reannounced on its own timer would wake the modem on a schedule nothing
    /// else on the device shares, and after a transmission a cellular radio
    /// holds a high-power state for seconds. Ten scattered wake-ups cost far
    /// more than ten batched ones carrying identical bytes.
    ///
    /// `radio_up` is the caller's: it knows whether a call, a transfer or a
    /// user-initiated fetch already has the modem hot, and this engine does
    /// not. Passing `false` is always safe — it means "do not wake on my
    /// account", which is the conservative direction.
    pub fn tick_with_radio(&mut self, now: u64, radio_up: bool) -> Vec<Out> {
        // Expiry is local bookkeeping — no packets, no radio — so it is never
        // gated. Deferring it would grow memory for no power saving at all.
        self.store.expire(now);

        let jobs = vec![
            // Re-announce is Background: the presence record has a TTL measured
            // in minutes, so a delay of one window is invisible while the
            // saving is not.
            wake_kit::Job {
                name: "reannounce".into(),
                period_ms: REANNOUNCE_MS,
                urgency: wake_kit::Urgency::Background,
                last_ms: self.last_reannounce_ms,
            },
            // Bucket refresh is Opportunistic: a stale bucket costs a slower
            // lookup later, never a failure, so it must never be the reason a
            // modem powers up.
            wake_kit::Job {
                name: "refresh".into(),
                period_ms: REFRESH_MS,
                urgency: wake_kit::Urgency::Opportunistic,
                last_ms: self.last_refresh_ms,
            },
        ];
        let plan = wake_kit::plan(&jobs, &self.id.0, now, radio_up);

        let mut out = Vec::new();
        for job in &plan.run {
            match job.as_str() {
                "reannounce" => {
                    self.last_reannounce_ms = now;
                    out.extend(self.reannounce_all(now));
                }
                "refresh" => {
                    self.last_refresh_ms = now;
                    out.extend(self.refresh_buckets(now));
                }
                _ => {}
            }
        }
        out
    }

    /// Backwards-compatible tick: never wakes the radio on its own account.
    ///
    /// Kept so the sim and existing callers are unchanged — `sim.rs` drives
    /// puts explicitly and has no radio to model.
    pub fn tick(&mut self, now: u64) -> Vec<Out> {
        self.tick_with_radio(now, false)
    }

    /// Re-publish what this node stores locally, so values survive their TTL.
    fn reannounce_all(&mut self, now: u64) -> Vec<Out> {
        let keys = self.store.local_keys();
        let mut out = Vec::new();
        for k in keys {
            out.extend(self.start(k, Op::Get, None, now));
        }
        out
    }

    /// Touch buckets that have gone quiet by looking up a random id in each.
    fn refresh_buckets(&mut self, now: u64) -> Vec<Out> {
        // One self-lookup is the cheap approximation: it walks the table and
        // re-contacts the nodes nearest us, which is where staleness hurts
        // most. A per-bucket sweep is the thorough version and is a lot more
        // packets for a benefit nobody has measured here.
        self.start(self.id, Op::Find, None, now)
    }

    /// Poll-and-clear resolved get results for a key.
    pub fn take_result(&mut self, key: &InfoHash) -> Vec<Value> {
        self.results.remove(key).unwrap_or_default()
    }
}
