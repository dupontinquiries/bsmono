// Major objectives — two neutral beasts in sheltered pits off the lanes,
// fought over for match-altering powers (the Gold Fury / Fire Giant shape):
//
//   The Elder Boar   west pit, up at 3:00, respawns 4:00 after a kill.
//                    Tusk charge + ground slam. Killing team: +250 gold each
//                    and THE WALLOW'S BOUNTY — kill and last-hit gold ×1.5
//                    for 90 s.
//   The Storm Drake  east pit, up at 8:00, respawns 5:00. Lightning bite at
//                    range + a breath cone. Killing team: STORMFIRE — +18 %
//                    to all damage dealt and autos hit structures +80 for
//                    90 s.
//
// Both are neutral (`TargetRef::Boss`): either team can hit them, they bite
// back at whoever hurt them last, and they mend fast once left alone (so a
// half-done objective can be stolen, or lost to the reset). The pits
// (`pit_positions`) are cleared of flora and ringed with boulders and
// pillars by scene.rs, with gaps to enter — a small arena inside the jungle.

use crate::consts::{lane_clearance, river_dist, Team, ARENA_RADIUS, MAP_SCALE};
use crate::sim::{Event, Game, Rng, TargetRef};
use glam::Vec3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BossKind {
    Boar,
    Drake,
    /// The Moonwyrm: the cat-dragon coiled in Starwell Glacier's Moonwell
    /// (sky map only). Killing it claims the well (`moonwell`).
    Moonwyrm,
}

impl BossKind {
    pub fn name(self) -> &'static str {
        match self {
            BossKind::Boar => "THE ELDER BOAR",
            BossKind::Drake => "THE STORM DRAKE",
            BossKind::Moonwyrm => "THE MOONWYRM",
        }
    }
    pub fn buff_name(self) -> &'static str {
        match self {
            BossKind::Boar => "THE WALLOW'S BOUNTY",
            BossKind::Drake => "STORMFIRE",
            BossKind::Moonwyrm => "THE MOONWELL",
        }
    }
    pub fn first_spawn(self) -> f32 {
        match self {
            BossKind::Boar => 180.0,
            BossKind::Drake => 360.0,
            BossKind::Moonwyrm => 240.0,
        }
    }
    pub fn respawn(self) -> f32 {
        match self {
            BossKind::Boar => 210.0,
            BossKind::Drake => 240.0,
            // back in the water as the claim it gave runs out, plus a minute
            BossKind::Moonwyrm => crate::moonwell::CLAIM_SECS + 60.0,
        }
    }
    pub fn base_hp(self) -> f32 {
        match self {
            BossKind::Boar => 2600.0,
            BossKind::Drake => 4600.0,
            BossKind::Moonwyrm => 7200.0,
        }
    }
    /// hp growth per minute of match time (so a late boar isn't free)
    pub fn hp_per_min(self) -> f32 {
        match self {
            BossKind::Boar => 90.0,
            BossKind::Drake => 130.0,
            BossKind::Moonwyrm => 180.0,
        }
    }
    pub fn reach(self) -> f32 {
        match self {
            BossKind::Boar => 4.5,
            BossKind::Drake => 13.0,
            BossKind::Moonwyrm => 7.0,
        }
    }
    pub fn color(self) -> [f32; 3] {
        match self {
            BossKind::Boar => [0.85, 0.7, 0.3],
            BossKind::Drake => [0.6, 0.7, 1.0],
            BossKind::Moonwyrm => [0.78, 0.86, 1.0],
        }
    }
    /// How far from home it fights: its pit, or the Moonwell's water.
    pub fn leash(self) -> f32 {
        match self {
            BossKind::Boar | BossKind::Drake => PIT_RADIUS,
            BossKind::Moonwyrm => crate::sky::POOL_R,
        }
    }
}

pub const PIT_RADIUS: f32 = 11.0;
pub const BUFF_SECS: f32 = 90.0;
pub const BOAR_GOLD: f32 = 250.0;
pub const BOUNTY_GOLD_MUL: f32 = 1.5;
pub const STORMFIRE_DMG_MUL: f32 = 1.18;
pub const STORMFIRE_STRUCT_BONUS: f32 = 80.0;
/// seconds without damage before a boss starts mending
pub const RESET_SECS: f32 = 8.0;
pub const RESET_REGEN_PER_S: f32 = 0.04;

