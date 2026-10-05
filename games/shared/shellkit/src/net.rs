//! Online play, riding the monorepo's fleet netcode exactly as designed in
//! games/NETCODE_DHT_ANALYSIS.md: the **control plane** is
//! `netcode::multiplayer::Multiplayer` — profile published to the snapcore
//! DHT store, `host()` announces a SessionRecord, `join(friend)` resolves it —
//! and the **data plane** is this module's own little UDP protocol on the
//! socket that handoff returns. Host-authoritative ball, star topology,
//! unreliable 30 Hz state, peers time out after 5 s. The store floor is the
//! shared FileStore (same-machine / shared-filesystem); the `opendht` feature
//! of the vendored netcode crate remains the global-reach upgrade.
//!
//! bsgames integration comes free: `netcode::handoff::from_env()` reads the
//! BSGAMES_HANDOFF_* variables, so the games centre can launch skydribble
//! straight into hosting or joining.
//!
//! The registry id a `Net` publishes (`"skydribble"`, `"blackstar-rs"`) is
//! the game's to pass to [`Net::new`]: it is what the hub dispatches a Join
//! on, and what rejects a cross-game join. Until c21 it was a constant here,
//! so blackstar-rs announced every match as skydribble -- the hub would have
//! launched the wrong game for a friend, and a skydribble client could dial a
//! blackstar-rs host.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Instant;

use glam::{Quat, Vec3};
use netcode::multiplayer::Multiplayer;

const MSG_HELLO: u8 = 1;
const MSG_WELCOME: u8 = 2;
const MSG_STATE: u8 = 3;
const MSG_PEERS: u8 = 4;
const MSG_BALL: u8 = 5;
const MSG_BYE: u8 = 6;
const SEND_DT: f32 = 1.0 / 30.0;
const PEER_TIMEOUT: f32 = 5.0;
pub const MAX_PLAYERS: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Offline,
    Host,
    Client,
}

pub struct PeerCar {
    pub name: String,
    pub car: usize,
    pub flip: bool,
    pub boosting: bool,
    pub pos: Vec3,
    pub vel: Vec3,
    pub rot: Quat,
    pub wheel_spin: f32,
    pub steer_vis: f32,
    pub shown: Vec3, // smoothed render position
    last: Instant,
}

/// What the game loop feeds in about the local car each tick.
pub struct MyState {
    pub car: usize,
    pub flip: bool,
    pub boosting: bool,
    pub pos: Vec3,
    pub vel: Vec3,
    pub rot: Quat,
    pub wheel_spin: f32,
    pub steer_vis: f32,
}

pub enum NetEvent {
    Joined(String),
    Left(String),
    MapSync { map: usize, seed: u64 },
}

pub struct Net {
    /// The registry id this game publishes and joins under (see the module doc).
    game: &'static str,
    announce_acc: f32, // host heartbeat: re-publish the session every ~20 s
    mp: Option<Multiplayer>,
    pub role: Role,
    pub status: String,
    peers: Vec<Option<PeerCar>>, // by slot; our own slot stays None
    addr_slot: HashMap<SocketAddr, u8>,
    my_slot: u8,
    send_acc: f32,
    hello_acc: f32,
    welcomed: bool,
    pub net_ball: Option<(Vec3, Vec3, Quat, Instant)>,
}

// ---- wire helpers --------------------------------------------------------
fn put_f(b: &mut Vec<u8>, v: f32) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn get_f(b: &[u8], o: &mut usize) -> f32 {
    let v = f32::from_le_bytes(b[*o..*o + 4].try_into().unwrap_or([0; 4]));
    *o += 4;
    v
}
fn put_v3(b: &mut Vec<u8>, v: Vec3) {
    put_f(b, v.x);
    put_f(b, v.y);
    put_f(b, v.z);
}
fn get_v3(b: &[u8], o: &mut usize) -> Vec3 {
    Vec3::new(get_f(b, o), get_f(b, o), get_f(b, o))
}
fn put_q(b: &mut Vec<u8>, q: Quat) {
    put_f(b, q.x);
    put_f(b, q.y);
    put_f(b, q.z);
    put_f(b, q.w);
}
fn get_q(b: &[u8], o: &mut usize) -> Quat {
    let q = Quat::from_xyzw(get_f(b, o), get_f(b, o), get_f(b, o), get_f(b, o));
    if q.length_squared() < 1e-4 {
        Quat::IDENTITY
    } else {
        q.normalize()
    }
}

const STATE_BYTES: usize = 3 + 13 * 4; // slot, car, flags + 13 floats

