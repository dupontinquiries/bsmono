//! lanlobby — the state behind the LAN screen.
//!
//! Headless: `netcode::lan` does the sockets, this decides what the screen
//! shows and what its buttons mean, and `ui::draw_lan` only paints it. So the
//! screen's behaviour is testable without a window.
//!
//! Discovery, hosting a den and joining one are real and work over a real
//! network; the den counts who is in it. Since c21 the host can START: it
//! sends every member a `lanmatch::MatchPlan` (seed, map, who is which
//! player, everyone's address) over the den's socket, and each machine
//! hands the plan and that socket to `lanmatch::LanMatch`, which runs the
//! match in lockstep. The screen used to say JOIN stopped at the address
//! "because the simulation is not yet synchronised between machines".

use netcode::lan::{Announcer, Beacon, Browser, Found};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

/// A member who has not said HELLO for this long has left.
pub const MEMBER_TIMEOUT: Duration = Duration::from_millis(4000);
const HELLO_EVERY: Duration = Duration::from_millis(1000);

pub const GAME: &str = "clawbound";
/// Bump when the sim changes in a way two builds could not share a match.
pub const PROTO: u16 = 1;
pub const SLOTS: u8 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub players: u8,
    pub slots: u8,
    pub addr: SocketAddr,
    pub joinable: bool,
    /// Why not, in the player's words.
    pub note: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Back,
    /// The player picked a lobby. The caller owns what happens next.
    Join(SocketAddr),
    /// The host pressed START (`LanLobby::start`): the caller picks the seed and the map.
    Start,
}

pub struct LanLobby {
    pub my_name: String,
    my_id: String,
    browser: Result<Browser, String>,
    announcer: Option<Announcer>,
    pub hosting: bool,
    /// The den's GAME socket. Bound when hosting starts, and its real port is
    /// what the beacon advertises. The app used to pass port 0 here, which
    /// `Beacon::decode` rejects -- so a den hosted from the actual game never
    /// appeared on anyone's screen, and the test passed a made-up 41000.
    game_sock: Option<UdpSocket>,
    /// Who has joined this den: address, name, last heard, their ancestor (`gods::ROSTER` id,
    /// said in every HELLO, so a member changes it by changing it).
    pub roster: Vec<(SocketAddr, String, Instant, u32)>,
    /// This machine's ancestor for the match (the select screen's choice; the LAN screen's
    /// arrows change it). A member says it in every HELLO; the host seats it as player 0.
    pub my_pick: u32,
    /// The den this player has joined, and what the host last said.
    pub joined: Option<(SocketAddr, String)>,
    join_sock: Option<UdpSocket>,
    last_hello: Option<Instant>,
    /// Kept alive while a bsgames handoff session is: dropping it would stop
    /// the DHT announcement.
    mp: Option<netcode::multiplayer::Multiplayer>,
    /// What the bsgames handoff came to, shown above the status line.
    pub handoff_note: Option<String>,
    pub rows: Vec<Row>,
    /// One line for the screen's footer: what is going on, or what is wrong.
    pub status: String,
    /// The match this den is starting -- set on the host by `start`, on a
    /// member when the host's plan arrives; taken by the app with `take_start`.
    start: Option<crate::lanmatch::MatchPlan>,
}

impl LanLobby {
    pub fn open(my_name: &str) -> LanLobby {
        Self::open_on(my_name, netcode::lan::LAN_PORT)
    }

    pub fn open_on(my_name: &str, port: u16) -> LanLobby {
        let my_id = format!("{my_name}-{:x}", std::process::id());
        let browser = Browser::on_port(GAME, PROTO, &my_id, port).map_err(|e| {
            // The one failure a player will actually hit: a second copy on
            // the same machine. Say THAT, not "os error 98".
            if e.kind() == std::io::ErrorKind::AddrInUse {
                "Another copy of the game on this machine is already listening for LAN games.".to_string()
            } else {
                format!("Cannot listen for LAN games: {e}")
            }
        });
        let status = browser.as_ref().err().cloned().unwrap_or_else(|| "Searching the local network...".into());
        LanLobby { my_name: my_name.into(), my_id, browser, announcer: None, hosting: false, game_sock: None, roster: Vec::new(), my_pick: 0, joined: None, join_sock: None, last_hello: None, mp: None, handoff_note: None, rows: Vec::new(), status, start: None }
    }

