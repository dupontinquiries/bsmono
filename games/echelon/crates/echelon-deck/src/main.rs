//! The Command Deck — Echelon's native client. Headless-first: a tape of
//! player inputs drives a real game against the coach (or an evolved
//! champion from the arena), rendering PNG frames and checking
//! assertions, so the whole human interface is CI-verifiable. The
//! `window` feature adds a minifb frontend that feeds the same op stream
//! from a live mouse and keyboard.
//!
//!   echelon-deck tape <file> [--race R] [--enemy R] [--map M] [--seed S]
//!                     [--champion arena_state.json]
//!   echelon-deck play  ... (requires --features window)
//!
//! Either verb takes `--lan-host PORT` (this deck is player 0 and waits for
//! the joiner) or `--lan-join HOST:PORT` (player 1): two decks, two
//! machines, one match over `echelon_net::udp` in lockstep -- the other
//! side is a person, not the coach. Both must give the same races, map and
//! seed (`--race` is player 0's race, `--enemy` player 1's, on both ends).
//!
//! Tape verbs (one per line, '#' comments):
//!   step N                advance N ticks (enemy bot thinks each second)
//!   select X0 Y0 X1 Y1    select own units in the tile rect
//!   selectat X Y          select the closest own unit to a tile
//!   move X Y | attack X Y | stop | hold
//!   harvest X Y           send selection to the node nearest that tile
//!   train NAME            queue in the first selected structure
//!   build NAME X Y        first selected worker builds at tile
//!   rally X Y             set rally on selected structures
//!   cast SLOT X Y         first selected caster fires ability slot
//!   shot FILE.png         render a frame
//!   assert KEY OP VALUE   alloy|flux|used2|cap2|sel|myunits|enemyvis
//!                         with OP in > < >= <= ==

mod audio;
mod cues;
mod font;
mod render;

use echelon_bots::{bot_step_g, plan, Bot, Genome, CADENCE};
use echelon_data::{DataPack, Kind, TilesetDef};
use echelon_net::udp::UdpLink;
use echelon_net::{Lockstep, NetCfg};
use echelon_sim::map::GameMap;
use echelon_sim::{Cmd, PlayerCmd, World};
use std::time::Instant;

fn assets() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data")
}

fn load_pack() -> DataPack {
    DataPack::load_dir(&assets().join("races"), &["genetron.ron", "xayid.ron", "keiron.ron"])
        .expect("pack")
}

fn load_map(name: &str) -> (GameMap, TilesetDef) {
    let md: echelon_data::MapDef =
        ron::from_str(&std::fs::read_to_string(assets().join("maps").join(format!("{}.ron", name))).expect("map"))
            .expect("map parse");
    let ts: TilesetDef = ron::from_str(
        &std::fs::read_to_string(assets().join("tilesets").join(format!("{}.ron", md.tileset))).expect("tileset"),
    )
    .expect("tileset parse");
    (GameMap::from_def(&md).expect("map build"), ts)
}

fn parse_champion(path: &str) -> Genome {
    let s = std::fs::read_to_string(path).expect("champion state");
    let key = "\"best\": [";
    let i = s.find(key).expect("best vec") + key.len();
    let j = i + s[i..].find(']').expect("vec end");
    let v: Vec<i32> = s[i..j].split(',').map(|x| x.trim().parse().expect("gene")).collect();
    Genome::from_vec(&v)
}

/// The lockstep config both decks of a LAN match run: the runner's (`--lan-host`).
pub const LAN_CFG: NetCfg = NetCfg { players: 2, turn_len: 4, delay: 2 };
/// Sim ticks between state-hash checkpoints exchanged over the link (a multiple of the
/// turn length, so a checkpoint tick is a turn boundary on both decks).
pub const HASH_EVERY: u32 = 448;
/// How far a stalled LAN deck lets owed ticks pile up: when the peer's turn arrives
/// late the deck catches up at most this many ticks, not the whole stall at once.
const MAX_DUE: u32 = LAN_CFG.turn_len * 8;

