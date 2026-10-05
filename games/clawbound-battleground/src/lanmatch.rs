//! lanmatch -- a LAN match: `netplay::Session` on the den's UDP socket.
//!
//! `lockstep` is the rule (every machine runs the same simulation on the same inputs; a tick
//! advances only when every player's input for it is in), `netplay` is the wire format and the
//! timing (redundant inputs in every packet, an adaptive input delay, time dilation), and both
//! are tested in-process over `NetSim`. Neither touched a socket. This is the thin layer that
//! does: a [`MatchPlan`] every machine agrees on before the first tick (the host sends it, see
//! `lanlobby`), one socket, the peers' addresses, and [`LanMatch::pump`] -- stamp this frame's
//! input, send, receive, step -- which the frame loop calls in place of `Game::tick`.
//!
//! Until c21 the LAN screen ended at discovery: a den could be hosted and joined, and JOIN
//! handed back an address "because the simulation is not yet synchronised between machines".
//! The pieces were all there; this is the match.

use std::net::{SocketAddr, UdpSocket};
use std::time::Instant;

use crate::lockstep::Input;
use crate::netplay::{Packet, Session, Tuning};
use crate::sim::Game;

/// What every machine agrees on before the first tick. The host builds it from its roster and
/// sends each member its own `me`; `peers[0]` is the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchPlan {
    pub seed: u64,
    /// `map::MapId` by its index in `MapId::ALL`.
    pub map: u8,
    /// This machine's player index.
    pub me: usize,
    /// Every player's address as the host saw it, by player index (`peers[me]` is this machine
    /// and is never sent to).
    pub peers: Vec<SocketAddr>,
    /// The host's match rules (`MatchRules::encode`; empty for a normal match): every machine
    /// builds the match under the same rules.
    pub rules: String,
    /// Each player's ancestor (`gods::ROSTER` id), by player index, as the host dealt them
    /// (`picks::deal_seated`): a pick the rules refused became a dealt one.
    pub picks: Vec<u32>,
}

/// Most humans a match takes: five a side.
pub const MAX_PLAYERS: usize = 10;
/// The wire prefix of the plan the host sends (the den's other messages are `CBHELLO`,
/// `CBWELCOME`, `CBFULL`; the match's own packets start with `netplay`'s magic).
pub const START_PREFIX: &str = "CBSTART\t";
/// The host's own entry in the plan's address list: a member fills in the address it joined.
const HOST_TOKEN: &str = "host";

impl MatchPlan {
    pub fn players(&self) -> usize {
        self.peers.len()
    }

    /// Which god each player drives, for a roster of `team_size` a side (gods are laid out
    /// blue first, then red): the first human is blue's first god, the second red's first, then
    /// blue's second, red's second, ... -- so a two-player match is 1v1 and teams fill evenly.
    pub fn god_slots(players: usize, team_size: usize) -> Vec<usize> {
        (0..players.min(MAX_PLAYERS)).map(|i| if i % 2 == 0 { i / 2 } else { team_size + i / 2 }).collect()
    }

    pub fn my_god(&self, team_size: usize) -> usize {
        Self::god_slots(self.players(), team_size)[self.me.min(self.players().saturating_sub(1))]
    }

    /// The line the host sends player `to` (the host writes `host` for itself).
    pub fn encode_for(&self, to: usize) -> String {
        let addrs: Vec<String> = self.peers.iter().enumerate().map(|(i, a)| if i == 0 { HOST_TOKEN.to_string() } else { a.to_string() }).collect();
        let picks: Vec<String> = self.picks.iter().map(|p| p.to_string()).collect();
        let rules: String = self.rules.chars().filter(|c| !c.is_control()).collect();
        format!("{START_PREFIX}{}\t{}\t{to}\t{}\t{}\t{}", self.seed, self.map, addrs.join(","), rules, picks.join(","))
    }

    /// Parse a start line received from `host` (the address the member joined).
    pub fn decode(line: &str, host: SocketAddr) -> Option<MatchPlan> {
        let rest = line.strip_prefix(START_PREFIX)?;
        let mut f = rest.split('\t');
        let seed = f.next()?.trim().parse().ok()?;
        let map = f.next()?.trim().parse().ok()?;
        let me: usize = f.next()?.trim().parse().ok()?;
        let peers: Vec<SocketAddr> = f
            .next()?
            .split(',')
            .map(|a| if a.trim() == HOST_TOKEN { Some(host) } else { a.trim().parse().ok() })
            .collect::<Option<Vec<_>>>()?;
        if peers.is_empty() || peers.len() > MAX_PLAYERS || me >= peers.len() {
            return None;
        }
        let rules = f.next()?.trim().to_string();
        let picks: Vec<u32> = f.next()?.split(',').filter(|t| !t.trim().is_empty()).map(|t| t.trim().parse().ok()).collect::<Option<Vec<_>>>()?;
        if picks.len() != peers.len() || picks.iter().any(|&p| p as usize >= crate::gods::ROSTER_LEN) || f.next().is_some() {
            return None;
        }
        Some(MatchPlan { seed, map, me, peers, rules, picks })
    }

