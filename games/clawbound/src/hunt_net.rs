//! hunt_net.rs — the 6v1 hunt with a PERSON at the hunter's controls, over a link.
//!
//! The pack's game (`sim::Game`) is one simulation with six cats and a hunter the
//! FSM drives. This is the second player: a friend joins the host's match and plays
//! the hunter. The shape is **host-authoritative state**, not lockstep: the host
//! runs the simulation exactly as it does alone, applies the joiner's stick to the
//! hunter (`Game::set_hunter_input`), and sends the match's moving state thirty
//! times a second; the joiner sends its stick, mirrors what arrives into its own
//! `Game` (same kind, mode and seed, so the forest and the bushes agree) and renders
//! that. Lockstep would need the hunt to be bit-identical across two machines — it
//! steps with the frame's `dt` and reaches for `sin`/`atan2`, so it is not — and a
//! mirrored state needs nothing but a link.
//!
//! This module is the protocol: the four messages, their bytes, what the host and
//! the joiner remember between datagrams, and the hold on the hunter's stick (a
//! stick that stops arriving hands the hunter back to the AI after [`HOLD_SECS`],
//! so a dropped link turns a human hunter into a bot rather than a statue). It
//! knows nothing about sockets; `app.rs` feeds it datagrams and sends what it
//! hands back, the posture `lockstep`/`wire` take for the battleground.
//!
//! The messages:
//! * `Hello` (joiner → host): "I am here; send the match."
//! * `Header` (host → joiner): what to build — the hunter's kind, the mode, the
//!   seed, the difficulty and the pack's clan. Sent on Hello and again whenever the
//!   host starts a new round (the seed changes), so the joiner rebuilds.
//! * `Input` (joiner → host): the hunter's stick as two i8 axes plus the pounce.
//! * `State` (host → joiner): the state the renderer and the HUD read, packed.

use crate::hunter::Kind;
use crate::sim::{Game, Mode, Outcome, N_CATS};

/// The state goes out this often (the joiner's stick too).
pub const SEND_DT: f32 = 1.0 / 30.0;
/// A stick not refreshed within this many seconds is released: the AI has the hunter.
pub const HOLD_SECS: f32 = 2.0;
/// A joiner that has heard no state for this long shows the link as lost.
pub const LOST_SECS: f32 = 3.0;

const TAG_HELLO: u8 = b'H';
const TAG_HEADER: u8 = b'M';
const TAG_INPUT: u8 = b'I';
const TAG_STATE: u8 = b'S';
/// The state format's version, first byte after the tag; a mismatch is dropped.
const STATE_V: u8 = 1;

/// What the joiner builds its mirror from: the match's parameters and the country it was
/// baked on (`terrain`: the knobs a Prowl or a battleground before it may have left; the
/// bake reads them as found, so the joiner must apply the host's).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Header {
    pub kind: Kind,
    pub mode: Mode,
    pub seed: u32,
    pub difficulty: f32,
    pub clan: u8,
    pub abilities_on: bool,
    pub terrain: crate::world::TerrainPool,
}

impl Header {
    /// The header for the host's live match (built with `seed`), on the country the knobs
    /// describe now -- the ones its bake read.
    pub fn of(g: &Game, seed: u32) -> Header {
        Header {
            kind: g.hunter.kind,
            mode: g.mode,
            seed,
            difficulty: g.difficulty,
            clan: g.clan,
            abilities_on: g.abilities_on,
            terrain: crate::world::terrain_pool(),
        }
    }

    /// The joiner's mirror: the same forest, bushes and roster as the host's.
    pub fn build(&self) -> Game {
        let mut g = Game::new_match_with_terrain(self.kind, self.mode, self.seed, Some(&self.terrain));
        g.difficulty = self.difficulty;
        g.clan = self.clan;
        g.set_abilities_on(self.abilities_on);
        g
    }
}

/// The hunter's stick, as sent: a direction in i8 (the sim normalises it) and the pounce.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct HunterInput {
    pub mx: i8,
    pub mz: i8,
    pub pounce: bool,
}

