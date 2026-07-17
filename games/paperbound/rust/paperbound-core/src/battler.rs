//! battler.rs — slice 6: the BASE GAME's mode, ported from game.py's lane
//! battler. The numbers are the shipped ones, cited to their source lines:
//!
//!   ink regen      ink_rate / 2.8 × 1.025 per second, cap 10   (game.py:2299)
//!   towers         king 2600 hp · princess 1500 hp,
//!                  range 223, dmg 60, swing 0.9 s              (game.py:1039)
//!   MODES          classic/blitz/sudden/fog/doubleink/drought/
//!                  fortress — ink_rate / tower_hp / start_ink  (game.py:339)
//!
//! Cards come from the real cards.json (cost/hp/dmg/spd/rng/count). The sim
//! is the authoritative core of a match: deploy spends ink (with the same
//! refusal the hand gives — "noink"), `count` fields fan out multi-unit
//! cards, units advance on the enemy king, fight what they meet, towers
//! defend, and the match ends when a king folds. Deterministic fixed-dt.

use std::collections::BTreeMap;
use std::path::Path;

pub const INK_CAP: f64 = 10.0;
pub const TOWER_KING_HP: f64 = 2600.0;
pub const TOWER_PRINCESS_HP: f64 = 1500.0;
pub const TOWER_RNG: f64 = 223.0;
pub const TOWER_DMG: f64 = 60.0;
pub const TOWER_CD: f64 = 0.9;
pub const AGGRO: f64 = 200.0;
pub const SWING: f64 = 0.6;

#[derive(Debug, Clone, Copy)]
pub struct Mode {
    pub ink_rate: f64,
    pub tower_hp: f64,
    pub start_ink: f64,
}

/// game.py MODES, minus presentation-only fields (fog is a visibility rule the
/// renderer owns; the sim is identical under it).
pub fn mode(name: &str) -> Mode {
    match name {
        "blitz" => Mode { ink_rate: 2.0, tower_hp: 1.0, start_ink: 6.0 },
        "sudden" => Mode { ink_rate: 1.2, tower_hp: 0.5, start_ink: 6.0 },
        "fog" => Mode { ink_rate: 1.0, tower_hp: 1.0, start_ink: 5.0 },
        "doubleink" => Mode { ink_rate: 1.4, tower_hp: 1.0, start_ink: 10.0 },
        "drought" => Mode { ink_rate: 0.55, tower_hp: 1.0, start_ink: 4.0 },
        "fortress" => Mode { ink_rate: 1.0, tower_hp: 2.0, start_ink: 5.0 },
        _ => Mode { ink_rate: 1.0, tower_hp: 1.0, start_ink: 5.0 }, // classic
    }
}

#[derive(Debug, Clone)]
pub struct Card {
    pub cost: f64,
    pub hp: f64,
    pub dmg: f64,
    pub spd: f64,
    pub rng: f64,
    pub count: usize,
    pub building: bool,
}

/// Load the shipped cards.json (the `cards` map inside the document).
pub fn load_cards(game_dir: &Path) -> Result<BTreeMap<String, Card>, String> {
    let raw = std::fs::read_to_string(game_dir.join("cards.json"))
        .map_err(|e| format!("cards.json: {e}"))?;
    let doc: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("cards.json parse: {e}"))?;
    let mut out = BTreeMap::new();
    let cards = doc["cards"].as_object().ok_or("no cards map")?;
    for (k, c) in cards {
        let f = |key: &str, d: f64| c[key].as_f64().unwrap_or(d);
        out.insert(
            k.clone(),
            Card {
                cost: f("cost", 3.0),
                hp: f("hp", 100.0),
                dmg: f("dmg", 20.0),
                spd: f("spd", 60.0),
                rng: f("rng", 26.0),
                count: c["count"].as_u64().unwrap_or(1) as usize,
                building: c["building"].as_bool().unwrap_or(false),
            },
        );
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct BUnit {
    pub team: usize,
    pub key: String,
    pub x: f64,
    pub y: f64,
    pub hp: f64,
    pub dmg: f64,
    pub spd: f64,
    pub rng: f64,
    pub cool: f64,
}

impl BUnit {
    pub fn alive(&self) -> bool {
        self.hp > 0.0
    }
}

#[derive(Debug, Clone)]
pub struct BTower {
    pub team: usize,
    pub king: bool,
    pub x: f64,
    pub y: f64,
    pub hp: f64,
    pub cool: f64,
}

impl BTower {
    pub fn alive(&self) -> bool {
        self.hp > 0.0
    }
}

pub struct Battle<'a> {
    pub cards: &'a BTreeMap<String, Card>,
    pub mode: Mode,
    pub ink: [f64; 2],
    pub units: Vec<BUnit>,
    pub towers: Vec<BTower>,
    pub t: f64,
    pub winner: Option<usize>,
}