#[derive(Clone, Debug)]
pub struct Boss {
    pub kind: BossKind,
    pub pos: Vec3,
    pub home: Vec3,
    pub yaw: f32,
    pub hp: f32,
    pub max_hp: f32,
    pub alive: bool,
    /// sim time it next (re)appears
    pub spawn_t: f32,
    pub atk_t: f32,
    pub special_t: f32,
    pub anim: f32,
    pub atk_anim: f32,
    pub last_dmg_t: f32,
    /// the ancestor it is angry at
    pub target: Option<usize>,
    pub last_hit_team: Option<Team>,
    pub kills: u32,
    /// The Moonwyrm's orbiting stars ready to throw (`moonwell::STARS`),
    /// and seconds until the next one gathers / the next throw.
    pub stars: u8,
    pub star_regen_t: f32,
    pub star_throw_t: f32,
    /// The Moonwyrm aloft: seconds into its flight (`wyrmflight`); when
    /// the next is due; seconds to its next falling star.
    pub flight: Option<f32>,
    pub next_flight_t: f32,
    pub drop_t: f32,
}

impl Boss {
    /// Up, and within reach: alive and not aloft (the Moonwyrm's flights).
    pub fn grounded(&self) -> bool {
        self.alive && self.flight.is_none()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Bosses {
    pub list: Vec<Boss>,
    /// per team: seconds left on the Wallow's Bounty / Stormfire
    pub bounty_t: [f32; 2],
    pub stormfire_t: [f32; 2],
    /// Per team: seconds left on a Moonwell claim, and the claim's number
    /// (0 = never; `moonwell::grants`).
    pub claim_t: [f32; 2],
    pub claim_id: [u32; 2],
    /// Claims made so far, both teams: the source of claim numbers.
    pub claims: u32,
}

/// The two pits, found deterministically in the west / east jungle at the
/// map's equator so both bases have the same walk.
pub fn pit_positions() -> [Vec3; 2] {
    match crate::map::active() {
        crate::map::MapId::Pass => return crate::pass::pit_positions(),
        crate::map::MapId::Sky => return crate::sky::pit_positions(),
        crate::map::MapId::Atlantis => return crate::atlantis::pit_positions(),
        crate::map::MapId::Maze => return crate::maze::pit_positions(),
        crate::map::MapId::Jungle => {}
    }
    let mut rng = Rng::new(0xB0A2);
    let s = MAP_SCALE;
    let bands: [((f32, f32), (f32, f32)); 2] = [((-70.0, -42.0), (-8.0, 8.0)), ((42.0, 70.0), (-8.0, 8.0))];
    let mut out = [Vec3::ZERO; 2];
    for (i, ((x0, x1), (z0, z1))) in bands.iter().enumerate() {
        let mut best = Vec3::new((x0 + x1) * 0.5 * s, 0.0, 0.0);
        let mut best_clear = f32::MIN;
        for _ in 0..300 {
            let x = rng.range(*x0, *x1) * s;
            let z = rng.range(*z0, *z1) * s;
            let clear = lane_clearance(x, z).min(river_dist(x, z) - 4.0);
            if clear > best_clear && Vec3::new(x, 0.0, z).length() < ARENA_RADIUS - PIT_RADIUS - 4.0 {
                best_clear = clear;
                best = Vec3::new(x, 0.0, z);
            }
        }
        out[i] = best;
    }
    out
}

/// True if a point is inside either pit (scene.rs keeps flora out).
pub fn in_pit(x: f32, z: f32) -> bool {
    pit_positions().iter().any(|p| Vec3::new(x - p.x, 0.0, z - p.z).length() < PIT_RADIUS + 3.0)
}

impl Bosses {
    pub fn new() -> Bosses {
        let pits = pit_positions();
        let mk = |kind: BossKind, home: Vec3| Boss {
            kind,
            pos: home,
            home,
            yaw: 0.0,
            hp: kind.base_hp(),
            max_hp: kind.base_hp(),
            alive: false,
            spawn_t: kind.first_spawn(),
            atk_t: 0.0,
            special_t: 5.0,
            anim: 0.0,
            atk_anim: 0.0,
            last_dmg_t: -100.0,
            target: None,
            last_hit_team: None,
            kills: 0,
            stars: crate::moonwell::STARS,
            star_regen_t: 0.0,
            star_throw_t: 2.0,
            flight: None,
            next_flight_t: kind.first_spawn() + crate::wyrmflight::FIRST_FLIGHT,
            drop_t: 0.0,
        };
        let mut list = vec![mk(BossKind::Boar, pits[0]), mk(BossKind::Drake, pits[1])];
        if crate::map::active() == crate::map::MapId::Sky {
            list.push(mk(BossKind::Moonwyrm, crate::sky::well_pos()));
        }
        Bosses { list, bounty_t: [0.0; 2], stormfire_t: [0.0; 2], claim_t: [0.0; 2], claim_id: [0; 2], claims: 0 }
    }

    pub fn gold_mul(&self, team: Team) -> f32 {
        if self.bounty_t[team as usize] > 0.0 { BOUNTY_GOLD_MUL } else { 1.0 }
    }
    pub fn dmg_mul(&self, team: Team) -> f32 {
        if self.stormfire_t[team as usize] > 0.0 { STORMFIRE_DMG_MUL } else { 1.0 }
    }
    pub fn struct_bonus(&self, team: Team) -> f32 {
        if self.stormfire_t[team as usize] > 0.0 { STORMFIRE_STRUCT_BONUS } else { 0.0 }
    }
}

impl Game {
    pub fn bosses_tick(&mut self, dt: f32) {
        let now = self.t;
        for t in 0..2 {
            self.bosses.bounty_t[t] = (self.bosses.bounty_t[t] - dt).max(0.0);
            self.bosses.stormfire_t[t] = (self.bosses.stormfire_t[t] - dt).max(0.0);
            self.bosses.claim_t[t] = (self.bosses.claim_t[t] - dt).max(0.0);
        }
        for bi in 0..self.bosses.list.len() {
            // --- (re)spawn
            if !self.bosses.list[bi].alive {
                if now >= self.bosses.list[bi].spawn_t {
                    let b = &mut self.bosses.list[bi];
                    b.alive = true;
                    b.max_hp = b.kind.base_hp() + b.kind.hp_per_min() * (now / 60.0);
                    b.hp = b.max_hp;
                    b.pos = b.home;
                    b.target = None;
                    b.last_hit_team = None;
                    b.stars = crate::moonwell::STARS;
                    b.flight = None;
                    b.next_flight_t = now + crate::wyrmflight::FIRST_FLIGHT;
                    let (kind, pos) = (b.kind, b.home);
                    self.events.push(Event::Announce { text: format!("{} has risen", kind.name()) });
                    self.events.push(Event::BossSpawn { kind, pos });
                }
                continue;
            }
            // the Moonwyrm aloft: no fighting, only flying (wyrmflight.rs)
            if self.bosses.list[bi].kind == BossKind::Moonwyrm && self.wyrm_flight_tick(bi, dt) {
                continue;
            }

            let (kind, pos, home) = {
                let b = &self.bosses.list[bi];
                (b.kind, b.pos, b.home)
            };
            // --- pick the angriest target: last attacker if in reach, else nearest in the pit
            let mut target: Option<usize> = None;
            let cur = self.bosses.list[bi].target;
            if let Some(ti) = cur {
                let g = &self.gods[ti];
                if g.respawn_t <= 0.0 && g.pos.distance(home) < kind.leash() + 6.0 {
                    target = Some(ti);
                }
            }
            if target.is_none() {
                let mut bd = kind.leash() + 4.0;
                for (gi, g) in self.gods.iter().enumerate() {
                    if g.respawn_t <= 0.0 {
                        let d = g.pos.distance(home);
                        if d < bd {
                            bd = d;
                            target = Some(gi);
                        }
                    }
                }
            }
            self.bosses.list[bi].target = target;

            // --- mend when left alone; leash back home
            let idle = now - self.bosses.list[bi].last_dmg_t > RESET_SECS;
            {
                let b = &mut self.bosses.list[bi];
                b.anim += dt * (if b.target.is_some() { 4.0 } else { 1.2 });
                b.atk_anim = (b.atk_anim - dt * 3.0).max(0.0);
                if idle {
                    b.hp = (b.hp + b.max_hp * RESET_REGEN_PER_S * dt).min(b.max_hp);
                }
            }

            // --- move + attack
            match target {
                Some(ti) => {
                    let tp = self.gods[ti].pos;
                    let to = Vec3::new(tp.x - pos.x, 0.0, tp.z - pos.z);
                    let d = to.length();
                    let reach = kind.reach();
                    let speed = match kind {
                        BossKind::Boar => 9.0,
                        BossKind::Drake => 6.0,
                        BossKind::Moonwyrm => 7.0,
                    };
                    let mut new_pos = pos;
                    if d > reach * 0.85 {
                        let step = to.normalize_or_zero() * speed * dt;
                        // never leave the pit
                        let cand = pos + step;
                        if cand.distance(home) < kind.leash() + 1.0 {
                            new_pos = cand;
                        }
                    }
                    let yaw = to.z.atan2(to.x);
                    {
                        let b = &mut self.bosses.list[bi];
                        b.pos = new_pos;
                        b.yaw = yaw;
                        b.atk_t -= dt;
                        b.special_t -= dt;
                    }
                    if d <= reach + 0.5 {
                        if self.bosses.list[bi].atk_t <= 0.0 {
                            let (dmg, cd) = match kind {
                                BossKind::Boar => (55.0 + now * 0.05, 1.6),
                                BossKind::Drake => (95.0 + now * 0.07, 1.9),
                                BossKind::Moonwyrm => (110.0 + now * 0.08, 1.5),
                            };
                            self.bosses.list[bi].atk_t = cd;
                            self.bosses.list[bi].atk_anim = 1.0;
                            match kind {
                                BossKind::Boar => {
                                    self.deal_damage(TargetRef::God(ti), dmg, false, None, 0.0, 0.0);
                                }
                                BossKind::Drake => {
                                    let from = new_pos + Vec3::Y * 3.5;
                                    self.events.push(Event::BoltFx { from, to: tp + Vec3::Y * 1.0, color: kind.color(), thick: 0.4 });
                                    self.deal_damage(TargetRef::God(ti), dmg, true, None, 0.0, 0.0);
                                }
                                BossKind::Moonwyrm => {
                                    // a starlit bite
                                    let from = new_pos + Vec3::Y * 2.5;
                                    self.events.push(Event::BoltFx { from, to: tp + Vec3::Y * 1.0, color: kind.color(), thick: 0.6 });
                                    self.deal_damage(TargetRef::God(ti), dmg, true, None, 0.0, 0.0);
                                }
                            }
                        }
                    }
                    // the special: boar slam (aoe + shove), drake breath (cone ahead)
                    if self.bosses.list[bi].special_t <= 0.0 && d < reach + 4.0 {
                        self.bosses.list[bi].special_t = match kind {
                            BossKind::Boar => 7.0,
                            BossKind::Drake => 9.0,
                            BossKind::Moonwyrm => 8.0,
                        };
                        self.bosses.list[bi].atk_anim = 1.4;
                        match kind {
                            BossKind::Boar => {
                                let dmg = 120.0 + now * 0.08;
                                self.events.push(Event::NovaFx { pos: new_pos, radius: 5.5, color: kind.color() });
                                let hits: Vec<usize> = self.gods.iter().enumerate().filter(|(_, g)| g.respawn_t <= 0.0 && g.pos.distance(new_pos) < 5.5).map(|(i, _)| i).collect();
                                for gi in hits {
                                    self.deal_damage(TargetRef::God(gi), dmg, false, None, 0.0, 0.0);
                                    let g = &mut self.gods[gi];
                                    let away = Vec3::new(g.pos.x - new_pos.x, 0.0, g.pos.z - new_pos.z).normalize_or_zero();
                                    g.pos += away * 3.5;
                                    g.slow = g.slow.max(0.3);
                                    g.slow_t = g.slow_t.max(1.0);
                                }
                            }
                            BossKind::Moonwyrm => {
                                // the Tide of Stars: the whole well flares,
                                // and everyone wading in it is struck and
                                // slowed -- fight it from the rim, or not at all
                                let dmg = 150.0 + now * 0.09;
                                let r = crate::sky::POOL_R;
                                self.events.push(Event::NovaFx { pos: home, radius: r, color: kind.color() });
                                let hits: Vec<usize> = self.gods.iter().enumerate().filter(|(_, g)| g.respawn_t <= 0.0 && Vec3::new(g.pos.x - home.x, 0.0, g.pos.z - home.z).length() < r).map(|(i, _)| i).collect();
                                for gi in hits {
                                    self.deal_damage(TargetRef::God(gi), dmg, true, None, 0.0, 0.0);
                                    let g = &mut self.gods[gi];
                                    g.slow = g.slow.max(0.35);
                                    g.slow_t = g.slow_t.max(1.5);
                                }
                            }
                            BossKind::Drake => {
                                let dmg = 170.0 + now * 0.1;
                                let dir = to.normalize_or_zero();
                                self.events.push(Event::BeamFx { from: new_pos + Vec3::Y * 3.0, dir, length: 14.0, width: 4.0, color: [0.7, 0.75, 1.0], dur: 0.6 });
                                let hits: Vec<usize> = self
                                    .gods
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, g)| {
                                        if g.respawn_t > 0.0 {
                                            return false;
                                        }
                                        let v = Vec3::new(g.pos.x - new_pos.x, 0.0, g.pos.z - new_pos.z);
                                        let along = v.dot(dir);
                                        along > 0.0 && along < 14.0 && (v - dir * along).length() < 2.5 + along * 0.2
                                    })
                                    .map(|(i, _)| i)
                                    .collect();
                                for gi in hits {
                                    self.deal_damage(TargetRef::God(gi), dmg, true, None, 0.0, 0.0);
                                }
                            }
                        }
                    }
                }
                None => {
                    // wander home, drop aggro
                    let b = &mut self.bosses.list[bi];
                    let to = Vec3::new(home.x - pos.x, 0.0, home.z - pos.z);
                    if to.length() > 0.5 {
                        b.pos += to.normalize() * 4.0 * dt;
                        b.yaw = to.z.atan2(to.x);
                    }
                }
            }
        }
    }