/// Who supplies the other side's commands and when the world may step.
pub enum Driver {
    /// The coach (or an evolved champion) plays the enemy in this process: the tape and
    /// the window against the bot, the world stepping whenever the deck says.
    Local { enemy_g: Genome, enemy_bot: Bot },
    /// A second deck over UDP: the world steps only on a lockstep turn that has both
    /// players' commands, and this deck's own commands go out `delay` turns ahead.
    Lan(Lan),
}

pub struct Lan {
    pub sess: Lockstep,
    pub link: UdpLink,
    /// this deck's commands since its last submitted turn
    pending: Vec<PlayerCmd>,
    /// sim ticks `step` owes that no ready turn has paid yet
    due: u32,
    pub checkpoints: u32,
}

impl Lan {
    pub fn new(me: u8, link: UdpLink) -> Lan {
        Lan { sess: Lockstep::new(LAN_CFG, me), link, pending: Vec::new(), due: 0, checkpoints: 0 }
    }
}

pub struct Deck {
    pub world: World,
    pub ts: TilesetDef,
    pub me: u8,
    pub driver: Driver,
    pub selection: Vec<u32>,
    pub ghosts: Vec<render::Ghost>,
    pub banner: Option<String>,
    /// The sound (`audio`): `None` headless or with no output, and the deck runs silent.
    pub audio: Option<audio::Audio>,
    /// What this frame of the match was, for the ears (`cues`).
    listener: cues::Listener,
}

impl Deck {
    /// Advance the match by `n` ticks: outright against the local bot; over a LAN, by as
    /// many whole lockstep turns as are ready, the rest owed until the peer's commands
    /// arrive (so a laggy peer freezes the world, never desynchronises it).
    pub fn step(&mut self, n: u32) {
        match self.driver {
            Driver::Local { .. } => {
                for _ in 0..n {
                    if self.banner.is_some() {
                        break;
                    }
                    self.tick(Vec::new());
                }
            }
            Driver::Lan(_) => self.step_lan(n),
        }
        // once per call, not per tick: a frame's worth of change is one set of cues
        self.hear();
    }

    fn sound(&self, m: audio::Msg) {
        if let Some(a) = &self.audio {
            a.send(m);
        }
    }

    /// What changed since the last frame, to hear.
    fn hear(&mut self) {
        let cues = self.listener.step(&self.world, self.me, self.banner.as_deref());
        for c in cues {
            self.sound(audio::Msg::Cue(c));
        }
    }

    /// One sim tick carrying `cmds` (this deck's), plus the local bot's when it thinks
    /// this tick; the fog memory and the victory check ride it.
    fn tick(&mut self, mut cmds: Vec<PlayerCmd>) {
        let ep = 1 - self.me;
        if let Driver::Local { enemy_g, enemy_bot } = &mut self.driver {
            if self.world.tick % CADENCE == 0 {
                let plan = plan(&self.world.pack.races[self.world.races[ep as usize]].name);
                bot_step_g(&self.world, ep, &plan, &enemy_g.clone(), enemy_bot, &mut cmds);
            }
        }
        self.world.step(&cmds);
        self.remember_ghosts();
        if self.world.tick % 44 == 0 {
            self.check_victory();
        }
    }

    fn step_lan(&mut self, n: u32) {
        let turn_len = LAN_CFG.turn_len;
        if let Driver::Lan(lan) = &mut self.driver {
            lan.due = (lan.due + n).min(MAX_DUE);
        }
        loop {
            let merged = {
                let Driver::Lan(lan) = &mut self.driver else { return };
                // one submission per executed turn keeps this deck's input stream exactly
                // `delay` turns ahead of execution (what the peer is waiting on)
                if lan.sess.lead() == LAN_CFG.delay {
                    let mine = std::mem::take(&mut lan.pending);
                    lan.sess.submit_local(mine);
                }
                lan.link.pump(&mut lan.sess, Instant::now());
                if lan.due < turn_len || self.banner.is_some() {
                    None
                } else {
                    lan.sess.next_turn().map(|(_, m)| m)
                }
            };
            let Some(merged) = merged else { break };
            self.tick(merged);
            for _ in 1..turn_len {
                self.tick(Vec::new());
            }
            let Driver::Lan(lan) = &mut self.driver else { return };
            lan.due -= turn_len;
            if self.world.tick % HASH_EVERY == 0 {
                lan.sess.report_hash(self.world.tick, self.world.state_hash());
                lan.checkpoints += 1;
            }
        }
        if let Driver::Lan(lan) = &self.driver {
            if let (Some(t), None) = (lan.sess.desync, &self.banner) {
                self.banner = Some(format!("DESYNC at t{t}"));
            }
        }
    }