impl HunterInput {
    pub fn from_axes(mx: f32, mz: f32, pounce: bool) -> HunterInput {
        let q = |v: f32| (v.clamp(-1.0, 1.0) * 127.0).round() as i8;
        HunterInput { mx: q(mx), mz: q(mz), pounce }
    }
    pub fn axes(&self) -> (f32, f32) {
        (self.mx as f32 / 127.0, self.mz as f32 / 127.0)
    }
}

/// A decoded datagram. `State` borrows the packed bytes; `read_state` applies them.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Msg<'a> {
    Hello,
    Header(Header),
    Input(HunterInput),
    State(&'a [u8]),
}

const KINDS: [Kind; 6] = [Kind::Fox, Kind::Dog, Kind::Badger, Kind::Hawk, Kind::Snake, Kind::Shadow];

fn kind_tag(k: Kind) -> u8 {
    KINDS.iter().position(|&x| x == k).unwrap_or(0) as u8
}

pub fn encode_hello() -> Vec<u8> {
    vec![TAG_HELLO]
}

/// Bytes of a header on the wire.
pub const HEADER_LEN: usize = 13 + 14;

pub fn encode_header(h: &Header) -> Vec<u8> {
    let mut out = vec![TAG_HEADER, kind_tag(h.kind), h.mode as u8, h.clan, h.abilities_on as u8];
    out.extend_from_slice(&h.seed.to_le_bytes());
    out.extend_from_slice(&h.difficulty.to_le_bytes());
    let t = &h.terrain;
    out.extend_from_slice(&t.mountain_bias.to_le_bytes());
    out.extend_from_slice(&t.land_lift.to_le_bytes());
    out.push(t.river);
    out.extend_from_slice(&t.seed.to_le_bytes());
    out.push(t.season);
    debug_assert_eq!(out.len(), HEADER_LEN);
    out
}

pub fn encode_input(i: &HunterInput) -> Vec<u8> {
    vec![TAG_INPUT, i.mx as u8, i.mz as u8, i.pounce as u8]
}

pub fn encode_state(g: &Game) -> Vec<u8> {
    let mut out = vec![TAG_STATE];
    write_state(g, &mut out);
    out
}

pub fn decode(b: &[u8]) -> Option<Msg<'_>> {
    match *b.first()? {
        TAG_HELLO => Some(Msg::Hello),
        TAG_HEADER if b.len() == HEADER_LEN => {
            let kind = *KINDS.get(b[1] as usize)?;
            let mode = match b[2] {
                0 => Mode::Rendezvous,
                1 => Mode::Survive,
                _ => return None,
            };
            let mut r = R(b, 5);
            let seed = r.u()?;
            let difficulty = r.f()?;
            let mountain_bias = r.f()?;
            let land_lift = r.f()?;
            let river = r.b()?;
            let tseed = r.u()?;
            let season = r.b()?;
            let terrain = crate::world::TerrainPool { mountain_bias, land_lift, river, seed: tseed, season };
            Some(Msg::Header(Header { kind, mode, seed, difficulty, clan: b[3], abilities_on: b[4] != 0, terrain }))
        }
        TAG_INPUT if b.len() == 4 => Some(Msg::Input(HunterInput { mx: b[1] as i8, mz: b[2] as i8, pounce: b[3] != 0 })),
        TAG_STATE if b.len() > 1 => Some(Msg::State(&b[1..])),
        _ => None,
    }
}

// ── the packed state ────────────────────────────────────────────────────────────

struct W<'a>(&'a mut Vec<u8>);
impl W<'_> {
    fn f(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn b(&mut self, v: u8) {
        self.0.push(v);
    }
}

struct R<'a>(&'a [u8], usize);
impl R<'_> {
    fn f(&mut self) -> Option<f32> {
        let s = self.0.get(self.1..self.1 + 4)?;
        self.1 += 4;
        let v = f32::from_le_bytes([s[0], s[1], s[2], s[3]]);
        v.is_finite().then_some(v)
    }
    fn u(&mut self) -> Option<u32> {
        let s = self.0.get(self.1..self.1 + 4)?;
        self.1 += 4;
        Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn b(&mut self) -> Option<u8> {
        let v = *self.0.get(self.1)?;
        self.1 += 1;
        Some(v)
    }
}