    /// Called from `deal_damage` for `TargetRef::Boss`.
    pub fn boss_take_damage(&mut self, bi: usize, dmg: f32, source: Option<usize>) {
        if !self.bosses.list[bi].grounded() {
            return;
        }
        let now = self.t;
        let (pos, dead, kind) = {
            let b = &mut self.bosses.list[bi];
            b.hp -= dmg;
            b.last_dmg_t = now;
            if let Some(s) = source {
                b.target = Some(s);
            }
            (b.pos, b.hp <= 0.0, b.kind)
        };
        if let Some(s) = source {
            let team = self.gods[s].team;
            self.bosses.list[bi].last_hit_team = Some(team);
        }
        self.events.push(Event::Damage { pos: pos + Vec3::Y * 3.0, amount: dmg, big: dmg > 150.0 });
        if dead {
            let team = self.bosses.list[bi].last_hit_team.or_else(|| source.map(|s| self.gods[s].team));
            {
                let b = &mut self.bosses.list[bi];
                b.alive = false;
                b.hp = 0.0;
                b.spawn_t = now + b.kind.respawn();
                b.kills += 1;
            }
            self.events.push(Event::BossDown { kind, pos, team });
            // a jungle kill: a kitten, if Claypaw's mudslide is running (`clay`)
            if let Some(s) = source {
                self.clay_kill_credit(s, crate::clay::KITS_PER_SMALL_KILL, pos);
            }
            if let Some(team) = team {
                let ti = team as usize;
                match kind {
                    BossKind::Boar => {
                        for g in self.gods.iter_mut().filter(|g| g.team == team) {
                            g.gold += BOAR_GOLD;
                        }
                        self.bosses.bounty_t[ti] = BUFF_SECS;
                    }
                    BossKind::Drake => {
                        self.bosses.stormfire_t[ti] = BUFF_SECS;
                    }
                    BossKind::Moonwyrm => {
                        self.bosses.claims += 1;
                        self.bosses.claim_id[ti] = self.bosses.claims;
                        self.bosses.claim_t[ti] = crate::moonwell::CLAIM_SECS;
                        // the other side's claim, if any, is broken
                        self.bosses.claim_t[1 - ti] = 0.0;
                    }
                }
                let killer = source.map(|s| self.god_name(s).to_string()).unwrap_or_else(|| team.name().to_string());
                self.events.push(Event::Kill { killer, victim: kind.name().to_string(), victim_team: team.other(), is_god: true });
                self.events.push(Event::Announce { text: format!("{} claim {}", team.name(), kind.buff_name()) });
            }
        }
    }