    pub fn port(&self) -> Option<u16> {
        self.browser.as_ref().ok().map(|b| b.port())
    }

    pub fn set_hosting(&mut self, on: bool) {
        self.hosting = on;
        if !on {
            self.announcer = None;
            self.game_sock = None;
            self.roster.clear();
        } else if self.announcer.is_none() {
            self.bind_game_sock();
            match Announcer::new() {
                Ok(a) => self.announcer = Some(a),
                Err(e) => {
                    self.hosting = false;
                    self.status = format!("Cannot announce on this network: {e}");
                }
            }
        }
    }

    /// For tests and unusual networks: announce somewhere specific.
    pub fn host_to(&mut self, to: Vec<SocketAddr>) {
        self.announcer = Announcer::to(to).ok();
        self.hosting = self.announcer.is_some();
        self.bind_game_sock();
    }

    /// bsgames launched us to HOST for a friend. The den's game socket IS the
    /// socket `netcode::multiplayer` announces over the DHT, so a friend who
    /// joins by id and a stranger who finds the den on the LAN screen arrive
    /// at the same roster. Also announces on the LAN.
    pub fn host_for_handoff(&mut self, mut mp: netcode::multiplayer::Multiplayer) -> Result<String, String> {
        let token = mp.host("0.0.0.0:0")?;
        let sock = mp.socket.as_ref().ok_or("no socket")?.try_clone().map_err(|e| e.to_string())?;
        sock.set_nonblocking(true).map_err(|e| e.to_string())?;
        self.game_sock = Some(sock);
        self.mp = Some(mp);
        self.set_hosting(true);
        Ok(token)
    }

    /// bsgames launched us to JOIN a friend: resolve them through the DHT,
    /// then say HELLO to the address they announced, exactly as a LAN join.
    pub fn join_for_handoff(&mut self, mut mp: netcode::multiplayer::Multiplayer, friend_id: &str) -> Result<SocketAddr, String> {
        let addr: SocketAddr = mp.join(friend_id)?.parse().map_err(|e| format!("{friend_id} announced an unusable address: {e}"))?;
        self.mp = Some(mp);
        self.join(addr);
        Ok(addr)
    }

    /// What a bsgames handoff becomes: a den, hosted or joined, with the
    /// outcome in `status` either way. It never fails outright -- if the
    /// friend cannot be resolved, the player is left on the LAN screen with
    /// the reason in words and can still find the den by browsing, which is
    /// better than the solo match they used to be dropped into.
    pub fn open_for_handoff(hosting: bool, peer: Option<&str>, mp: netcode::multiplayer::Multiplayer, port: u16) -> LanLobby {
        let mut l = LanLobby::open_on(&mp.me.name.clone(), port);
        let outcome = if hosting {
            l.host_for_handoff(mp).map(|token| format!("Hosting for your friends as {token}."))
        } else {
            match peer {
                Some(id) => l.join_for_handoff(mp, id).map(|a| format!("Joining {id} at {a}...")),
                None => Err("bsgames asked to join, but did not say whom.".into()),
            }
        };
        l.handoff_note = Some(match outcome {
            Ok(s) => s,
            Err(e) => format!("Could not set that up: {e}. You can still look for the den below."),
        });
        l
    }