    fn remember_ghosts(&mut self) {
        if self.world.tick % 22 != 0 {
            return;
        }
        let mask = self.world.visible_mask(self.me);
        for (i, e) in self.world.ents.iter().enumerate() {
            if !e.alive || e.def == u16::MAX || e.player == self.me || !mask[i] {
                continue;
            }
            let d = self.world.pack.def(e.def);
            if d.kind != Kind::Structure {
                continue;
            }
            let fp = d.fp();
            let (tx, ty) = e.pos.tile();
            let (gx, gy) = (tx - fp / 2, ty - fp / 2);
            if !self.ghosts.iter().any(|g| g.tx == gx && g.ty == gy) {
                self.ghosts.push(render::Ghost { tx: gx, ty: gy, fp, player: e.player });
            }
        }
        // forget ghosts we can see are gone
        let world = &self.world;
        self.ghosts.retain(|g| {
            let center_i = world
                .ents
                .iter()
                .position(|e| e.alive && e.def != u16::MAX && e.pos.tile() == (g.tx + g.fp / 2, g.ty + g.fp / 2));
            center_i.is_some() || !render::tile_vis(world, self.me)[((g.ty + g.fp / 2) * world.map.w + g.tx + g.fp / 2) as usize]
        });
    }

    fn check_victory(&mut self) {
        let alive = |p: u8| {
            self.world
                .ents
                .iter()
                .filter(|e| {
                    e.alive
                        && e.player == p
                        && e.def != u16::MAX
                        && self.world.pack.def(e.def).kind == Kind::Structure
                })
                .count()
        };
        if self.world.tick > 600 {
            if alive(1 - self.me) == 0 {
                self.banner = Some("VICTORY".into());
            } else if alive(self.me) == 0 {
                self.banner = Some("DEFEAT".into());
            }
        }
    }

    pub fn cmd_pub(&mut self, c: Cmd) { self.cmd(c) }
    pub fn hud_lines_pub(&self) -> Vec<String> { self.hud_lines() }

    fn cmd(&mut self, c: Cmd) {
        self.sound(audio::Msg::Cue(cues::Cue::Ack));
        let pc = PlayerCmd { player: self.me, cmd: c };
        match &mut self.driver {
            // player commands ride an otherwise normal tick
            Driver::Local { .. } => self.tick(vec![pc]),
            // over a LAN they wait for the next submitted turn
            Driver::Lan(lan) => lan.pending.push(pc),
        }
    }