/// The page: 1280×800, river at mid-height; team 0 holds the bottom.
pub const PAGE_W: f64 = 1280.0;
pub const PAGE_H: f64 = 800.0;
pub const RIVER_Y: f64 = 400.0;

impl<'a> Battle<'a> {
    pub fn new(cards: &'a BTreeMap<String, Card>, mode_name: &str) -> Self {
        let m = mode(mode_name);
        let mut towers = Vec::new();
        for team in 0..2usize {
            let (ky, py) = if team == 0 { (740.0, 640.0) } else { (60.0, 160.0) };
            towers.push(BTower {
                team,
                king: true,
                x: PAGE_W / 2.0,
                y: ky,
                hp: TOWER_KING_HP * m.tower_hp,
                cool: 0.0,
            });
            for px in [320.0, 960.0] {
                towers.push(BTower {
                    team,
                    king: false,
                    x: px,
                    y: py,
                    hp: TOWER_PRINCESS_HP * m.tower_hp,
                    cool: 0.0,
                });
            }
        }
        Battle {
            cards,
            mode: m,
            ink: [m.start_ink, m.start_ink],
            units: Vec::new(),
            towers,
            t: 0.0,
            winner: None,
        }
    }

    /// game.py try_play: refuse without ink ("noink"), spend, fan out `count`.
    pub fn deploy(&mut self, team: usize, key: &str, x: f64, y: f64) -> Result<(), &'static str> {
        let Some(card) = self.cards.get(key) else {
            return Err("unknown");
        };
        if self.ink[team] < card.cost {
            return Err("noink");
        }
        // your own half only (the hand clamps to below the river for team 0)
        let y = if team == 0 { y.max(RIVER_Y + 20.0) } else { y.min(RIVER_Y - 20.0) };
        self.ink[team] -= card.cost;
        for i in 0..card.count {
            let off = (i as f64 - (card.count as f64 - 1.0) / 2.0) * 24.0;
            self.units.push(BUnit {
                team,
                key: key.into(),
                x: x + off,
                y,
                hp: card.hp,
                dmg: card.dmg,
                spd: card.spd,
                rng: card.rng,
                cool: 0.0,
            });
        }
        Ok(())
    }

    fn nearest_enemy_unit(&self, team: usize, x: f64, y: f64, within: f64) -> Option<usize> {
        let mut best = None;
        let mut bd = within;
        for (i, u) in self.units.iter().enumerate() {
            if u.team != team && u.alive() {
                let d = ((u.x - x).powi(2) + (u.y - y).powi(2)).sqrt();
                if d < bd {
                    bd = d;
                    best = Some(i);
                }
            }
        }
        best
    }

    fn nearest_enemy_tower(&self, team: usize, x: f64, y: f64) -> Option<usize> {
        self.towers
            .iter()
            .enumerate()
            .filter(|(_, tw)| tw.team != team && tw.alive())
            .min_by(|a, b| {
                let da = (a.1.x - x).powi(2) + (a.1.y - y).powi(2);
                let db = (b.1.x - x).powi(2) + (b.1.y - y).powi(2);
                da.partial_cmp(&db).unwrap()
            })
            .map(|(i, _)| i)
    }

    pub fn tick(&mut self, dt: f64) {
        if self.winner.is_some() {
            return;
        }
        self.t += dt;
        // ink regen: game.py:2299 — rate/2.8 × 1.025, capped
        let regen = self.mode.ink_rate / 2.8 * 1.025;
        for side in 0..2 {
            self.ink[side] = (self.ink[side] + regen * dt).min(INK_CAP);
        }
        // towers fire (rng 223, dmg 60, 0.9s swing)
        for ti in 0..self.towers.len() {
            if !self.towers[ti].alive() {
                continue;
            }
            self.towers[ti].cool = (self.towers[ti].cool - dt).max(0.0);
            let (team, tx, ty) = {
                let tw = &self.towers[ti];
                (tw.team, tw.x, tw.y)
            };
            if self.towers[ti].cool <= 0.0 {
                if let Some(ei) = self.nearest_enemy_unit(team, tx, ty, TOWER_RNG) {
                    self.units[ei].hp -= TOWER_DMG;
                    self.towers[ti].cool = TOWER_CD;
                }
            }
        }
        // units: fight what's in aggro, else advance on the nearest tower
        for i in 0..self.units.len() {
            if !self.units[i].alive() {
                continue;
            }
            self.units[i].cool = (self.units[i].cool - dt).max(0.0);
            let (team, x, y) = {
                let u = &self.units[i];
                (u.team, u.x, u.y)
            };
            if let Some(ei) = self.nearest_enemy_unit(team, x, y, AGGRO) {
                let (ex, ey) = (self.units[ei].x, self.units[ei].y);
                let d = ((ex - x).powi(2) + (ey - y).powi(2)).sqrt();
                if d > self.units[i].rng {
                    self.step(i, ex, ey, dt);
                } else if self.units[i].cool <= 0.0 {
                    let dmg = self.units[i].dmg;
                    self.units[ei].hp -= dmg;
                    self.units[i].cool = SWING;
                }
                continue;
            }
            if let Some(ti) = self.nearest_enemy_tower(team, x, y) {
                let (tx, ty) = (self.towers[ti].x, self.towers[ti].y);
                let d = ((tx - x).powi(2) + (ty - y).powi(2)).sqrt();
                if d > self.units[i].rng + 26.0 {
                    self.step(i, tx, ty, dt);
                } else if self.units[i].cool <= 0.0 {
                    let dmg = self.units[i].dmg;
                    self.towers[ti].hp -= dmg;
                    self.units[i].cool = SWING;
                }
            }
        }
        // a fallen king ends it (game.py: result on king destruction)
        for side in 0..2 {
            if !self.towers.iter().any(|tw| tw.team == side && tw.king && tw.alive()) {
                self.winner = Some(1 - side);
            }
        }
    }

    fn step(&mut self, i: usize, tx: f64, ty: f64, dt: f64) {
        let u = &mut self.units[i];
        let dx = tx - u.x;
        let dy = ty - u.y;
        let d = (dx * dx + dy * dy).sqrt();
        if d > 1e-6 {
            let s = (u.spd * dt).min(d);
            u.x += dx / d * s;
            u.y += dy / d * s;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cards() -> BTreeMap<String, Card> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        load_cards(&dir).expect("real cards.json")
    }

    #[test]
    fn deploy_refusals_and_count_fanout() {
        let c = cards();
        let mut b = Battle::new(&c, "classic");
        assert_eq!(b.ink[0], 5.0, "classic starts at 5 (game.py MODES)");
        // an expensive card refuses exactly like the hand does
        let pricey = c
            .iter()
            .find(|(_, v)| v.cost > 5.0 && !v.building)
            .map(|(k, _)| k.clone())
            .expect("some card costs > 5");
        assert_eq!(b.deploy(0, &pricey, 640.0, 600.0), Err("noink"));
        // a multi-unit card fans out its whole count
        let (mk, mc) = c
            .iter()
            .find(|(_, v)| v.count > 1 && v.cost <= 5.0)
            .map(|(k, v)| (k.clone(), v.count))
            .expect("a multi-unit card within budget");
        b.deploy(0, &mk, 640.0, 600.0).unwrap();
        assert_eq!(b.units.len(), mc);
        // team 0 can't deploy over the river
        b.ink[0] = 10.0;
        let cheap = c
            .iter()
            .find(|(_, v)| v.cost <= 3.0 && v.count == 1)
            .map(|(k, _)| k.clone())
            .unwrap();
        b.deploy(0, &cheap, 640.0, 100.0).unwrap();
        assert!(b.units.last().unwrap().y >= RIVER_Y);
    }

    #[test]
    fn ink_regen_matches_the_python_formula() {
        let c = cards();
        let mut b = Battle::new(&c, "classic");
        b.ink = [0.0, 0.0];
        for _ in 0..(10 * 30) {
            b.tick(1.0 / 30.0);
        }
        let expect = 10.0 * (1.0 / 2.8) * 1.025; // 10 s at classic rate
        assert!((b.ink[0] - expect).abs() < 0.05, "{} vs {}", b.ink[0], expect);
        // blitz doubles it (capped at 10)
        let mut bl = Battle::new(&c, "blitz");
        bl.ink = [0.0, 0.0];
        for _ in 0..(10 * 30) {
            bl.tick(1.0 / 30.0);
        }
        assert!((bl.ink[0] - (expect * 2.0).min(INK_CAP)).abs() < 0.05);
    }

    #[test]
    fn modes_reshape_towers() {
        let c = cards();
        let sudden = Battle::new(&c, "sudden");
        assert_eq!(sudden.towers[0].hp, TOWER_KING_HP * 0.5);
        let fortress = Battle::new(&c, "fortress");
        assert_eq!(fortress.towers[0].hp, TOWER_KING_HP * 2.0);
    }

    #[test]
    fn a_push_takes_a_tower_and_the_king_ends_it() {
        let c = cards();
        let mut b = Battle::new(&c, "classic");
        // team 0 pushes with a heavy card, refilled ink, no defence
        let tank = c
            .iter()
            .filter(|(_, v)| !v.building && v.count == 1)
            .max_by(|a, b2| a.1.hp.partial_cmp(&b2.1.hp).unwrap())
            .map(|(k, _)| k.clone())
            .unwrap();
        for _ in 0..6 {
            b.ink[0] = 10.0;
            b.deploy(0, &tank, 320.0, 430.0).unwrap();
        }
        for _ in 0..(240 * 30) {
            b.tick(1.0 / 30.0);
            if b.winner.is_some() {
                break;
            }
        }
        assert_eq!(b.winner, Some(0), "an unanswered push must end the match");
        // decided battles freeze
        let t0 = b.t;
        b.tick(1.0);
        assert_eq!(b.t, t0);
    }

    #[test]
    fn deterministic_battle() {
        let c = cards();
        let run = || {
            let mut b = Battle::new(&c, "classic");
            let cheap: Vec<String> = c
                .iter()
                .filter(|(_, v)| v.cost <= 4.0 && !v.building)
                .take(3)
                .map(|(k, _)| k.clone())
                .collect();
            for (i, k) in cheap.iter().enumerate() {
                b.ink = [10.0, 10.0];
                b.deploy(0, k, 300.0 + i as f64 * 120.0, 500.0).unwrap();
                b.deploy(1, k, 340.0 + i as f64 * 120.0, 300.0).unwrap();
            }
            for _ in 0..(30 * 30) {
                b.tick(1.0 / 30.0);
            }
            b.units
                .iter()
                .map(|u| format!("{:.3},{:.3},{:.1}", u.x, u.y, u.hp))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }
}