    fn bind_game_sock(&mut self) {
        if self.game_sock.is_none() {
            self.game_sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).and_then(|s| s.set_nonblocking(true).map(|_| s)).ok();
        }
    }

    pub fn players(&self) -> u8 {
        1 + self.roster.len().min(SLOTS as usize - 1) as u8
    }

    /// `CBHELLO\t<name>\t<pick>`.
    pub fn hello(name: &str, pick: u32) -> String {
        format!("CBHELLO\t{name}\t{pick}")
    }

    /// The name and the pick out of a HELLO's body (after the prefix): a pick that is missing
    /// (an older build) or off the roster is 0.
    pub fn parse_hello(rest: &str) -> (String, u32) {
        let mut f = rest.split('\t');
        let name: String = f.next().unwrap_or("").chars().filter(|c| !c.is_control()).take(24).collect();
        let pick = f.next().and_then(|p| p.trim().parse::<u32>().ok()).filter(|&p| (p as usize) < crate::gods::ROSTER_LEN).unwrap_or(0);
        (name, pick)
    }

    /// The den's members as the screen lists them: name and ancestor id, the host first.
    pub fn members(&self) -> Vec<(String, u32)> {
        let mut v = vec![(self.my_name.clone(), self.my_pick)];
        v.extend(self.roster.iter().map(|m| (m.1.clone(), m.3)));
        v
    }

    /// Ask to join a den. Keeps saying HELLO once a second, which is also
    /// the keep-alive: stop, and the host forgets you.
    pub fn join(&mut self, host: SocketAddr) {
        self.join_sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).and_then(|s| s.set_nonblocking(true).map(|_| s)).ok();
        self.joined = Some((host, "asking...".into()));
        self.last_hello = None;
    }

    pub fn leave(&mut self) {
        self.joined = None;
        self.join_sock = None;
    }

    /// Once a frame.
    pub fn tick(&mut self, now: Instant) {
        // Hosting: answer HELLOs on the game socket, age the roster, announce.
        if let (true, Some(sock)) = (self.hosting, self.game_sock.as_ref()) {
            let mut buf = [0u8; 256];
            for _ in 0..32 {
                let Ok((n, from)) = sock.recv_from(&mut buf) else { break };
                let Some(rest) = std::str::from_utf8(&buf[..n]).ok().and_then(|t| t.strip_prefix("CBHELLO\t")) else { continue };
                // `CBHELLO\t<name>\t<pick>`; a HELLO without a pick (an older build) is pick 0
                let (name, pick) = Self::parse_hello(rest);
                let room = self.roster.len() + 1 < SLOTS as usize;
                if let Some(m) = self.roster.iter_mut().find(|m| m.0 == from) {
                    m.2 = now;
                    m.3 = pick;
                } else if room {
                    self.roster.push((from, name, now, pick));
                } else {
                    let _ = sock.send_to(b"CBFULL", from);
                    continue;
                }
                let _ = sock.send_to(format!("CBWELCOME\t{}\t{}", 1 + self.roster.len(), SLOTS).as_bytes(), from);
            }
            self.roster.retain(|m| now.duration_since(m.2) < MEMBER_TIMEOUT);
        }
        let (port, players) = (self.game_sock.as_ref().and_then(|s| s.local_addr().ok()).map_or(0, |a| a.port()), self.players());
        if let (true, Some(a)) = (self.hosting && port != 0, self.announcer.as_mut()) {
            a.tick(&Beacon { game: GAME.into(), host_id: self.my_id.clone(), name: format!("{}'s den", self.my_name), port, players, slots: SLOTS, proto: PROTO }, now);
        }
        // Joined somewhere: HELLO once a second, and listen for the answer.
        if let (Some((host, said)), Some(sock)) = (self.joined.as_mut(), self.join_sock.as_ref()) {
            if self.last_hello.map_or(true, |t| now.duration_since(t) >= HELLO_EVERY) {
                let _ = sock.send_to(Self::hello(&self.my_name, self.my_pick).as_bytes(), *host);
                self.last_hello = Some(now);
            }
            let mut buf = [0u8; 512]; // a start plan with ten addresses is longer than the 64 the den's replies needed
            while let Ok((n, from)) = sock.recv_from(&mut buf) {
                if from != *host {
                    continue;
                }
                let text = std::str::from_utf8(&buf[..n]).unwrap_or("");
                if let Some(plan) = crate::lanmatch::MatchPlan::decode(text, *host) {
                    *said = format!("the match is starting -- you are player {}", plan.me + 1);
                    self.start = Some(plan);
                    continue;
                }
                match text.split('\t').collect::<Vec<_>>().as_slice() {
                    ["CBWELCOME", p, s] => *said = format!("in the den ({p}/{s})"),
                    ["CBFULL"] => *said = "the den is full".into(),
                    _ => {}
                }
            }
        }
        let Ok(b) = self.browser.as_mut() else { return };
        let found: Vec<Found> = b.poll(now).to_vec();
        self.rows = found
            .iter()
            .map(|f| {
                let joinable = b.joinable(f);
                let here = self.joined.as_ref().is_some_and(|j| j.0 == f.addr);
                let joinable = joinable && !here;
                let note = if here {
                    // Seen in the real window: a friend already in the den was
                    // still offered JOIN for it.
                    "you are here"
                } else if f.beacon.proto != PROTO {
                    "different version"
                } else if f.beacon.players >= f.beacon.slots {
                    "full"
                } else {
                    ""
                };
                Row { name: f.beacon.name.clone(), players: f.beacon.players, slots: f.beacon.slots, addr: f.addr, joinable, note }
            })
            .collect();
        if let Some((host, said)) = &self.joined {
            self.status = format!("{host}: {said}");
            return;
        }
        self.status = match (self.hosting, self.rows.len()) {
            (true, _) => format!("Hosting as \"{}'s den\" -- {}/{} here, visible to this network.", self.my_name, self.players(), SLOTS),
            (false, 0) => "Searching the local network... no games yet.".into(),
            (false, n) => format!("{n} game{} on this network.", if n == 1 { "" } else { "s" }),
        };
    }

    /// Can the host press START: hosting, with at least one member in the den.
    pub fn can_start(&self) -> bool {
        self.hosting && self.game_sock.is_some() && !self.roster.is_empty()
    }

    /// The host starts the match: builds the plan from the roster (the host is player 0, the
    /// members in the order they arrived) and sends each member its own copy, twice (UDP; the
    /// member keeps saying HELLO until it has one, so a lost plan shows as a member still "in
    /// the den"). Returns the host's own plan, also left for `take_start`.
    pub fn start(&mut self, seed: u64, map: u8, rules: &str) -> Option<crate::lanmatch::MatchPlan> {
        if !self.can_start() {
            return None;
        }
        let sock = self.game_sock.as_ref()?;
        let me_addr = sock.local_addr().ok()?;
        let mut peers = vec![me_addr];
        let mut picks = vec![self.my_pick];
        for m in self.roster.iter().take(crate::lanmatch::MAX_PLAYERS - 1) {
            peers.push(m.0);
            picks.push(m.3);
        }
        let plan = crate::lanmatch::MatchPlan { seed, map, me: 0, peers, rules: rules.to_string(), picks };
        for (i, addr) in plan.peers.iter().enumerate().skip(1) {
            let line = plan.encode_for(i);
            for _ in 0..2 {
                let _ = sock.send_to(line.as_bytes(), addr);
            }
        }
        self.status = format!("Starting with {} player(s)...", plan.players());
        self.start = Some(plan.clone());
        Some(plan)
    }

    /// The plan this den is starting, with the socket the match runs on (the host's game
    /// socket, a member's join socket -- the address the others know), once. The lobby keeps
    /// its own handle but must not be ticked while the match reads the socket.
    pub fn take_start(&mut self) -> Option<(crate::lanmatch::MatchPlan, UdpSocket)> {
        let plan = self.start.take()?;
        let sock = if plan.me == 0 { self.game_sock.as_ref() } else { self.join_sock.as_ref() }?;
        let sock = sock.try_clone().ok()?;
        Some((plan, sock))
    }

    /// A click on row `i`. Unjoinable rows do nothing: the button is drawn
    /// disabled, and this is the second lock on the same door.
    pub fn click_join(&self, i: usize) -> Action {
        match self.rows.get(i) {
            Some(r) if r.joinable => Action::Join(r.addr),
            _ => Action::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::time::Duration;

    /// Two lobbies on one machine, real sockets: one hosts, the other sees
    /// it, can join it, and loses it when the host stops.
    #[test]
    fn a_hosted_den_appears_on_the_other_screen_and_leaves_when_hosting_stops() {
        let mut seeker = LanLobby::open_on("Ash", 0);
        let mut host = LanLobby::open_on("Bramble", 0);
        let to = vec![(Ipv4Addr::LOCALHOST, seeker.port().expect("seeker is listening")).into()];
        let t0 = Instant::now();
        seeker.tick(t0);
        assert!(seeker.rows.is_empty() && seeker.status.contains("no games"), "{}", seeker.status);
        assert_eq!(seeker.click_join(0), Action::None, "nothing to click");

        host.host_to(to);
        host.tick(t0);
        assert!(host.status.contains("Hosting"), "{}", host.status);
        for _ in 0..50 {
            seeker.tick(t0);
            if !seeker.rows.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        assert_eq!(seeker.rows.len(), 1, "{}", seeker.status);
        let r = &seeker.rows[0];
        assert_eq!((r.name.as_str(), r.players, r.slots, r.joinable, r.note), ("Bramble's den", 1, SLOTS, true, ""));
        // The advertised port is the den's REAL game socket -- not the 0 the
        // app used to pass, which no browser would ever have listed.
        let Action::Join(addr) = seeker.click_join(0) else { panic!("not joinable") };
        assert!(addr.port() != 0 && addr.ip() == Ipv4Addr::LOCALHOST);

        // JOIN reaches it: the host counts us, tells us so, and says so to
        // the whole network in its next beacon.
        seeker.join(addr);
        let t1 = t0 + netcode::lan::ANNOUNCE_EVERY;
        for _ in 0..60 {
            seeker.tick(t1);
            host.tick(t1);
            if host.players() == 2 && seeker.status.contains("in the den (2/") {
                break;
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        assert_eq!((host.players(), host.roster[0].1.as_str()), (2, "Ash"), "the host has Ash on its roster");
        assert!(seeker.status.contains("in the den (2/10)"), "{}", seeker.status);
        let mine = &seeker.rows[0];
        assert_eq!((mine.joinable, mine.note), (false, "you are here"), "the den you are in is not offered again");
        assert_eq!(seeker.click_join(0), Action::None);
        assert!(host.status.contains("2/10"), "{}", host.status);
        // Stop saying HELLO and the host lets the slot go.
        seeker.leave();
        host.tick(t1 + MEMBER_TIMEOUT + Duration::from_millis(50));
        assert_eq!(host.players(), 1, "a member who goes quiet is dropped");
        assert!(host.rows.is_empty(), "a host does not list itself");

        host.set_hosting(false);
        // Drain first. The host announced again during the handshake, and that
        // datagram is still sitting in the seeker's socket: read NOW it counts
        // as heard now. Staleness is measured from the last thing heard.
        let t2 = t1 + MEMBER_TIMEOUT + Duration::from_millis(100);
        std::thread::sleep(Duration::from_millis(20));
        seeker.tick(t2);
        let later = t2 + netcode::lan::STALE_AFTER + Duration::from_millis(50);
        host.tick(later);
        seeker.tick(later);
        assert!(seeker.rows.is_empty(), "the den closed and the row went with it");
    }

    /// The bsgames path, end to end on one machine: a host launched by
    /// handoff announces over the (file-backed) DHT, a joiner launched by
    /// handoff resolves the friend BY ID -- no LAN discovery involved -- and
    /// lands on the same roster a LAN joiner would.
    /// The host's START reaches the member as a plan naming it player 2 at the host's address,
    /// and both come away with the plan and a socket the match can run on. Nobody can start an
    /// empty den.
    #[test]
    fn the_hosts_start_hands_both_machines_the_same_plan() {
        let mut seeker = LanLobby::open_on("Ash", 0);
        let mut host = LanLobby::open_on("Bramble", 0);
        seeker.my_pick = 7;
        host.my_pick = 3;
        assert!(!host.can_start() && host.start(1, 0, "").is_none(), "nothing to start before hosting");
        let to = vec![(Ipv4Addr::LOCALHOST, seeker.port().expect("seeker is listening")).into()];
        host.host_to(to);
        assert!(!host.can_start(), "an empty den has nobody to start with");
        let t0 = Instant::now();
        for _ in 0..50 {
            host.tick(t0);
            seeker.tick(t0);
            if !seeker.rows.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        let Action::Join(addr) = seeker.click_join(0) else { panic!("not joinable: {}", seeker.status) };
        seeker.join(addr);
        let t1 = t0 + netcode::lan::ANNOUNCE_EVERY;
        for _ in 0..60 {
            seeker.tick(t1);
            host.tick(t1);
            if host.players() == 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        assert!(host.can_start());
        assert_eq!(host.members(), vec![("Bramble".to_string(), 3), ("Ash".to_string(), 7)], "the HELLO carried Ash's pick");
        // the member changes its mind before the start: the next HELLO carries the new pick
        seeker.my_pick = 9;
        let t2 = t1 + HELLO_EVERY;
        for _ in 0..60 {
            seeker.tick(t2);
            host.tick(t2);
            if host.roster[0].3 == 9 {
                break;
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        assert_eq!(host.roster[0].3, 9, "a changed pick reaches the host with the next HELLO");
        let plan = host.start(0xC0FFEE, 2, "hp=2").expect("the host starts");
        assert_eq!((plan.me, plan.players(), plan.seed, plan.map), (0, 2, 0xC0FFEE, 2));
        assert_eq!(plan.picks, vec![3, 9], "the host's pick first, then the member's");
        assert_eq!(plan.rules, "hp=2");
        let mut got = None;
        for _ in 0..60 {
            seeker.tick(t1);
            if let Some((p, _)) = seeker.take_start() {
                got = Some(p);
                break;
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        let got = got.expect("the plan reached the member");
        assert_eq!((got.me, got.players(), got.seed, got.map), (1, 2, 0xC0FFEE, 2));
        assert_eq!((got.picks.clone(), got.rules.as_str()), (vec![3, 9], "hp=2"), "the member builds the match under the host's rules and picks");
        assert_eq!(got.peers[0], addr, "the member fills the host in as the address it joined");
        assert_eq!(got.peers[1], plan.peers[1], "and sees itself as the host saw it");
        assert!(seeker.status.contains("starting"), "{}", seeker.status);
        let (mine, sock) = host.take_start().expect("the host keeps its own plan");
        assert_eq!(mine, plan);
        assert_eq!(sock.local_addr().unwrap().port(), addr.port(), "the match runs on the den's socket");
        assert!(host.take_start().is_none(), "taken once");
    }

    #[test]
    fn a_hello_carries_the_pick_and_an_old_one_is_pick_zero() {
        assert_eq!(LanLobby::hello("Ash", 7), "CBHELLO\tAsh\t7");
        assert_eq!(LanLobby::parse_hello("Ash\t7"), ("Ash".to_string(), 7));
        assert_eq!(LanLobby::parse_hello("Ash"), ("Ash".to_string(), 0), "an older build says no pick");
        assert_eq!(LanLobby::parse_hello("Ash\t999"), ("Ash".to_string(), 0), "off the roster is no pick");
        assert_eq!(LanLobby::parse_hello("A\u{7}sh\tx"), ("Ash".to_string(), 0));
    }

    #[test]
    fn a_friend_joining_by_id_through_the_dht_lands_in_the_den() {
        use netcode::multiplayer::Multiplayer;
        let store = std::env::temp_dir().join(format!("cb-handoff-{}.store", std::process::id()));
        let _ = std::fs::remove_file(&store);
        let mut host = LanLobby::open_on("Wren", 0);
        let token = host.host_for_handoff(Multiplayer::at_store(GAME, "wren", "Wren", store.clone())).unwrap();
        assert_eq!(token, "clawbound@wren");
        let mut friend = LanLobby::open_on("Fox", 0);
        let addr = friend.join_for_handoff(Multiplayer::at_store(GAME, "fox", "Fox", store.clone()), "wren").unwrap();
        assert!(!addr.ip().is_unspecified(), "resolved {addr}");
        let now = Instant::now();
        for _ in 0..60 {
            friend.tick(now);
            host.tick(now);
            if host.players() == 2 && friend.status.contains("in the den (2/") {
                break;
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        assert_eq!((host.players(), host.roster[0].1.as_str()), (2, "Fox"));
        assert!(friend.status.contains("in the den (2/10)"), "{}", friend.status);
        let mut stranger = LanLobby::open_on("Nobody", 0);
        assert!(stranger.join_for_handoff(Multiplayer::at_store(GAME, "nobody", "N", store.clone()), "not-a-friend").is_err());
        let _ = std::fs::remove_file(&store);
    }

    /// The path app.rs takes on a bsgames launch, both roles, and the case
    /// that used to end in a silent solo match: the friend is not hosting.
    #[test]
    fn a_bsgames_handoff_becomes_a_den_or_says_why_not() {
        use netcode::multiplayer::Multiplayer;
        let store = std::env::temp_dir().join(format!("cb-open-handoff-{}.store", std::process::id()));
        let _ = std::fs::remove_file(&store);
        let mp = |id: &str, name: &str| Multiplayer::at_store(GAME, id, name, store.clone());

        // Joining someone who is not hosting: still a lobby, with a reason.
        let early = LanLobby::open_for_handoff(false, Some("wren"), mp("fox", "Fox"), 0);
        let note = early.handoff_note.clone().unwrap();
        assert!(note.contains("Could not set that up") && note.contains("wren"), "{note}");
        assert!(early.joined.is_none() && early.port().is_some(), "and the LAN browser is still open to fall back on");
        assert!(LanLobby::open_for_handoff(false, None, mp("fox", "Fox"), 0).handoff_note.unwrap().contains("did not say whom"));

        let mut host = LanLobby::open_for_handoff(true, None, mp("wren", "Wren"), 0);
        assert!(host.hosting && host.handoff_note.as_deref().unwrap().contains("clawbound@wren"));
        let mut friend = LanLobby::open_for_handoff(false, Some("wren"), mp("fox", "Fox"), 0);
        assert!(friend.handoff_note.as_deref().unwrap().starts_with("Joining wren at "), "{:?}", friend.handoff_note);
        let now = Instant::now();
        for _ in 0..60 {
            friend.tick(now);
            host.tick(now);
            if host.players() == 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        assert_eq!((host.players(), host.roster[0].1.as_str()), (2, "Fox"), "the display name bsgames gave us is the one on the roster");
        let _ = std::fs::remove_file(&store);
    }

    #[test]
    fn a_second_copy_on_the_same_port_is_told_why_in_words() {
        let first = LanLobby::open_on("Ash", 0);
        let second = LanLobby::open_on("Ash", first.port().unwrap());
        assert!(second.port().is_none());
        assert!(second.status.contains("Another copy"), "got: {}", second.status);
        assert!(!second.status.contains("os error"), "an errno is not a message");
    }
}