fn put_state(b: &mut Vec<u8>, slot: u8, s: &MyState) {
    b.push(slot);
    b.push(s.car as u8);
    b.push((s.flip as u8) | ((s.boosting as u8) << 1));
    put_v3(b, s.pos);
    put_v3(b, s.vel);
    put_q(b, s.rot);
    put_f(b, s.wheel_spin);
    put_f(b, s.steer_vis);
    put_f(b, 0.0); // spare
}

struct WireState {
    slot: u8,
    car: usize,
    flip: bool,
    boosting: bool,
    pos: Vec3,
    vel: Vec3,
    rot: Quat,
    wheel_spin: f32,
    steer_vis: f32,
}

fn get_state(b: &[u8], o: &mut usize) -> Option<WireState> {
    if b.len() < *o + STATE_BYTES {
        return None;
    }
    let slot = b[*o];
    let car = (b[*o + 1] as usize).min(8);
    let flags = b[*o + 2];
    *o += 3;
    let pos = get_v3(b, o);
    let vel = get_v3(b, o);
    let rot = get_q(b, o);
    let wheel_spin = get_f(b, o);
    let steer_vis = get_f(b, o);
    let _ = get_f(b, o);
    Some(WireState {
        slot,
        car,
        flip: flags & 1 != 0,
        boosting: flags & 2 != 0,
        pos,
        vel,
        rot,
        wheel_spin,
        steer_vis,
    })
}

fn put_str(b: &mut Vec<u8>, s: &str) {
    let s = &s.as_bytes()[..s.len().min(24)];
    b.push(s.len() as u8);
    b.extend_from_slice(s);
}
fn get_str(b: &[u8], o: &mut usize) -> Option<String> {
    let n = *b.get(*o)? as usize;
    *o += 1;
    let s = b.get(*o..*o + n)?;
    *o += n;
    Some(String::from_utf8_lossy(s).into_owned())
}

// --------------------------------------------------------------------------
impl Net {
    /// `game` is the bsgames registry id (`bsgames::centre::registry`) this
    /// build publishes under -- `"skydribble"` or `"blackstar-rs"`.
    pub fn new(game: &'static str) -> Net {
        Net {
            game,
            announce_acc: 0.0,
            mp: None,
            role: Role::Offline,
            status: String::new(),
            peers: (0..MAX_PLAYERS).map(|_| None).collect(),
            addr_slot: HashMap::new(),
            my_slot: 0,
            send_acc: 0.0,
            hello_acc: 0.0,
            welcomed: false,
            net_ball: None,
        }
    }

    pub fn online(&self) -> bool {
        self.role != Role::Offline
    }

    fn reset_peers(&mut self) {
        self.peers = (0..MAX_PLAYERS).map(|_| None).collect();
        self.addr_slot.clear();
        self.net_ball = None;
        self.welcomed = false;
    }

    /// "▶ HOST": publish the profile, announce the session on the fleet store,
    /// bind the UDP data socket.
    pub fn host(&mut self, name: &str) -> String {
        self.leave();
        let name = clean_name(name);
        let mut mp = Multiplayer::new(self.game, &name, &name);
        match mp.host("0.0.0.0:0") {
            Ok(token) => {
                self.status = format!("HOSTING AS {name}  ({token})");
                self.role = Role::Host;
                self.my_slot = 0;
                self.reset_peers();
                self.mp = Some(mp);
            }
            Err(e) => self.status = format!("HOST FAILED: {}", short(&e)),
        }
        self.status.clone()
    }

    /// "▶ JOIN": resolve the friend's session record off the fleet store and
    /// dial their UDP address.
    pub fn join(&mut self, name: &str, friend: &str) -> String {
        self.leave();
        let name = clean_name(name);
        let friend = friend.trim();
        if friend.is_empty() {
            self.status = "JOIN: ENTER A FRIEND NAME FIRST".into();
            return self.status.clone();
        }
        let mut mp = Multiplayer::new(self.game, &name, &name);
        match mp.join(friend) {
            Ok(addr) => {
                self.status = format!("JOINED {friend} AT {addr}");
                self.role = Role::Client;
                self.my_slot = 255; // until WELCOME
                self.reset_peers();
                self.mp = Some(mp);
                self.send_hello(name.as_str());
            }
            Err(e) => self.status = format!("JOIN FAILED: {}", short(&e)),
        }
        self.status.clone()
    }

    fn send_hello(&self, name: &str) {
        let Some(mp) = &self.mp else { return };
        let Some(sock) = &mp.socket else { return };
        let mut b = vec![MSG_HELLO];
        put_str(&mut b, name);
        let _ = sock.send(&b);
    }