/// Pack what the joiner's renderer and HUD read: the round's clock and score, the
/// hunter's pose and state, every cat's pose, flags and timers. Not the forest, the
/// scent, the wildlife or the host player's combat flourishes — the forest the joiner
/// builds itself from the header, and the rest is the host's own screen.
pub fn write_state(g: &Game, out: &mut Vec<u8>) {
    let mut w = W(out);
    w.b(STATE_V);
    w.u(g.tick);
    w.f(g.time);
    w.u(g.caught);
    w.u(g.escaped);
    w.b(match g.outcome {
        None => 0,
        Some(Outcome::CatsWin) => 1,
        Some(Outcome::HunterWins) => 2,
    });
    w.b(g.shifted as u8);
    w.f(g.danger);
    w.f(g.hunter_stagger);
    let h = &g.hunter;
    w.f(h.x);
    w.f(h.z);
    w.f(h.heading);
    w.b(h.state as u8);
    w.f(h.suspicion);
    w.b(h.carrying as u8);
    w.b(g.cats.len().min(255) as u8);
    for c in &g.cats {
        w.f(c.x);
        w.f(c.z);
        w.f(c.heading);
        let flags = (c.alive as u8)
            | (c.crouched as u8) << 1
            | (c.hidden as u8) << 2
            | (c.moving as u8) << 3
            | (c.safe as u8) << 4
            | (c.climbing as u8) << 5
            | (c.player as u8) << 6;
        w.b(flags);
        for v in [c.stamina, c.dodge_t, c.leap_t, c.carried_t, c.crippled_t, c.climb_t, c.speed, c.snag_t] {
            w.f(v);
        }
    }
}

/// Apply a packed state to a mirror built from the same header. `false` (and the game
/// untouched) for bytes of another version, another cat count, or that end early.
pub fn read_state(g: &mut Game, b: &[u8]) -> bool {
    let mut r = R(b, 0);
    let Some(parsed) = parse_state(&mut r, g.cats.len()) else { return false };
    let (tick, time, caught, escaped, outcome, shifted, danger, stagger, hunter, cats) = parsed;
    g.tick = tick;
    g.time = time;
    g.caught = caught;
    g.escaped = escaped;
    g.outcome = outcome;
    g.shifted = shifted;
    g.danger = danger;
    g.hunter_stagger = stagger;
    let (hx, hz, hh, hs, hsusp, hcarry) = hunter;
    g.hunter.x = hx;
    g.hunter.z = hz;
    g.hunter.heading = hh;
    g.hunter.state = hs;
    g.hunter.suspicion = hsusp;
    g.hunter.carrying = hcarry;
    for (c, s) in g.cats.iter_mut().zip(cats) {
        let (x, z, heading, flags, t) = s;
        c.x = x;
        c.z = z;
        c.heading = heading;
        c.alive = flags & 1 != 0;
        c.crouched = flags & 2 != 0;
        c.hidden = flags & 4 != 0;
        c.moving = flags & 8 != 0;
        c.safe = flags & 16 != 0;
        c.climbing = flags & 32 != 0;
        c.player = flags & 64 != 0;
        [c.stamina, c.dodge_t, c.leap_t, c.carried_t, c.crippled_t, c.climb_t, c.speed, c.snag_t] = t;
    }
    true
}

type HunterState = (f32, f32, f32, crate::hunter::HState, f32, bool);
type CatState = (f32, f32, f32, u8, [f32; 8]);
type Parsed = (u32, f32, u32, u32, Option<Outcome>, bool, f32, f32, HunterState, Vec<CatState>);

