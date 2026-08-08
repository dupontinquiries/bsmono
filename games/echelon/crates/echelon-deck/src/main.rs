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

mod font;
mod render;

use echelon_bots::{bot_step_g, plan, Bot, Genome, CADENCE};
use echelon_data::{DataPack, Kind, TilesetDef};
use echelon_sim::map::GameMap;
use echelon_sim::{Cmd, PlayerCmd, World};

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

pub struct Deck {
    pub world: World,
    pub ts: TilesetDef,
    pub me: u8,
    pub enemy_g: Genome,
    pub enemy_bot: Bot,
    pub selection: Vec<u32>,
    pub ghosts: Vec<render::Ghost>,
    pub banner: Option<String>,
}

impl Deck {
    fn enemy_plan(&self) -> echelon_bots::Plan {
        plan(&self.world.pack.races[self.world.races[1 - self.me as usize]].name)
    }

    pub fn step(&mut self, n: u32) {
        let ep = 1 - self.me;
        for _ in 0..n {
            if self.banner.is_some() {
                return;
            }
            let mut cmds: Vec<PlayerCmd> = Vec::new();
            if self.world.tick % CADENCE == 0 {
                let plan = self.enemy_plan();
                bot_step_g(&self.world, ep, &plan, &self.enemy_g.clone(), &mut self.enemy_bot, &mut cmds);
            }
            self.world.step(&cmds);
            self.remember_ghosts();
            if self.world.tick % 44 == 0 {
                self.check_victory();
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
        let pc = vec![PlayerCmd { player: self.me, cmd: c }];
        // player commands ride an otherwise normal tick
        let ep = 1 - self.me;
        let mut cmds = pc;
        if self.world.tick % CADENCE == 0 {
            let plan = self.enemy_plan();
            bot_step_g(&self.world, ep, &plan, &self.enemy_g.clone(), &mut self.enemy_bot, &mut cmds);
        }
        self.world.step(&cmds);
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
        vec![
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
        ]
    }
}

pub fn new_deck(race: &str, enemy: &str, map: &str, seed: u64, champion: Option<&str>) -> Deck {
    let pack = load_pack();
    let (gm, ts) = load_map(map);
    let r0 = pack.race_index(race).expect("race");
    let r1 = pack.race_index(enemy).expect("enemy race");
    let world = World::new(pack, gm, [r0, r1], seed);
    let enemy_g = champion.map(parse_champion).unwrap_or_else(Genome::coach);
    Deck {
        world,
        ts,
        me: 0,
        enemy_g,
        enemy_bot: Bot::default(),
        selection: Vec::new(),
        ghosts: Vec::new(),
        banner: None,
    }
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
    match args.get(1).map(|s| s.as_str()) {
        Some("tape") => {
            let path = args.get(2).expect("tape <file>");
            let tape = std::fs::read_to_string(path).expect("tape file");
            let mut deck = new_deck(&race, &enemy, &map, seed, champ.as_deref());
            match deck.run_tape(&tape, false) {
                Ok(()) => println!("tape ok  (t{})", deck.world.tick),
                Err(e) => {
                    eprintln!("tape FAILED: {}", e);
                    std::process::exit(1);
                }
            }
        }
        #[cfg(feature = "window")]
        Some("play") => window::run(new_deck(&race, &enemy, &map, seed, champ.as_deref())),
        #[cfg(not(feature = "window"))]
        Some("play") => {
            eprintln!("built without the `window` feature; rebuild with --features window");
            std::process::exit(2);
        }
        _ => println!("echelon-deck tape <file> [--race --enemy --map --seed --champion] | play"),
    }
}