    pub fn leave(&mut self) {
        if let Some(mp) = &self.mp {
            if let Some(sock) = &mp.socket {
                let bye = [MSG_BYE, self.my_slot];
                match self.role {
                    Role::Client => {
                        let _ = sock.send(&bye);
                    }
                    Role::Host => {
                        for addr in self.addr_slot.keys() {
                            let _ = sock.send_to(&bye, addr);
                        }
                    }
                    Role::Offline => {}
                }
            }
        }
        if let Some(mut mp) = self.mp.take() {
            mp.stop_hosting();
        }
        self.role = Role::Offline;
        self.reset_peers();
        if !self.status.is_empty() {
            self.status = "OFFLINE".into();
        }
    }

    /// One frame of network: throttled sends of our state (+ ball if hosting),
    /// drain every waiting datagram, expire silent peers. `map`/`seed` ride in
    /// the WELCOME so a joiner loads the host's world.
    pub fn tick(
        &mut self,
        dt: f32,
        me: &MyState,
        ball_pos: Vec3,
        ball_vel: Vec3,
        ball_rot: Quat,
        map: usize,
        seed: u64,
        events: &mut Vec<NetEvent>,
    ) {
        if self.mp.is_none() || self.role == Role::Offline {
            return;
        }

        // host heartbeat: keep the session discoverable on the fleet store
        // (records + presence carry TTLs — announce-once hosts silently
        // fall off the friend list mid-match)
        if self.role == Role::Host {
            self.announce_acc += dt;
            if self.announce_acc >= 20.0 {
                self.announce_acc = 0.0;
                if let Some(mp) = self.mp.as_mut() {
                    mp.reannounce();
                }
            }
        }

        // ---- receive everything waiting ----
        let mut buf = [0u8; 1500];
        loop {
            let got = self.mp.as_ref().and_then(|mp| mp.poll(&mut buf));
            let Some((n, from)) = got else { break };
            self.handle(&buf[..n], from, map, seed, events);
        }

        // ---- expire silent peers ----
        for slot in 0..self.peers.len() {
            let gone = match &self.peers[slot] {
                Some(p) => p.last.elapsed().as_secs_f32() > PEER_TIMEOUT,
                None => false,
            };
            if gone {
                if let Some(p) = self.peers[slot].take() {
                    events.push(NetEvent::Left(p.name));
                }
                self.addr_slot.retain(|_, s| *s as usize != slot);
            }
        }

        // ---- throttled sends ----
        self.send_acc += dt;
        if self.send_acc < SEND_DT {
            return;
        }
        self.send_acc = 0.0;
        let Some(mp) = &self.mp else { return };
        let Some(sock) = &mp.socket else { return };
        match self.role {
            Role::Host => {
                // PEERS: our state + every live peer's, to every peer
                let mut b = vec![MSG_PEERS, 0];
                let mut count = 1u8;
                put_state(&mut b, 0, me);
                for (slot, p) in self.peers.iter().enumerate() {
                    if let Some(p) = p {
                        put_state(
                            &mut b,
                            slot as u8,
                            &MyState {
                                car: p.car,
                                flip: p.flip,
                                boosting: p.boosting,
                                pos: p.pos,
                                vel: p.vel,
                                rot: p.rot,
                                wheel_spin: p.wheel_spin,
                                steer_vis: p.steer_vis,
                            },
                        );
                        count += 1;
                    }
                }
                b[1] = count;
                let mut ballmsg = vec![MSG_BALL];
                put_v3(&mut ballmsg, ball_pos);
                put_v3(&mut ballmsg, ball_vel);
                put_q(&mut ballmsg, ball_rot);
                for addr in self.addr_slot.keys() {
                    let _ = sock.send_to(&b, addr);
                    let _ = sock.send_to(&ballmsg, addr);
                }
            }
            Role::Client => {
                if self.welcomed {
                    let mut b = vec![MSG_STATE];
                    put_state(&mut b, self.my_slot, me);
                    let _ = sock.send(&b);
                } else {
                    // keep knocking until the host answers
                    self.hello_acc += SEND_DT;
                    if self.hello_acc > 0.5 {
                        self.hello_acc = 0.0;
                        self.send_hello(&mp.me.name.clone());
                    }
                }
            }
            Role::Offline => {}
        }
    }