fn parse_state(r: &mut R, n_cats: usize) -> Option<Parsed> {
    use crate::hunter::HState;
    if r.b()? != STATE_V {
        return None;
    }
    let tick = r.u()?;
    let time = r.f()?;
    let caught = r.u()?;
    let escaped = r.u()?;
    let outcome = match r.b()? {
        0 => None,
        1 => Some(Outcome::CatsWin),
        2 => Some(Outcome::HunterWins),
        _ => return None,
    };
    let shifted = r.b()? != 0;
    let danger = r.f()?;
    let stagger = r.f()?;
    let hx = r.f()?;
    let hz = r.f()?;
    let hh = r.f()?;
    let hs = match r.b()? {
        0 => HState::Patrol,
        1 => HState::Alert,
        2 => HState::Track,
        3 => HState::Chase,
        4 => HState::Search,
        _ => return None,
    };
    let hsusp = r.f()?;
    let hcarry = r.b()? != 0;
    let n = r.b()? as usize;
    if n != n_cats {
        return None;
    }
    let mut cats = Vec::with_capacity(n);
    for _ in 0..n {
        let x = r.f()?;
        let z = r.f()?;
        let heading = r.f()?;
        let flags = r.b()?;
        let mut t = [0.0f32; 8];
        for v in t.iter_mut() {
            *v = r.f()?;
        }
        cats.push((x, z, heading, flags, t));
    }
    Some((tick, time, caught, escaped, outcome, shifted, danger, stagger, (hx, hz, hh, hs, hsusp, hcarry), cats))
}

/// Bytes of one packed state for the stock pack: the number that decides whether thirty
/// a second is fine on a phone's uplink (it is: under 10 KB/s).
pub fn state_len() -> usize {
    1 + 4 + 4 + 4 + 4 + 1 + 1 + 4 + 4 + (4 + 4 + 4 + 1 + 4 + 1) + 1 + N_CATS * (12 + 1 + 32)
}

// ── what each end remembers ───────────────────────────────────────────────────────

/// The host's side: who holds the hunter's stick and when the state is due.
#[derive(Debug, Default)]
pub struct Host {
    /// A joiner has said Hello (the state goes out from then on).
    pub joined: bool,
    /// The header should go out: on Hello, and after every new round.
    pub header_due: bool,
    /// The last stick received, and when.
    held: Option<(HunterInput, f32)>,
    acc: f32,
    pub last_heard: f32,
}

impl Host {
    /// A datagram from the joiner, at `now` seconds.
    pub fn on_message(&mut self, m: Msg, now: f32) {
        self.last_heard = now;
        match m {
            Msg::Hello => {
                self.joined = true;
                self.header_due = true;
            }
            Msg::Input(i) => self.held = Some((i, now)),
            Msg::Header(_) | Msg::State(_) => {} // the host's own kind of message, echoed: ignored
        }
    }

    /// The stick to apply before this step, while it is fresh.
    pub fn hunter_input(&self, now: f32) -> Option<HunterInput> {
        self.held.filter(|&(_, at)| now - at <= HOLD_SECS).map(|(i, _)| i)
    }

    /// A new round started: the joiner needs the new header.
    pub fn new_round(&mut self) {
        if self.joined {
            self.header_due = true;
        }
    }

    /// Whether a state should go out this frame (thirty a second while joined).
    pub fn state_due(&mut self, dt: f32) -> bool {
        if !self.joined {
            return false;
        }
        self.acc += dt;
        if self.acc >= SEND_DT {
            self.acc = 0.0;
            true
        } else {
            false
        }
    }
}

/// The joiner's side: the header it is playing and how fresh the mirror is.
#[derive(Debug, Default)]
pub struct Client {
    pub header: Option<Header>,
    pub last_state: Option<f32>,
    acc: f32,
    pub states: u32,
}

impl Client {
    /// A datagram from the host at `now`. The first header, or one with a new seed, replaces
    /// `game` with the mirror it describes; a state lands in the mirror (and is ignored until a
    /// header has arrived -- a state for a match this end has not built is noise). Returns
    /// whether `game` changed.
    pub fn on_message(&mut self, m: Msg, now: f32, game: &mut Game) -> bool {
        match m {
            Msg::Header(h) => {
                if self.header != Some(h) {
                    self.header = Some(h);
                    *game = h.build();
                    self.states = 0;
                    return true;
                }
                false
            }
            Msg::State(b) => {
                if self.header.is_none() {
                    return false;
                }
                if read_state(game, b) {
                    self.last_state = Some(now);
                    self.states += 1;
                    true
                } else {
                    false
                }
            }
            Msg::Hello | Msg::Input(_) => false,
        }
    }

    /// Whether the stick should go out this frame.
    pub fn input_due(&mut self, dt: f32) -> bool {
        self.acc += dt;
        if self.acc >= SEND_DT {
            self.acc = 0.0;
            true
        } else {
            false
        }
    }