    pub fn select_rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        let (lo_x, hi_x) = (x0.min(x1), x0.max(x1));
        let (lo_y, hi_y) = (y0.min(y1), y0.max(y1));
        self.selection = self
            .world
            .ents
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e.alive && e.def != u16::MAX && e.player == self.me && {
                    let (tx, ty) = e.pos.tile();
                    tx >= lo_x && tx <= hi_x && ty >= lo_y && ty <= hi_y
                }
            })
            .map(|(i, _)| i as u32)
            .collect();
    }

    pub fn select_at(&mut self, x: i32, y: i32) {
        // mobiles win ties against structures — "click near a unit"
        // should never grab the townhall
        let best = self
            .world
            .ents
            .iter()
            .enumerate()
            .filter(|(_, e)| e.alive && e.def != u16::MAX && e.player == self.me)
            .min_by_key(|(i, e)| {
                let (tx, ty) = e.pos.tile();
                let structure =
                    self.world.pack.def(e.def).kind == Kind::Structure;
                ((tx - x).abs() + (ty - y).abs(), structure, *i)
            })
            .map(|(i, _)| i as u32);
        self.selection = best.into_iter().collect();
    }

    fn sel_units(&self) -> Vec<u32> {
        self.selection
            .iter()
            .copied()
            .filter(|&i| {
                let e = &self.world.ents[i as usize];
                e.alive && self.world.pack.def(e.def).kind != Kind::Structure
            })
            .collect()
    }

    pub fn run_tape(&mut self, tape: &str, quiet: bool) -> Result<(), String> {
        for (ln, raw) in tape.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let t: Vec<&str> = line.split_whitespace().collect();
            let ip = |k: usize| -> i32 { t[k].parse().expect("int arg") };
            let mm = |x: i32, y: i32| (x * 1000 + 500, y * 1000 + 500);
            match t[0] {
                "step" => self.step(ip(1) as u32),
                "select" => self.select_rect(ip(1), ip(2), ip(3), ip(4)),
                "selectat" => self.select_at(ip(1), ip(2)),
                "move" => {
                    let (mx, my) = mm(ip(1), ip(2));
                    let ids = self.sel_units();
                    self.cmd(Cmd::Move { ids, mx, my });
                }
                "attack" => {
                    let (mx, my) = mm(ip(1), ip(2));
                    let ids = self.sel_units();
                    self.cmd(Cmd::Attack { ids, mx, my });
                }
                "stop" => {
                    let ids = self.sel_units();
                    self.cmd(Cmd::Stop { ids });
                }
                "hold" => {
                    let ids = self.sel_units();
                    self.cmd(Cmd::Hold { ids });
                }
                "harvest" => {
                    let node = self
                        .world
                        .map
                        .nodes
                        .iter()
                        .enumerate()
                        .filter(|(_, n)| n.amount > 0)
                        .min_by_key(|(_, n)| (n.tx - ip(1)).abs() + (n.ty - ip(2)).abs())
                        .map(|(i, _)| i as u32)
                        .ok_or("no node")?;
                    for id in self.sel_units() {
                        self.cmd(Cmd::Harvest { id, node });
                    }
                }
                "train" => {
                    let unit = self.world.pack.id(t[1]);
                    let s = self
                        .selection
                        .iter()
                        .copied()
                        .find(|&i| {
                            let e = &self.world.ents[i as usize];
                            e.alive
                                && self.world.pack.def(e.def).trains.iter().any(|n| n == t[1])
                        })
                        .ok_or_else(|| format!("line {}: nothing selected trains {}", ln + 1, t[1]))?;
                    self.cmd(Cmd::Train { structure: s, unit });
                }
                "build" => {
                    let unit = self.world.pack.id(t[1]);
                    let worker = self
                        .selection
                        .iter()
                        .copied()
                        .find(|&i| {
                            self.world.pack.def(self.world.ents[i as usize].def).kind == Kind::Worker
                        })
                        .ok_or("no worker selected")?;
                    self.cmd(Cmd::Build { worker, unit, tx: ip(2), ty: ip(3) });
                }
                "rally" => {
                    let (mx, my) = mm(ip(1), ip(2));
                    for s in self.selection.clone() {
                        self.cmd(Cmd::Rally { structure: s, mx, my });
                    }
                }
                "cast" => {
                    let slot = ip(1) as u8;
                    let (mx, my) = mm(ip(2), ip(3));
                    let caster = *self.selection.first().ok_or("no caster selected")?;
                    self.cmd(Cmd::Cast { caster, slot, target: 0, mx, my });
                }
                "track" => {
                    for (i, e) in self.world.ents.iter().enumerate() {
                        if e.alive && e.def != u16::MAX && e.player == self.me {
                            let d = self.world.pack.def(e.def);
                            if d.kind != Kind::Structure {
                                println!(
                                    "  #{} {} {:?} path {}/{} {:?}",
                                    i,
                                    d.name,
                                    e.pos.tile(),
                                    e.path_i,
                                    e.path.len(),
                                    e.order
                                );
                            }
                        }
                    }
                }
                "dump" => {
                    for &i in &self.selection {
                        let e = &self.world.ents[i as usize];
                        let d = self.world.pack.def(e.def);
                        println!(
                            "  ent{} {} p{} q{} prog{} hp{}",
                            i, d.name, e.player, e.queue.len(), e.progress,
                            e.hp.to_int_floor()
                        );
                    }
                }
                "shot" => {
                    let hud = self.hud_lines();
                    let f = render::render(
                        &self.world,
                        &self.ts,
                        self.me,
                        &self.selection,
                        &self.ghosts,
                        self.banner.as_deref(),
                        &hud,
                    );
                    render::write_png(&f, t[1]);
                    if !quiet {
                        println!("shot -> {}", t[1]);
                    }
                }
                "assert" => {
                    let v = self.metric(t[1]).ok_or_else(|| format!("bad metric {}", t[1]))?;
                    let rhs: i64 = t[3].parse().expect("assert value");
                    let ok = match t[2] {
                        ">" => v > rhs,
                        "<" => v < rhs,
                        ">=" => v >= rhs,
                        "<=" => v <= rhs,
                        "==" => v == rhs,
                        _ => return Err(format!("bad op {}", t[2])),
                    };
                    if !ok {
                        return Err(format!(
                            "line {}: assert {} {} {} failed (actual {})",
                            ln + 1,
                            t[1],
                            t[2],
                            rhs,
                            v
                        ));
                    }
                    if !quiet {
                        println!("assert {} {} {} ok ({})", t[1], t[2], rhs, v);
                    }
                }
                other => return Err(format!("line {}: unknown verb {}", ln + 1, other)),
            }
        }
        Ok(())
    }

    fn metric(&self, k: &str) -> Option<i64> {
        let pi = self.me as usize;
        Some(match k {
            "alloy" => self.world.res[pi].0 as i64,
            "flux" => self.world.res[pi].1 as i64,
            "used2" => self.world.used2[pi] as i64,
            "cap2" => self.world.cap2[pi] as i64,
            "sel" => self.selection.len() as i64,
            "ghosts" => self.ghosts.len() as i64,
            "myunits" => self
                .world
                .ents
                .iter()
                .filter(|e| e.alive && e.def != u16::MAX && e.player == self.me)
                .count() as i64,
            "enemyvis" => {
                let m = self.world.visible_mask(self.me);
                self.world
                    .ents
                    .iter()
                    .enumerate()
                    .filter(|(i, e)| e.alive && e.def != u16::MAX && e.player != self.me && m[*i])
                    .count() as i64
            }
            _ => return None,
        })
    }

    fn hud_lines(&self) -> Vec<String> {
        let pi = self.me as usize;
        let sel = if let Some(&i) = self.selection.first() {
            let e = &self.world.ents[i as usize];
            let d = self.world.pack.def(e.def);
            format!(
                "SEL: {} X{}  HP {}",
                d.name.to_uppercase(),
                self.selection.len(),
                e.hp.to_int_floor()
            )
        } else {
            "SEL: -".into()
        };
        let mut lines = vec![
            format!(
                "ALLOY {}  FLUX {}  SUPPLY {}/{}  T{}",
                self.world.res[pi].0,
                self.world.res[pi].1,
                self.world.used2[pi] / 2,
                self.world.cap2[pi] / 2,
                self.world.tick
            ),
            sel,
            "RIGHTCLICK MOVE  A ATTACK  S STOP  H HOLD  R RALLY".into(),
        ];
        // over a LAN the HUD says where the turns stand: a world that stops moving is the peer,
        // and this line is how a person tells that from a hang
        if let Driver::Lan(lan) = &self.driver {
            let owed = lan.due / LAN_CFG.turn_len;
            let state = if owed >= 2 { "WAITING FOR PEER" } else { "IN STEP" };
            lines.push(format!("LAN P{}  TURN {}  {}  UNACKED {}", self.me, lan.sess.exec_turn(), state, lan.link.unacked()));
        }
        lines
    }
}