    fn handle(&mut self, b: &[u8], from: SocketAddr, map: usize, seed: u64, events: &mut Vec<NetEvent>) {
        let Some(&msg) = b.first() else { return };
        match (msg, self.role) {
            (MSG_HELLO, Role::Host) => {
                let mut o = 1;
                let name = get_str(b, &mut o).unwrap_or_else(|| "PILOT".into());
                let slot = match self.addr_slot.get(&from) {
                    Some(s) => *s,
                    None => {
                        let Some(free) = (1..MAX_PLAYERS as u8).find(|s| self.peers[*s as usize].is_none()) else {
                            return; // full
                        };
                        self.addr_slot.insert(from, free);
                        self.peers[free as usize] = Some(PeerCar {
                            name: name.clone(),
                            car: 0,
                            flip: false,
                            boosting: false,
                            pos: Vec3::ZERO,
                            vel: Vec3::ZERO,
                            rot: Quat::IDENTITY,
                            wheel_spin: 0.0,
                            steer_vis: 0.0,
                            shown: Vec3::ZERO,
                            last: Instant::now(),
                        });
                        events.push(NetEvent::Joined(name));
                        free
                    }
                };
                // WELCOME: slot + the world to load
                let mut w = vec![MSG_WELCOME, slot];
                w.extend_from_slice(&(map as u32).to_le_bytes());
                w.extend_from_slice(&seed.to_le_bytes());
                if let Some(mp) = &self.mp {
                    if let Some(sock) = &mp.socket {
                        let _ = sock.send_to(&w, from);
                    }
                }
            }
            (MSG_STATE, Role::Host) => {
                let Some(&slot) = self.addr_slot.get(&from) else { return };
                let mut o = 1;
                if let Some(ws) = get_state(b, &mut o) {
                    if ws.slot == slot {
                        if let Some(p) = self.peers[slot as usize].as_mut() {
                            apply_state(p, &ws);
                        }
                    }
                }
            }
            (MSG_WELCOME, Role::Client) => {
                if b.len() >= 2 + 4 + 8 && !self.welcomed {
                    self.my_slot = b[1];
                    self.welcomed = true;
                    let m = u32::from_le_bytes(b[2..6].try_into().unwrap_or_default()) as usize;
                    let s = u64::from_le_bytes(b[6..14].try_into().unwrap_or_default());
                    events.push(NetEvent::MapSync { map: m, seed: s });
                    self.status = format!("CONNECTED (SLOT {})", self.my_slot);
                }
            }
            (MSG_PEERS, Role::Client) => {
                let mut o = 2;
                let count = *b.get(1).unwrap_or(&0);
                for _ in 0..count {
                    let Some(ws) = get_state(b, &mut o) else { break };
                    if ws.slot == self.my_slot || ws.slot as usize >= MAX_PLAYERS {
                        continue;
                    }
                    let slot = ws.slot as usize;
                    if self.peers[slot].is_none() {
                        self.peers[slot] = Some(PeerCar {
                            name: if slot == 0 { "HOST".into() } else { format!("PILOT {slot}") },
                            car: ws.car,
                            flip: false,
                            boosting: false,
                            pos: ws.pos,
                            vel: Vec3::ZERO,
                            rot: ws.rot,
                            wheel_spin: 0.0,
                            steer_vis: 0.0,
                            shown: ws.pos,
                            last: Instant::now(),
                        });
                        events.push(NetEvent::Joined(self.peers[slot].as_ref().unwrap().name.clone()));
                    }
                    if let Some(p) = self.peers[slot].as_mut() {
                        apply_state(p, &ws);
                    }
                }
            }
            (MSG_BALL, Role::Client) => {
                let mut o = 1;
                if b.len() >= 1 + 10 * 4 {
                    let pos = get_v3(b, &mut o);
                    let vel = get_v3(b, &mut o);
                    let rot = get_q(b, &mut o);
                    self.net_ball = Some((pos, vel, rot, Instant::now()));
                }
            }
            (MSG_BYE, Role::Host) => {
                if let Some(slot) = self.addr_slot.remove(&from) {
                    if let Some(p) = self.peers[slot as usize].take() {
                        events.push(NetEvent::Left(p.name));
                    }
                }
            }
            (MSG_BYE, Role::Client) => {
                self.status = "HOST LEFT".into();
                self.leave();
            }
            _ => {}
        }
    }

    /// Smooth remote cars toward their latest state; call once per frame.
    pub fn smooth(&mut self, dt: f32) {
        let k = (dt * 14.0).min(1.0);
        for p in self.peers.iter_mut().flatten() {
            // dead-reckon a touch, then pull the shown position in
            let target = p.pos + p.vel * p.last.elapsed().as_secs_f32().min(0.2);
            p.shown += (target - p.shown) * k;
            if (target - p.shown).length() > 6.0 {
                p.shown = target;
            }
        }
    }

    pub fn peers(&self) -> impl Iterator<Item = &PeerCar> {
        self.peers.iter().flatten()
    }