    /// A line for the HUD: waiting, playing, or lost.
    pub fn status(&self, now: f32) -> &'static str {
        match (self.header, self.last_state) {
            (None, _) => "waiting for the host's match",
            (Some(_), None) => "match received, waiting for the first state",
            (Some(_), Some(at)) if now - at > LOST_SECS => "link lost: no state from the host",
            _ => "playing the hunter",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The baseline country (what the hunt bakes in a fresh process): the tests that BUILD
    /// from this bake the same grid the sim's own tests bake, so they add no new country to
    /// the suite's parallel re-bakes of the one global grid.
    fn header() -> Header {
        let terrain = crate::world::TerrainPool { mountain_bias: 1.0, land_lift: 0.0, river: 0, seed: 0, season: 1 };
        Header { kind: Kind::Badger, mode: Mode::Survive, seed: 0xC0FFEE, difficulty: 1.25, clan: 3, abilities_on: true, terrain }
    }

    /// A country no other test bakes: for the codec only, never built.
    fn odd_country() -> Header {
        Header { terrain: crate::world::TerrainPool { mountain_bias: 1.3, land_lift: 0.5, river: 1, seed: 9, season: 2 }, ..header() }
    }

    #[test]
    fn every_message_round_trips_through_its_bytes() {
        assert_eq!(decode(&encode_hello()), Some(Msg::Hello));
        assert_eq!(decode(&encode_header(&header())), Some(Msg::Header(header())));
        assert_eq!(decode(&encode_header(&odd_country())), Some(Msg::Header(odd_country())), "the country rides along");
        let i = HunterInput::from_axes(0.7, -1.3, true);
        assert_eq!(i, HunterInput { mx: 89, mz: -127, pounce: true });
        assert_eq!(decode(&encode_input(&i)), Some(Msg::Input(i)));
        let (ax, az) = i.axes();
        assert!((ax - 0.7).abs() < 0.01 && (az + 1.0).abs() < 1e-6);
        for k in KINDS {
            let h = Header { kind: k, ..header() };
            assert_eq!(decode(&encode_header(&h)), Some(Msg::Header(h)), "{:?}", k);
        }
        // junk and truncation are None, not a panic
        assert_eq!(decode(&[]), None);
        assert_eq!(decode(&[b'?']), None);
        assert_eq!(decode(&encode_header(&header())[..7]), None);
        assert_eq!(decode(&encode_header(&header())[..13]), None, "a header without its country");
        assert_eq!(decode(&[TAG_INPUT, 0]), None);
        let mut bad = encode_header(&header());
        bad[1] = 9; // no such kind
        assert_eq!(decode(&bad), None);
    }

    #[test]
    fn a_state_mirrors_the_round_onto_a_game_built_from_the_header() {
        let h = header();
        let mut host = h.build();
        for _ in 0..90 {
            host.set_hunter_input(1.0, 0.3, false);
            host.step(1.0 / 30.0);
        }
        host.cats[2].alive = false;
        host.cats[4].hidden = true;
        host.cats[4].climb_t = 0.8;
        host.caught = 1;
        host.danger = 0.42;
        let bytes = encode_state(&host);
        assert_eq!(bytes.len(), 1 + state_len(), "the documented size");
        let Some(Msg::State(s)) = decode(&bytes) else { panic!("not a state") };
        let mut mirror = h.build();
        assert!(read_state(&mut mirror, s));
        assert_eq!(mirror.tick, host.tick);
        assert_eq!(mirror.time, host.time);
        assert_eq!(mirror.caught, 1);
        assert_eq!((mirror.hunter.x, mirror.hunter.z, mirror.hunter.heading), (host.hunter.x, host.hunter.z, host.hunter.heading));
        assert_eq!(mirror.hunter.state as u8, host.hunter.state as u8);
        for (a, b) in mirror.cats.iter().zip(&host.cats) {
            assert_eq!((a.x, a.z, a.heading), (b.x, b.z, b.heading));
            assert_eq!((a.alive, a.hidden, a.safe, a.moving, a.player), (b.alive, b.hidden, b.safe, b.moving, b.player));
            assert_eq!((a.climb_t, a.stamina, a.leap_t), (b.climb_t, b.stamina, b.leap_t));
        }
        assert!(!mirror.cats[2].alive && mirror.cats[4].hidden);
        assert_eq!(mirror.danger, 0.42);
        // the forest is the header's, not the wire's
        assert_eq!(mirror.bushes.len(), host.bushes.len());
        assert_eq!(mirror.trees, host.trees);
    }

    #[test]
    fn a_short_or_foreign_state_leaves_the_mirror_alone() {
        let h = header();
        let host = h.build();
        let bytes = encode_state(&host);
        let mut mirror = h.build();
        mirror.caught = 7;
        assert!(!read_state(&mut mirror, &bytes[1..bytes.len() - 3]), "truncated");
        assert_eq!(mirror.caught, 7);
        let mut other = bytes.clone();
        other[1] = STATE_V + 1;
        assert!(!read_state(&mut mirror, &other[1..]), "another version");
        assert_eq!(mirror.caught, 7);
        let mut fewer = h.build();
        fewer.cats.pop();
        assert!(!read_state(&mut fewer, &bytes[1..]), "a different cat count");
    }

    /// The whole exchange over real loopback UDP, in the order the app runs it: the joiner
    /// knocks, the host answers with its match, the joiner's stick lands on the host's hunter,
    /// and the mirror follows the host's world -- so the only thing left untested is the
    /// window glue around it.
    #[test]
    fn a_joiner_over_loopback_drives_the_hosts_hunter_and_mirrors_its_world() {
        use std::net::UdpSocket;
        let hs = UdpSocket::bind("127.0.0.1:0").unwrap(); // the host: bound, not connected
        hs.set_nonblocking(true).unwrap();
        let cs = UdpSocket::bind("127.0.0.1:0").unwrap(); // the joiner: connected to the host
        cs.connect(hs.local_addr().unwrap()).unwrap();
        cs.set_nonblocking(true).unwrap();
        let seed = 77;
        // the host's country, pinned (the suite bakes other countries in parallel; the
        // app has one match and reads the knobs its bake used -- `Header::of`)
        let pool = crate::world::TerrainPool { mountain_bias: 1.0, land_lift: 0.0, river: 0, seed: 0, season: 1 };
        let mut host_game = Game::new_match_with_terrain(Kind::Dog, Mode::Rendezvous, seed, Some(&pool));
        host_game.bushes.clear();
        host_game.hunter.x = 0.0;
        host_game.hunter.z = 0.0;
        // left to itself the AI would patrol the objective, put far to -x, with no cat or
        // trail to pull it anywhere: only the joiner's stick can take the hunter +x
        host_game.objective = (-80.0, 0.0);
        for c in host_game.cats.iter_mut() { c.x = 200.0; c.z = 200.0; c.hidden = true; c.moving = false; c.player = false; c.goal = (200.0, 200.0); }
        host_game.scent.clear();
        let mut host = Host::default();
        let mut peer = None;
        let mut client = Client::default();
        let mut mirror = Game::new_6v1(Kind::Fox); // whatever the joiner had open
        let mut buf = [0u8; 2048];
        let dt = 1.0 / 60.0;
        let mut now = 0.0f32;
        let mut hello_acc = 1.0f32;
        for _ in 0..240 {
            now += dt;
            // ── the host's frame ──
            while let Ok((n, from)) = hs.recv_from(&mut buf) {
                if let Some(m) = decode(&buf[..n]) {
                    if matches!(m, Msg::Hello) {
                        peer = Some(from);
                    }
                    host.on_message(m, now);
                }
            }
            if let Some(i) = host.hunter_input(now) {
                let (mx, mz) = i.axes();
                host_game.set_hunter_input(mx, mz, i.pounce);
            }
            host_game.step(dt);
            if let Some(p) = peer {
                if host.header_due {
                    host.header_due = false;
                    let header = Header { terrain: pool, ..Header::of(&host_game, seed) };
                    hs.send_to(&encode_header(&header), p).unwrap();
                }
                if host.state_due(dt) {
                    hs.send_to(&encode_state(&host_game), p).unwrap();
                }
            }
            // ── the joiner's frame ──
            while let Ok(n) = cs.recv(&mut buf) {
                if let Some(m) = decode(&buf[..n]) {
                    client.on_message(m, now, &mut mirror);
                }
            }
            hello_acc += dt;
            if client.header.is_none() && hello_acc >= 1.0 {
                hello_acc = 0.0;
                cs.send(&encode_hello()).unwrap();
            }
            if client.header.is_some() && client.input_due(dt) {
                cs.send(&encode_input(&HunterInput::from_axes(1.0, 0.0, false))).unwrap(); // the person walks +x
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(host.joined, "the Hello arrived");
        assert_eq!(client.header.map(|h| (h.kind, h.seed)), Some((Kind::Dog, seed)), "the match arrived");
        assert!(client.states > 60, "states kept coming ({})", client.states);
        assert!(host_game.hunter.x > 20.0, "the host's hunter walked the joiner's way, not the AI's ({})", host_game.hunter.x);
        assert!(mirror.hunter.x > 20.0, "and the mirror shows it ({})", mirror.hunter.x);
        assert_eq!(mirror.hunter.kind, Kind::Dog);
        assert!((mirror.hunter.x - host_game.hunter.x).abs() < 2.0, "the mirror is a few states behind at most ({} vs {})", mirror.hunter.x, host_game.hunter.x);
        assert_eq!(mirror.trees, host_game.trees, "the same forest");
        assert_eq!(client.status(now), "playing the hunter");
    }

    #[test]
    fn the_host_holds_a_fresh_stick_and_releases_a_stale_one() {
        let mut host = Host::default();
        assert!(!host.state_due(1.0), "nothing goes out before a Hello");
        host.on_message(Msg::Hello, 0.0);
        assert!(host.joined && host.header_due);
        let i = HunterInput { mx: 127, mz: 0, pounce: true };
        host.on_message(Msg::Input(i), 1.0);
        assert_eq!(host.hunter_input(1.5), Some(i));
        assert_eq!(host.hunter_input(1.0 + HOLD_SECS + 0.01), None, "a stale stick is the AI's again");
        // thirty states a second once joined
        let mut sent = 0;
        for _ in 0..60 {
            if host.state_due(1.0 / 60.0) {
                sent += 1;
            }
        }
        assert!((29..=31).contains(&sent), "{sent}");
        host.header_due = false;
        host.new_round();
        assert!(host.header_due, "a new round resends the header");
    }

    #[test]
    fn the_joiner_builds_on_a_header_rebuilds_on_a_new_seed_and_mirrors_states() {
        let mut c = Client::default();
        let mut game = Game::new_6v1(Kind::Fox); // whatever this end had open before the join
        assert_eq!(c.status(0.0), "waiting for the host's match");
        let h = header();
        // a state before any header is noise: nothing to mirror it into
        let stray = encode_state(&h.build());
        let Some(m) = decode(&stray) else { panic!() };
        assert!(!c.on_message(m, 0.0, &mut game));
        assert_eq!(game.hunter.kind, Kind::Fox);
        assert!(c.on_message(Msg::Header(h), 0.0, &mut game));
        assert_eq!((game.hunter.kind, game.mode, game.clan, game.difficulty), (Kind::Badger, Mode::Survive, 3, 1.25));
        assert!(game.abilities_on);
        assert!(!c.on_message(Msg::Header(h), 0.1, &mut game), "the same header again builds nothing");
        // a state from a host that stepped lands in the mirror
        let mut host = h.build();
        for _ in 0..30 {
            host.step(1.0 / 30.0);
        }
        let bytes = encode_state(&host);
        let Some(m) = decode(&bytes) else { panic!() };
        assert!(c.on_message(m, 0.5, &mut game));
        assert_eq!(game.tick, host.tick);
        assert_eq!(c.status(0.6), "playing the hunter");
        assert_eq!(c.status(0.5 + LOST_SECS + 0.1), "link lost: no state from the host");
        // a new round: the header's seed changes and the mirror is rebuilt
        let h2 = Header { seed: 99, ..h };
        assert!(c.on_message(Msg::Header(h2), 1.0, &mut game));
        assert_eq!(game.tick, 0);
        assert_eq!(c.states, 0);
    }
}