    /// AI: which boss (index) is worth contesting from `pos` for `team`, if
    /// any. A beast the team is already on gets rallied to from further away
    /// and fought down to a lower health line — that's how five AI end up
    /// on the same objective instead of trickling in one at a time.
    pub fn contestable_boss(&self, pos: Vec3, team: Team, hp_frac: f32) -> Option<usize> {
        let now = self.t;
        self.bosses
            .list
            .iter()
            .enumerate()
            .filter(|(_, b)| b.grounded())
            .filter_map(|(i, b)| {
                let engaged = b.last_hit_team == Some(team) && now - b.last_dmg_t < 6.0;
                // the Moonwyrm is worth a long walk: the map's centrepiece
                let far = if b.kind == BossKind::Moonwyrm { 3.0 } else { 1.0 };
                let (reach, line) = if engaged { (75.0 * far, 0.30) } else { (48.0 * far, 0.55) };
                let d = b.pos.distance(pos);
                if d < reach && hp_frac >= line {
                    Some((i, d))
                } else {
                    None
                }
            })
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .map(|(i, _)| i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> Game {
        let mut g = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        for u in g.gods.iter_mut() {
            u.ai = None;
        }
        g
    }

    #[test]
    fn pits_are_off_lane_and_equidistant() {
        let ps = pit_positions();
        for p in ps {
            assert!(lane_clearance(p.x, p.z) > PIT_RADIUS - 2.0, "pit on a lane at {p}: {}", lane_clearance(p.x, p.z));
            assert!(river_dist(p.x, p.z) > 8.0, "pit in the river at {p}");
        }
        let d = |p: Vec3, t: Team| p.distance(t.base_pos());
        for p in ps {
            assert!((d(p, Team::Blue) - d(p, Team::Red)).abs() < 12.0 * MAP_SCALE, "pit favours a side: {p}");
        }
    }

    #[test]
    fn boar_rises_at_three_minutes_bites_back_and_pays_out() {
        let mut g = game();
        g.t = 179.0;
        g.bosses_tick(0.5);
        assert!(!g.bosses.list[0].alive);
        g.t = 181.0;
        g.events.clear();
        g.bosses_tick(0.5);
        assert!(g.bosses.list[0].alive);
        assert!(g.events.iter().any(|e| matches!(e, Event::BossSpawn { .. })));
        // walk in and poke it: it should turn and hurt us
        let home = g.bosses.list[0].home;
        g.gods[0].pos = home + Vec3::new(3.0, 0.0, 0.0);
        let hp0 = g.gods[0].hp;
        g.deal_damage(TargetRef::Boss(0), 100.0, false, Some(0), 0.0, 0.0);
        assert_eq!(g.bosses.list[0].target, Some(0));
        for _ in 0..(4 * 30) {
            g.t += 1.0 / 30.0;
            g.bosses_tick(1.0 / 30.0);
        }
        assert!(g.gods[0].hp < hp0, "the boar bites");
        // kill it: the whole team gets paid and the bounty runs
        let gold: Vec<f32> = g.gods.iter().map(|u| u.gold).collect();
        g.deal_damage(TargetRef::Boss(0), 1e6, false, Some(0), 0.0, 0.0);
        assert!(!g.bosses.list[0].alive);
        for i in 0..5 {
            assert!((g.gods[i].gold - gold[i] - BOAR_GOLD).abs() < 1e-3, "blue gets paid");
        }
        assert!((g.gods[5].gold - gold[5]).abs() < 1e-3, "red does not");
        assert!(g.bosses.gold_mul(Team::Blue) > 1.0 && g.bosses.gold_mul(Team::Red) == 1.0);
        assert!(g.bosses.list[0].spawn_t > g.t + 200.0, "respawn timer set");
    }

    #[test]
    fn a_boss_left_alone_mends() {
        let mut g = game();
        g.t = 500.0;
        g.bosses_tick(0.1);
        let bi = 1;
        assert!(g.bosses.list[bi].alive, "the drake is up at 8:00");
        g.deal_damage(TargetRef::Boss(bi), 2000.0, true, Some(5), 0.0, 0.0);
        let hurt = g.bosses.list[bi].hp;
        // nobody near the pit: after the reset window it regenerates
        for _ in 0..(15 * 10) {
            g.t += 0.1;
            g.bosses_tick(0.1);
        }
        assert!(g.bosses.list[bi].hp > hurt + 500.0, "should mend: {} -> {}", hurt, g.bosses.list[bi].hp);
    }

    #[test]
    fn stormfire_scales_damage_dealt() {
        let mut g = game();
        g.bosses.stormfire_t[Team::Blue as usize] = 10.0;
        let hp = g.gods[5].hp;
        g.deal_damage(TargetRef::God(5), 100.0, false, Some(0), 1.0, 0.0);
        let dealt = hp - g.gods[5].hp;
        assert!((dealt - 100.0 * STORMFIRE_DMG_MUL).abs() < 1e-2, "dealt {dealt}");
    }
}