    pub fn player_count(&self) -> usize {
        1 + self.peers.iter().flatten().count()
    }
}

fn apply_state(p: &mut PeerCar, ws: &WireState) {
    p.car = ws.car;
    p.flip = ws.flip;
    p.boosting = ws.boosting;
    p.pos = ws.pos;
    p.vel = ws.vel;
    p.rot = ws.rot;
    p.wheel_spin = ws.wheel_spin;
    p.steer_vis = ws.steer_vis;
    p.last = Instant::now();
}

fn clean_name(n: &str) -> String {
    let n: String = n.trim().chars().filter(|c| !c.is_whitespace()).take(16).collect();
    if n.is_empty() {
        format!("PILOT{}", std::process::id() % 1000)
    } else {
        n
    }
}

fn short(e: &str) -> String {
    e.chars().take(48).collect()
}

/// bsgames launch integration: read BSGAMES_HANDOFF_* and host/join at boot.
/// Uses the DISPLAY name bsgames published (ME = "<id>|<name>"), so the
/// in-game tag matches the friend list instead of showing a raw id.
pub fn from_handoff(net: &mut Net) -> Option<String> {
    let h = netcode::handoff::from_env()?;
    Some(match h {
        netcode::handoff::Handoff::Host { me_name, .. } => net.host(&me_name),
        netcode::handoff::Handoff::Join { me_name, peer_id, .. } => net.join(&me_name, &peer_id),
    })
}