    /// Where each player sits: `(on_blue, seat on that side, pick)` by player index, for
    /// `picks::deal_seated`.
    pub fn seats(&self, team_size: usize) -> Vec<(bool, usize, u32)> {
        let slots = Self::god_slots(self.players(), team_size);
        slots
            .iter()
            .zip(self.picks.iter())
            .map(|(&slot, &pick)| if slot < team_size { (true, slot, pick) } else { (false, slot - team_size, pick) })
            .collect()
    }
}

pub struct LanMatch {
    pub plan: MatchPlan,
    sock: UdpSocket,
    pub session: Session,
    start: Instant,
    /// Frames in a row the sim could not advance (waiting on a peer).
    pub stall_run: u32,
}

impl LanMatch {
    /// `sock` is the den's socket (the host's game socket, a member's join socket): the peers
    /// already know it by this address.
    pub fn new(plan: MatchPlan, sock: UdpSocket, team_size: usize) -> LanMatch {
        let _ = sock.set_nonblocking(true);
        let mut session = Session::new(plan.me, plan.players(), Tuning::TUNED);
        session.lockstep.slots = MatchPlan::god_slots(plan.players(), team_size);
        LanMatch { plan, sock, session, start: Instant::now(), stall_run: 0 }
    }

    /// The gods the humans drive lose their AI and are marked players on EVERY machine (the
    /// sim reads `is_player`: bots focus humans, humans buy for themselves), and this machine
    /// looks through its own (`Game::viewer`, which the sim never reads).
    pub fn prepare(&self, game: &mut Game) {
        for &slot in &self.session.lockstep.slots {
            if let Some(g) = game.gods.get_mut(slot) {
                g.ai = None;
                g.is_player = true;
            }
        }
        game.viewer = Some(self.my_god());
    }

    pub fn my_god(&self) -> usize {
        self.session.lockstep.slots[self.plan.me.min(self.session.lockstep.slots.len().saturating_sub(1))]
    }

    fn now_ms(&self) -> u32 {
        self.start.elapsed().as_millis() as u32
    }

    /// One frame: take in what the peers sent, stamp and step this frame's `input` when the
    /// tick is ready (else count a stall), and send the peers this machine's recent inputs.
    /// Returns whether the simulation advanced.
    pub fn pump(&mut self, game: &mut Game, input: Input, pressed: bool, dt: f32) -> bool {
        let now = self.now_ms();
        let mut buf = [0u8; 1500];
        while let Ok((n, from)) = self.sock.recv_from(&mut buf) {
            if !self.plan.peers.contains(&from) {
                continue; // a stranger, or the den's own HELLO traffic
            }
            if let Some(p) = Packet::decode(&buf[..n]) {
                self.session.receive(&p, now);
            }
        }
        let stepped = if self.session.ready() {
            self.session.stamp(input, pressed, now);
            self.session.step(game, dt, now)
        } else {
            false
        };
        if stepped {
            self.stall_run = 0;
        } else {
            self.stall_run += 1;
        }
        for peer in 0..self.plan.players() {
            if peer == self.plan.me {
                continue;
            }
            for p in self.session.packets_for(peer, now) {
                let _ = self.sock.send_to(&p.encode(), self.plan.peers[peer]);
            }
        }
        stepped
    }

