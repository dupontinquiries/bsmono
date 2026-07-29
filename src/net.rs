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

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Instant;

use glam::{Quat, Vec3};
use netcode::multiplayer::Multiplayer;

const GAME: &str = "skydribble";
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
    pub fn new() -> Net {
        Net {
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
        let mut mp = Multiplayer::new(GAME, &name, &name);
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
        let mut mp = Multiplayer::new(GAME, &name, &name);
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
        let mut mp = Multiplayer::at_store(GAME, &name, &name, store);
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
        let mut mp = Multiplayer::at_store(GAME, &name, &name, store);
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