/// Test/sim variants pointing the fleet store at an explicit path, so two
/// Net stacks in ONE process (the `--sim mp` loopback pair) share a store.
impl Net {
    pub fn host_with(&mut self, name: &str, store: std::path::PathBuf) -> String {
        self.leave();
        let name = clean_name(name);
        let mut mp = Multiplayer::at_store(self.game, &name, &name, store);
        match mp.host("127.0.0.1:0") {
            Ok(token) => {
                self.status = format!("HOSTING AS {name}  ({token})");
                self.role = Role::Host;
                self.my_slot = 0;
                self.reset_peers();
                self.mp = Some(mp);
            }
            Err(e) => self.status = format!("HOST FAILED: {}", short(&e)),
        }
        self.status.clone()
    }
    pub fn join_with(&mut self, name: &str, friend: &str, store: std::path::PathBuf) -> String {
        self.leave();
        let name = clean_name(name);
        let mut mp = Multiplayer::at_store(self.game, &name, &name, store);
        match mp.join(friend) {
            Ok(addr) => {
                self.status = format!("JOINED {friend} AT {addr}");
                self.role = Role::Client;
                self.my_slot = 255;
                self.reset_peers();
                self.mp = Some(mp);
                self.send_hello(name.as_str());
            }
            Err(e) => self.status = format!("JOIN FAILED: {}", short(&e)),
        }
        self.status.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal peer at a given position, ready for `smooth()`. `last` is
    /// `Instant::now()` at construction, so calling `smooth` immediately
    /// after leaves dead-reckoning's `elapsed()` term at ~0 — this isolates
    /// the exponential-smoothing half of the function from the dead-
    /// reckoning half, which needs real wall-clock time to test instead.
    fn peer_at(pos: Vec3, shown: Vec3) -> PeerCar {
        PeerCar {
            name: "peer".into(),
            car: 0,
            flip: false,
            boosting: false,
            pos,
            vel: Vec3::ZERO,
            rot: Quat::IDENTITY,
            wheel_spin: 0.0,
            steer_vis: 0.0,
            shown,
            last: Instant::now(),
        }
    }

    fn net_with_peer(p: PeerCar) -> Net {
        let mut n = Net::new("skydribble");
        n.peers[0] = Some(p);
        n
    }

    #[test]
    fn smoothing_pulls_shown_toward_target_at_the_documented_rate() {
        // k = (dt * 14.0).min(1.0) is the function's own formula -- this
        // test would catch a change to either the 14.0 rate or the min(1.0)
        // clamp, not just "did shown move at all". Distance chosen so the
        // REMAINING gap after one smoothing step stays under the function's
        // own 6.0 snap threshold (2.0 * (1 - 0.28) = 1.44) -- a bigger gap
        // reads as "just teleported" and snaps instead of smoothing, which
        // is a different behavior this test isn't the one to cover.
        let mut n = net_with_peer(peer_at(Vec3::new(2.0, 0.0, 0.0), Vec3::ZERO));
        let dt = 0.02; // k = 0.28
        n.smooth(dt);
        let shown = n.peers[0].as_ref().unwrap().shown;
        let k = (dt * 14.0f32).min(1.0);
        let expected = Vec3::ZERO + (Vec3::new(2.0, 0.0, 0.0) - Vec3::ZERO) * k;
        assert!((shown - expected).length() < 1e-4, "shown={shown:?} expected={expected:?}");
    }

    #[test]
    fn a_large_dt_clamps_the_smoothing_rate_to_one_not_beyond() {
        // (dt * 14.0).min(1.0): a dropped-frame-sized dt (e.g. 1.0s) must
        // not overshoot target, which an unclamped k > 1.0 would do.
        let mut n = net_with_peer(peer_at(Vec3::new(1.0, 0.0, 0.0), Vec3::ZERO));
        n.smooth(1.0);
        let shown = n.peers[0].as_ref().unwrap().shown;
        assert!((shown - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-4, "overshot or undershot: {shown:?}");
    }

    #[test]
    fn a_target_more_than_six_units_away_snaps_instead_of_creeping() {
        // the function's own anti-rubber-band clause: a peer that jumped
        // (a respawn, a fold, a network hiccup catching up) should not be
        // seen crawling across the map at the smoothing rate.
        let mut n = net_with_peer(peer_at(Vec3::new(50.0, 0.0, 0.0), Vec3::ZERO));
        n.smooth(0.016);
        let shown = n.peers[0].as_ref().unwrap().shown;
        assert_eq!(shown, Vec3::new(50.0, 0.0, 0.0), "should have snapped, not smoothed: {shown:?}");
    }

    #[test]
    fn a_target_just_under_six_units_still_smooths() {
        // the boundary case right next to the snap test above: close
        // enough that even a slow dt's small step should stay under 6.0
        // rather than tripping the snap.
        let mut n = net_with_peer(peer_at(Vec3::new(5.9, 0.0, 0.0), Vec3::ZERO));
        n.smooth(0.016);
        let shown = n.peers[0].as_ref().unwrap().shown;
        assert!(shown.x > 0.0 && shown.x < 5.9, "should have smoothed partway, not snapped: {shown:?}");
    }

    #[test]
    fn player_count_is_one_plus_live_peers_not_slot_count() {
        // MAX_PLAYERS slots exist regardless of how many are filled --
        // player_count must read occupancy, not Vec length.
        let mut n = Net::new("skydribble");
        assert_eq!(n.player_count(), 1, "just the local player, no peers yet");
        n.peers[0] = Some(peer_at(Vec3::ZERO, Vec3::ZERO));
        n.peers[3] = Some(peer_at(Vec3::ZERO, Vec3::ZERO));
        assert_eq!(n.player_count(), 3, "local + 2 filled slots, not MAX_PLAYERS");
    }

    #[test]
    fn peers_iterator_skips_empty_slots() {
        let mut n = Net::new("skydribble");
        n.peers[2] = Some(peer_at(Vec3::new(7.0, 0.0, 0.0), Vec3::ZERO));
        let positions: Vec<Vec3> = n.peers().map(|p| p.pos).collect();
        assert_eq!(positions, vec![Vec3::new(7.0, 0.0, 0.0)], "empty slots should not appear");
    }

    // ---- wire protocol: encode/decode round trips and malformed input ----
    //
    // Every real call site into get_v3/get_q/get_f/get_state/get_str already
    // guards buffer length before calling (checked by hand, not assumed:
    // MSG_WELCOME/MSG_BALL check `b.len() >=` explicitly, MSG_STATE/MSG_PEERS
    // go through get_state's own internal length check first) -- so there's
    // no live panic-on-truncated-packet bug today. These tests exist to keep
    // it that way: a future message type that skips the length check before
    // calling get_v3/get_f directly would panic on a short/malicious UDP
    // packet, and nothing else in this file would catch that regression.

    #[test]
    fn v3_and_quat_round_trip_through_the_wire_format() {
        let mut b = Vec::new();
        let v = Vec3::new(1.5, -2.25, 100.0);
        let q = Quat::from_xyzw(0.1, 0.2, 0.3, 0.9).normalize();
        put_v3(&mut b, v);
        put_q(&mut b, q);
        let mut o = 0;
        assert_eq!(get_v3(&b, &mut o), v);
        let got_q = get_q(&b, &mut o);
        assert!((got_q.x - q.x).abs() < 1e-5 && (got_q.w - q.w).abs() < 1e-5, "got={got_q:?} want={q:?}");
        assert_eq!(o, b.len(), "offset should land exactly on the end after reading everything written");
    }

    #[test]
    fn a_near_zero_quat_decodes_to_identity_rather_than_nan() {
        // get_q's own defensive branch: normalizing a (0,0,0,0) quaternion
        // divides by zero and produces NaN, which would then corrupt a
        // peer's rotation and everything downstream of it (rendering,
        // collision) for the rest of the match. Confirms the guard fires
        // rather than assuming the comment describing it is still true.
        let mut b = Vec::new();
        put_q(&mut b, Quat::from_xyzw(0.0, 0.0, 0.0, 0.0));
        let mut o = 0;
        let q = get_q(&b, &mut o);
        assert_eq!(q, Quat::IDENTITY);
        assert!(!q.x.is_nan() && !q.w.is_nan());
    }

    #[test]
    fn state_round_trips_position_velocity_rotation_and_flags() {
        let mut b = Vec::new();
        let s = MyState {
            car: 3,
            flip: true,
            boosting: false,
            pos: Vec3::new(10.0, 0.0, -5.0),
            vel: Vec3::new(0.0, 9.8, 0.0),
            rot: Quat::IDENTITY,
            wheel_spin: 1.25,
            steer_vis: -0.5,
        };
        put_state(&mut b, 4, &s);
        let mut o = 0;
        let ws = get_state(&b, &mut o).expect("a full buffer should decode");
        assert_eq!(ws.slot, 4);
        assert_eq!(ws.car, 3);
        assert!(ws.flip && !ws.boosting);
        assert_eq!(ws.pos, s.pos);
        assert_eq!(ws.vel, s.vel);
        assert_eq!(ws.wheel_spin, s.wheel_spin);
        assert_eq!(ws.steer_vis, s.steer_vis);
        assert_eq!(o, STATE_BYTES, "get_state should consume exactly what put_state wrote");
    }

    #[test]
    fn state_clamps_an_out_of_range_car_index_from_the_wire() {
        // car is a plain u8 on the wire -- a peer running a build with more
        // car models (or a corrupted packet) could send an index this
        // build's own car list can't hold. get_state's own `.min(8)` is the
        // only thing standing between that and an out-of-bounds car lookup
        // somewhere downstream.
        let mut b = Vec::new();
        let s = MyState {
            car: 250,
            flip: false,
            boosting: false,
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            rot: Quat::IDENTITY,
            wheel_spin: 0.0,
            steer_vis: 0.0,
        };
        put_state(&mut b, 0, &s);
        let mut o = 0;
        let ws = get_state(&b, &mut o).unwrap();
        assert_eq!(ws.car, 8, "an out-of-range wire car index should clamp, not pass through raw");
    }

    #[test]
    fn get_state_refuses_a_truncated_buffer_instead_of_panicking() {
        let mut b = Vec::new();
        put_state(&mut b, 0, &MyState {
            car: 0, flip: false, boosting: false,
            pos: Vec3::ZERO, vel: Vec3::ZERO, rot: Quat::IDENTITY,
            wheel_spin: 0.0, steer_vis: 0.0,
        });
        b.truncate(b.len() - 1); // one byte short of a full state record
        let mut o = 0;
        assert!(get_state(&b, &mut o).is_none(), "a short buffer must return None, not panic");
    }

    #[test]
    fn string_round_trips_and_truncates_at_24_bytes() {
        let mut b = Vec::new();
        put_str(&mut b, "PILOT");
        let mut o = 0;
        assert_eq!(get_str(&b, &mut o).as_deref(), Some("PILOT"));

        let mut b2 = Vec::new();
        let long = "A".repeat(40);
        put_str(&mut b2, &long);
        let mut o2 = 0;
        let got = get_str(&b2, &mut o2).unwrap();
        assert_eq!(got.len(), 24, "put_str's own truncation limit should have applied on encode");
    }

    #[test]
    fn get_str_refuses_a_length_prefix_longer_than_the_buffer() {
        // a hand-built/malicious packet could claim a 20-byte name while
        // only supplying 3 bytes after the length prefix.
        let b = vec![20u8, b'h', b'i', b'!'];
        let mut o = 0;
        assert!(get_str(&b, &mut o).is_none(), "should refuse rather than read past the buffer");
    }

    /// END-TO-END, REAL SOCKETS: two full `Net` stacks in one process share
    /// a temp `FileStore` (the same control plane `host()`/`join()` use in
    /// production, just pointed at a test-local path) and run the actual
    /// UDP data plane on loopback -- real `host_with`/`join_with`, real
    /// `tick()`, real HELLO/WELCOME/STATE wire traffic, not a mock.
    ///
    /// This is the automated form of `blackstar-rs`'s own `--sim mp` sim
    /// harness (`src/sim.rs`, `if name == "mp"`), which already proved this
    /// path works (confirmed by running it: "MP LOOPBACK: PASS") but only as
    /// a manual, println!-driven binary nobody runs by accident -- a
    /// regression here would sit unnoticed until someone remembered to run
    /// it by hand. This is that same proof, as a real `assert!` in `cargo
    /// test`. Kept the loop far shorter (steps-based, not the sim's 3
    /// wall-clock seconds of `thread::sleep`) since correctness doesn't need
    /// real-time pacing, only enough ticks for HELLO to reach the host and
    /// WELCOME/STATE to come back.
    /// The announced record carries the id the Net was built for, and a
    /// different game's client is refused -- the two shellkit games are not
    /// wire-compatible (different maps and physics) and the hub launches the
    /// game named in a friend's presence.
    #[test]
    fn a_net_publishes_the_game_it_was_built_for_and_refuses_the_other() {
        let store = std::env::temp_dir().join(format!(
            "shellkit-net-game-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&store);
        let mut host = Net::new("blackstar-rs");
        let st = host.host_with("ALICE", store.clone());
        assert!(st.starts_with("HOSTING"), "{st}");
        let probe = Multiplayer::at_store("blackstar-rs", "PROBE", "PROBE", store.clone());
        let (alice, _) = probe.svc().lookup("ALICE");
        let alice = alice.expect("the host published a profile");
        match &alice.presence {
            netcode::profile::Presence::InGame { game, session } => {
                assert_eq!(game, "blackstar-rs", "presence names the game the Net was built for");
                assert!(session.is_some(), "and the session token");
            }
            other => panic!("host is not InGame: {other:?}"),
        }
        let mut wrong = Net::new("skydribble");
        let st = wrong.join_with("BOB", "ALICE", store.clone());
        assert!(st.starts_with("JOIN FAILED"), "a skydribble client must not dial a blackstar-rs host: {st}");
        let mut right = Net::new("blackstar-rs");
        let st = right.join_with("CAROL", "ALICE", store.clone());
        assert!(st.starts_with("JOINED"), "{st}");
        let _ = std::fs::remove_dir_all(&store);
    }

    #[test]
    fn host_and_join_actually_sync_over_real_loopback_udp() {
        let store = std::env::temp_dir().join(format!(
            "shellkit-net-test-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos() // unique across parallel test threads
        ));
        let _ = std::fs::remove_dir_all(&store);
        let mut a = Net::new("skydribble");
        let mut b = Net::new("skydribble");
        let host_status = a.host_with("ALICE", store.clone());
        assert!(host_status.starts_with("HOSTING"), "host_with failed: {host_status}");
        let join_status = b.join_with("BOB", "ALICE", store.clone());
        assert!(join_status.starts_with("JOINED"), "join_with failed: {join_status}");

        let mut events = Vec::new();
        let mk = |x: f32| MyState {
            car: 0,
            flip: false,
            boosting: false,
            pos: Vec3::new(x, 1.0, 0.0),
            vel: Vec3::ZERO,
            rot: Quat::IDENTITY,
            wheel_spin: 0.0,
            steer_vis: 0.0,
        };
        let mut joined = (false, false); // (host saw BOB, client saw HOST)
        for _ in 0..800 {
            a.tick(0.016, &mk(-4.0), Vec3::ZERO, Vec3::ZERO, Quat::IDENTITY, 20, 9, &mut events);
            b.tick(0.016, &mk(4.0), Vec3::ZERO, Vec3::ZERO, Quat::IDENTITY, 20, 9, &mut events);
            for e in events.drain(..) {
                if let NetEvent::Joined(n) = e {
                    if n == "BOB" { joined.0 = true }
                    if n == "HOST" { joined.1 = true }
                }
            }
            a.smooth(0.016);
            b.smooth(0.016);
            std::thread::sleep(std::time::Duration::from_micros(500));
        }
        let _ = std::fs::remove_dir_all(&store);

        assert!(joined.0, "host never saw BOB's Joined event");
        assert!(joined.1, "client never saw HOST's Joined event");
        assert_eq!(a.player_count(), 2, "host should see itself + 1 peer");
        assert_eq!(b.player_count(), 2, "client should see itself + 1 peer");
        let host_peer_x: Vec<f32> = a.peers().map(|p| p.shown.x).collect();
        let client_peer_x: Vec<f32> = b.peers().map(|p| p.shown.x).collect();
        assert!((host_peer_x[0] - 4.0).abs() < 1.6, "host's view of the client's x drifted: {host_peer_x:?}");
        assert!((client_peer_x[0] - -4.0).abs() < 1.6, "client's view of the host's x drifted: {client_peer_x:?}");
    }
}