    /// What the HUD says while the match is not advancing, or has forked.
    pub fn status(&self) -> Option<String> {
        if let Some(t) = self.session.stats.desync {
            return Some(format!("DESYNC at tick {t} -- the match has forked"));
        }
        if self.stall_run >= 12 {
            let waiting: Vec<String> = self.session.lockstep.waiting_on().iter().map(|p| format!("player {}", p + 1)).collect();
            return Some(format!("waiting for {}", waiting.join(", ")));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn god_slots_alternate_teams_so_two_players_face_each_other() {
        assert_eq!(MatchPlan::god_slots(1, 5), vec![0]);
        assert_eq!(MatchPlan::god_slots(2, 5), vec![0, 5]);
        assert_eq!(MatchPlan::god_slots(4, 5), vec![0, 5, 1, 6]);
        assert_eq!(MatchPlan::god_slots(10, 5), vec![0, 5, 1, 6, 2, 7, 3, 8, 4, 9]);
        assert_eq!(MatchPlan::god_slots(12, 5).len(), MAX_PLAYERS);
        assert_eq!(MatchPlan::god_slots(2, 1), vec![0, 1], "a 1v1 roster: red's first god is god 1");
    }

    #[test]
    fn the_plan_round_trips_the_wire_with_the_host_filled_in_by_the_member() {
        let host: SocketAddr = "192.168.1.10:41000".parse().unwrap();
        let a: SocketAddr = "192.168.1.11:5001".parse().unwrap();
        let b: SocketAddr = "192.168.1.12:5002".parse().unwrap();
        let plan = MatchPlan { seed: 0xBEEF, map: 2, me: 0, peers: vec![host, a, b], rules: "hp=2 gods=open".into(), picks: vec![3, 7, 3] };
        let line = plan.encode_for(2);
        assert!(line.starts_with(START_PREFIX) && line.contains("host,"), "{line}");
        let got = MatchPlan::decode(&line, host).expect("decodes");
        assert_eq!(got, MatchPlan { me: 2, ..plan.clone() });
        assert_eq!(got.my_god(5), 1, "the third player is blue's second god");
        assert_eq!(got.seats(5), vec![(true, 0, 3), (false, 0, 7), (true, 1, 3)], "host blue seat 0, second player red seat 0, third blue seat 1");
        // a normal match: no rules text, and the plan still carries every pick
        let plain = MatchPlan { rules: String::new(), ..plan.clone() };
        assert_eq!(MatchPlan::decode(&plain.encode_for(1), host).unwrap().rules, "");
        // garbage, a bad index, a wrong prefix, a pick off the roster, a pick count that is not the player count
        assert_eq!(MatchPlan::decode("CBSTART\tx\t2\t0\thost\t\t1", host), None);
        assert_eq!(MatchPlan::decode("CBSTART\t1\t2\t3\thost,1.2.3.4:5\t\t1,2", host), None);
        assert_eq!(MatchPlan::decode("CBSTART\t1\t2\t0\thost,1.2.3.4:5\t\t1,99", host), None);
        assert_eq!(MatchPlan::decode("CBSTART\t1\t2\t0\thost,1.2.3.4:5\t\t1", host), None);
        assert_eq!(MatchPlan::decode("CBHELLO\tbob", host), None);
    }

    /// Two machines, two sockets on loopback, two copies of the match: six hundred frames of
    /// presses in lockstep, both advance, nobody forks, the worlds are bit-identical.
    #[test]
    fn two_machines_on_loopback_run_one_match_in_lockstep() {
        let sa = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let sb = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let (aa, ab) = (sa.local_addr().unwrap(), sb.local_addr().unwrap());
        let host_plan = MatchPlan { seed: 11, map: 0, me: 0, peers: vec![aa, ab], rules: String::new(), picks: vec![0, 1] };
        let member_plan = MatchPlan::decode(&host_plan.encode_for(1), aa).unwrap();
        assert_eq!(member_plan.peers, vec![aa, ab]);
        let mut ma = LanMatch::new(host_plan, sa, 1);
        let mut mb = LanMatch::new(member_plan, sb, 1);
        let (mut ga, mut gb) = (Game::new_teams(&[0], &[1]), Game::new_teams(&[0], &[1]));
        ma.prepare(&mut ga);
        mb.prepare(&mut gb);
        assert_eq!(ga.me(), 0);
        assert_eq!(gb.me(), 1, "the member drives red's first god");
        assert!(ga.gods[1].ai.is_none() && ga.gods[1].is_player, "the other human is a human on this machine too");
        assert!(gb.gods[0].ai.is_none() && gb.gods[0].is_player);

        let mut steps = [0u32; 2];
        for f in 0..600u32 {
            let press = |m: u32| Input { mv: [(f % 50) as i8 - 25, if m == 0 { 40 } else { -40 }], attack: f % 7 == 0, sneak: false, leap: f % 97 == 0, cast: None };
            steps[0] += ma.pump(&mut ga, press(0), f % 30 == 0, 1.0 / 60.0) as u32;
            // the member runs a hair behind, like a real second machine
            if f % 4 != 3 {
                steps[1] += mb.pump(&mut gb, press(1), false, 1.0 / 60.0) as u32;
            }
            ga.events.clear();
            gb.events.clear();
            std::thread::sleep(std::time::Duration::from_micros(300));
        }
        assert!(steps[0] > 200 && steps[1] > 200, "both must advance: {steps:?}");
        assert!(ma.session.stats.desync.is_none() && mb.session.stats.desync.is_none(), "forked: {:?} {:?}", ma.session.stats.desync, mb.session.stats.desync);
        // catch up the slower one, then compare the worlds at the same tick
        for _ in 0..200 {
            if ma.session.lockstep.tick == mb.session.lockstep.tick {
                break;
            }
            let idle = Input::default();
            if ma.session.lockstep.tick < mb.session.lockstep.tick {
                ma.pump(&mut ga, idle, false, 1.0 / 60.0);
            } else {
                mb.pump(&mut gb, idle, false, 1.0 / 60.0);
            }
            std::thread::sleep(std::time::Duration::from_micros(300));
        }
        assert_eq!(ma.session.lockstep.tick, mb.session.lockstep.tick, "could not bring the two to the same tick");
        assert_eq!(crate::lockstep::checksum(&ga), crate::lockstep::checksum(&gb), "the two worlds differ at tick {}", ma.session.lockstep.tick);
        assert!(ma.status().is_none(), "{:?}", ma.status());
    }
}