pub fn new_deck(race: &str, enemy: &str, map: &str, seed: u64, champion: Option<&str>) -> Deck {
    let enemy_g = champion.map(parse_champion).unwrap_or_else(Genome::coach);
    deck_with(race, enemy, map, seed, 0, Driver::Local { enemy_g, enemy_bot: Bot::default() })
}

/// A deck seated as `me` (0 or 1) in a match whose two races are given in seat order --
/// the same world on both ends of a LAN link.
pub fn deck_with(race0: &str, race1: &str, map: &str, seed: u64, me: u8, driver: Driver) -> Deck {
    let pack = load_pack();
    let (gm, ts) = load_map(map);
    let r0 = pack.race_index(race0).expect("race");
    let r1 = pack.race_index(race1).expect("enemy race");
    let world = World::new(pack, gm, [r0, r1], seed);
    Deck { world, ts, me, driver, selection: Vec::new(), ghosts: Vec::new(), banner: None, audio: None, listener: cues::Listener::new() }
}

/// The LAN seat the command line asks for: `--lan-host PORT` binds and waits for the
/// joiner's first datagram (player 0); `--lan-join HOST:PORT` dials (player 1).
fn lan_driver(args: &[String]) -> Option<(u8, Lan)> {
    use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
    let val = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    if let Some(port) = val("--lan-host") {
        let port: u16 = port.parse().expect("--lan-host PORT");
        let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).expect("bind the host port");
        sock.set_nonblocking(true).unwrap();
        println!("lan host: listening on {} -- waiting for the joiner", sock.local_addr().unwrap());
        let mut buf = [0u8; 64];
        let from: SocketAddr = loop {
            if let Ok((_, from)) = sock.peek_from(&mut buf) {
                break from;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        println!("joiner at {from}");
        return Some((0, Lan::new(0, UdpLink::over(sock, from))));
    }
    if let Some(peer) = val("--lan-join") {
        let peer: SocketAddr = peer.parse().expect("--lan-join HOST:PORT");
        println!("lan joiner: host {peer}");
        return Some((1, Lan::new(1, UdpLink::open("0.0.0.0:0", peer).expect("bind a socket"))));
    }
    None
}

#[cfg(feature = "window")]
mod window;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |flag: &str, def: &str| -> String {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
            .unwrap_or_else(|| def.to_string())
    };
    let race = get("--race", "Genetron");
    let enemy = get("--enemy", "Xayid");
    let map = get("--map", "cauldron");
    let seed: u64 = get("--seed", "7").parse().unwrap();
    let champ = args
        .iter()
        .position(|a| a == "--champion")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let verb = args.get(1).map(|s| s.as_str());
    let deck = || match lan_driver(&args) {
        Some((me, lan)) => deck_with(&race, &enemy, &map, seed, me, Driver::Lan(lan)),
        None => new_deck(&race, &enemy, &map, seed, champ.as_deref()),
    };
    match verb {
        Some("tape") => {
            let path = args.get(2).expect("tape <file>");
            let tape = std::fs::read_to_string(path).expect("tape file");
            let mut deck = deck();
            match deck.run_tape(&tape, false) {
                Ok(()) => println!("tape ok  (t{})", deck.world.tick),
                Err(e) => {
                    eprintln!("tape FAILED: {}", e);
                    std::process::exit(1);
                }
            }
        }
        #[cfg(feature = "window")]
        Some("play") => {
            let mut d = deck();
            d.audio = audio::Audio::start();
            if d.audio.is_none() {
                eprintln!("echelon-deck: no sound (build with --features sound and have an output device)");
            }
            window::run(d)
        }
        #[cfg(not(feature = "window"))]
        Some("play") => {
            eprintln!("built without the `window` feature; rebuild with --features window");
            std::process::exit(2);
        }
        _ => println!("echelon-deck tape <file> [--race --enemy --map --seed --champion] [--lan-host PORT | --lan-join HOST:PORT] | play [...]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echelon_sim::world::Order;
    use std::net::UdpSocket;

    /// The tape path against the coach: verbs run, assertions read the world, a failing
    /// assertion is an error with the line in it.
    #[test]
    fn a_tape_plays_against_the_coach() {
        let mut d = new_deck("Genetron", "Xayid", "cauldron", 7, None);
        d.run_tape("step 44\nassert myunits >= 1\nselect 0 0 500 500\nassert sel >= 1\nhold\nstep 400\n", true).unwrap();
        assert!(d.world.tick >= 444, "t{}", d.world.tick);
        let err = d.run_tape("assert sel > 100000\n", true).unwrap_err();
        assert!(err.contains("line 1") && err.contains("failed"), "{err}");
        assert!(d.run_tape("frobnicate\n", true).unwrap_err().contains("unknown verb"));
    }

    /// Two decks seated 0 and 1, each the other's peer over loopback UDP.
    fn pair() -> (Deck, Deck) {
        let sa = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sb = UdpSocket::bind("127.0.0.1:0").unwrap();
        let (aa, ab) = (sa.local_addr().unwrap(), sb.local_addr().unwrap());
        let a = deck_with("Genetron", "Xayid", "cauldron", 7, 0, Driver::Lan(Lan::new(0, UdpLink::over(sa, ab))));
        let b = deck_with("Genetron", "Xayid", "cauldron", 7, 1, Driver::Lan(Lan::new(1, UdpLink::over(sb, aa))));
        (a, b)
    }

    fn lan(d: &Deck) -> &Lan {
        match &d.driver {
            Driver::Lan(l) => l,
            _ => panic!("not a LAN deck"),
        }
    }

    /// Two decks over loopback: a command issued on one lands in both worlds on the same
    /// turn, the worlds hash identical at every checkpoint, and neither deck's input
    /// stream runs more than one turn past the delay.
    #[test]
    fn two_decks_over_loopback_play_one_world() {
        let (mut a, mut b) = pair();
        // player 0 puts one of its mobiles on hold -- an order with no pathing in it, so
        // it reads the same on both ends the turn it executes
        a.select_rect(0, 0, 500, 500);
        let ids = a.sel_units();
        assert!(!ids.is_empty(), "player 0 starts with a mobile");
        let id = ids[0];
        assert_ne!(a.world.ents[id as usize].order, Order::Hold);
        a.cmd_pub(Cmd::Hold { ids: vec![id] });
        // player 1 orders its own unit the same way, from its own deck
        b.select_rect(0, 0, 500, 500);
        let bids = b.sel_units();
        assert!(!bids.is_empty());
        b.cmd_pub(Cmd::Hold { ids: vec![bids[0]] });
        for _ in 0..600 {
            a.step(2);
            b.step(2);
            assert!(lan(&a).sess.lead() <= LAN_CFG.delay + 1, "deck 0 ran ahead of its input: lead {}", lan(&a).sess.lead());
            assert!(lan(&b).sess.lead() <= LAN_CFG.delay + 1);
        }
        // let the trailing deck execute what the other already has
        for _ in 0..50 {
            a.step(0);
            b.step(0);
        }
        assert_eq!(a.world.tick, b.world.tick, "both decks executed the same turns");
        assert!(a.world.tick >= 1100, "the match advanced (t{})", a.world.tick);
        assert_eq!(a.world.state_hash(), b.world.state_hash(), "one world on both ends");
        assert_eq!(a.world.ents[id as usize].order, Order::Hold, "player 0's order on deck 0");
        assert_eq!(b.world.ents[id as usize].order, Order::Hold, "player 0's order reached deck 1");
        assert_eq!(a.world.ents[bids[0] as usize].order, Order::Hold, "player 1's order reached deck 0");
        assert!(lan(&a).checkpoints >= 2 && lan(&b).checkpoints >= 2, "hashes were exchanged");
        assert_eq!(lan(&a).sess.desync, None);
        assert_eq!(lan(&b).sess.desync, None);
        assert!(a.banner.is_none() && b.banner.is_none());
        assert!(a.hud_lines_pub().iter().any(|l| l.starts_with("LAN P0") && l.contains("IN STEP")), "{:?}", a.hud_lines_pub());
    }

    /// A deck whose peer never answers plays its bootstrap turns and then stands still:
    /// the owed ticks are capped and the world does not step without the peer's input.
    #[test]
    fn a_deck_with_a_silent_peer_freezes_instead_of_running_ahead() {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        let nobody = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut d = deck_with("Genetron", "Xayid", "cauldron", 7, 0, Driver::Lan(Lan::new(0, UdpLink::over(s, nobody.local_addr().unwrap()))));
        for _ in 0..200 {
            d.step(20);
        }
        let bootstrap = LAN_CFG.delay * LAN_CFG.turn_len;
        assert_eq!(d.world.tick, bootstrap, "only the input-free bootstrap turns ran");
        assert_eq!(lan(&d).due, MAX_DUE, "owed ticks are capped");
        assert!(d.banner.is_none());
        assert_eq!(lan(&d).sess.lead(), LAN_CFG.delay + 1, "one turn submitted past the delay, waiting");
        let hud = d.hud_lines_pub();
        assert!(hud.iter().any(|l| l.contains("WAITING FOR PEER")), "{hud:?}");
    }
}
