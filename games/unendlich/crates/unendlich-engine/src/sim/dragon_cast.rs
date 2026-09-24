//! `sim::dragon_cast` — a dragon casting one of its `dragon_abilities`.
//!
//! **Wind-up.** Every cast gathers for the ability's `windup_s` before it
//! lands. That is the player's window: the shell shows the charge (the dragon
//! glows, the log names the ability and where it will land). Where it lands
//! is the dragon's mind at work ([`lead_target`], [`Caster::tick`]):
//! a dull dragon aims at where you stand when it starts; a cunning one leads
//! you by your velocity and keeps re-aiming while it gathers — up to a point
//! (`track_until`), after which even it is committed and you can still step
//! out of the way.
//!
//! **Release** yields an [`Area`]: the ability, where, which way. The shell
//! applies [`release_hit`] once, [`linger_hit`] every tick while the area
//! lasts, and [`terrain`] once — dragon-scale bending **reshapes the land**:
//! an Upheaval leaves a ring of stone pillars, a Rampart a wall, a Magma Rift a
//! trench of magma, a Thunderhead glass where the bolts struck, a Downburst a
//! crater. Terrain edits come back as [`Edit`]s in this module's own
//! [`Mat`]erials; the shell maps them onto its block ids.
//!
//! Pure: positions, velocities and dt in. No clock, RNG (seeded hashes only)
//! or world access (terrain height comes in as a closure).

use crate::sim::dragon_abilities::{Ability, Form, Repertoire, Shape};

// ─────────────────────────────────────────────────────────────────────────────
// Aim and the cast state machine
// ─────────────────────────────────────────────────────────────────────────────

/// Where to aim so the ability lands on you: your position plus your velocity
/// times the wind-up, scaled by cunning (0 = aims at where you are now).
pub fn lead_target(player: [f32; 3], player_vel: [f32; 3], windup_s: f32, cunning: f32) -> [f32; 3] {
    let k = windup_s.max(0.0) * cunning.clamp(0.0, 1.0);
    let v = |i: usize| if player_vel[i].is_finite() { player_vel[i].clamp(-20.0, 20.0) } else { 0.0 };
    [player[0] + v(0) * k, player[1], player[2] + v(2) * k]
}

/// Fraction of the wind-up during which a dragon of this cunning keeps
/// re-aiming. Dull 0 (locks at once); the sharpest 0.7 (the last 30% is
/// always committed: there is always a moment to dodge).
pub fn track_until(cunning: f32) -> f32 {
    0.7 * cunning.clamp(0.0, 1.0)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gathering {
    pub idx: usize,
    pub t: f32,
    pub target: [f32; 3],
    pub from: [f32; 3],
}

/// A released ability, alive for its linger time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub ability: Ability,
    pub center: [f32; 3],
    /// Unit direction dragon → target on the ground plane.
    pub dir: [f32; 2],
    pub from: [f32; 3],
    pub age: f32,
    /// Seed for this cast's jitter (pillar heights, bolt points).
    pub seed: u64,
}

impl Area {
    pub fn alive(&self) -> bool {
        self.age <= self.ability.linger_s
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CastTick {
    Idle,
    Gathering { idx: usize, charge01: f32, target: [f32; 3] },
    Released(Area),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Caster {
    pub cooldowns: Vec<f32>,
    pub gathering: Option<Gathering>,
    casts: u64,
}

impl Caster {
    pub fn new(rep: &Repertoire) -> Caster {
        Caster { cooldowns: vec![0.0; rep.abilities.len()], gathering: None, casts: 0 }
    }

    pub fn busy(&self) -> bool {
        self.gathering.is_some()
    }

    /// The best ready ability for a target `dist` away. `want_defence`:
    /// prefer a defensive form if one is ready. `self_radius`: area abilities
    /// centred on the target must not reach back to the dragon.
    pub fn choose(&self, rep: &Repertoire, dist: f32, self_radius: f32, want_defence: bool) -> Option<usize> {
        if self.busy() || !dist.is_finite() {
            return None;
        }
        let usable = |i: usize, a: &Ability| {
            self.cooldowns.get(i).copied().unwrap_or(f32::MAX) <= 0.0
                && dist <= a.range
                && match a.shape {
                    Shape::Ring | Shape::Disc => dist > a.radius + self_radius,
                    Shape::Line => dist > self_radius + 1.0,
                    Shape::Wall => true,
                }
        };
        let mut best: Option<(usize, f32)> = None;
        for (i, a) in rep.abilities.iter().enumerate() {
            if !usable(i, a) || a.defensive != want_defence {
                continue;
            }
            let value = a.hit + a.dps * a.linger_s * 0.5 + a.slow01 * 10.0 + if a.defensive { 50.0 } else { 0.0 };
            if best.map_or(true, |(_, v)| value > v) {
                best = Some((i, value));
            }
        }
        best.map(|b| b.0)
    }

    /// Start gathering ability `idx` at `target` from `from`.
    pub fn begin(&mut self, idx: usize, from: [f32; 3], target: [f32; 3]) -> bool {
        if self.busy() || self.cooldowns.get(idx).map_or(true, |c| *c > 0.0) {
            return false;
        }
        self.gathering = Some(Gathering { idx, t: 0.0, target, from });
        true
    }

    /// Advance. `aim`: where the dragon would aim now (its lead on you) —
    /// applied only while it is still tracking; `from`: where it is now.
    pub fn tick(&mut self, rep: &Repertoire, cunning: f32, aim: Option<[f32; 3]>, from: [f32; 3], dt: f32) -> CastTick {
        let dt = if dt.is_finite() { dt.clamp(0.0, 1.0) } else { 0.0 };
        for c in self.cooldowns.iter_mut() {
            *c = (*c - dt).max(0.0);
        }
        let Some(mut g) = self.gathering else { return CastTick::Idle };
        let Some(a) = rep.abilities.get(g.idx).copied() else {
            self.gathering = None;
            return CastTick::Idle;
        };
        g.t += dt;
        let charge = (g.t / a.windup_s.max(1e-3)).clamp(0.0, 1.0);
        if charge < track_until(cunning) {
            if let Some(t) = aim.filter(|t| t.iter().all(|v| v.is_finite())) {
                g.target = t;
            }
            g.from = from;
        }
        if charge >= 1.0 {
            self.gathering = None;
            self.cooldowns[g.idx] = a.cooldown_s;
            self.casts += 1;
            let (dx, dz) = (g.target[0] - g.from[0], g.target[2] - g.from[2]);
            let l = (dx * dx + dz * dz).sqrt().max(1e-3);
            let dir = [dx / l, dz / l];
            // A Rampart goes up between the dragon and you, a third of the way.
            let center = if a.shape == Shape::Wall {
                [g.from[0] + dx / 3.0, g.target[1], g.from[2] + dz / 3.0]
            } else {
                g.target
            };
            let seed = (self.casts.wrapping_mul(0x9E37_79B9_7F4A_7C15)) ^ (g.idx as u64) << 32;
            return CastTick::Released(Area { ability: a, center, dir, from: g.from, age: 0.0, seed });
        }
        self.gathering = Some(g);
        CastTick::Gathering { idx: g.idx, charge01: charge, target: g.target }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Effects on the player
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Hit {
    pub damage: f32,
    /// Velocity change to apply (m/s).
    pub knock: [f32; 3],
    pub slow01: f32,
    pub inside: bool,
}

fn h(x: f32, z: f32) -> f32 {
    (x * x + z * z).sqrt()
}

/// Is `p` inside the area, and how central (1 centre .. 0 edge)?
pub fn coverage(area: &Area, p: [f32; 3]) -> Option<f32> {
    let a = &area.ability;
    let (dx, dz) = (p[0] - area.center[0], p[2] - area.center[2]);
    let r = h(dx, dz);
    if (p[1] - area.center[1]).abs() > 6.0 + a.radius * 0.5 {
        return None; // far above or below (flying over, or in a cave)
    }
    match a.shape {
        Shape::Disc => (r < a.radius).then(|| 1.0 - r / a.radius),
        Shape::Ring => {
            let band = 1.0 + a.radius * 0.1;
            let rim = (r - a.radius).abs();
            if rim < band {
                Some(1.0 - rim / band)
            } else if r < a.radius * 0.3 {
                Some(1.0 - r / (a.radius * 0.3))
            } else {
                None
            }
        }
        Shape::Line => {
            let (fx, fz) = (p[0] - area.from[0], p[2] - area.from[2]);
            let along = fx * area.dir[0] + fz * area.dir[1];
            let across = (-fx * area.dir[1] + fz * area.dir[0]).abs();
            (along > 0.0 && along < a.range && across < a.radius).then(|| 1.0 - across / a.radius)
        }
        Shape::Wall => {
            let along = dx * -area.dir[1] + dz * area.dir[0];
            let across = (dx * area.dir[0] + dz * area.dir[1]).abs();
            (along.abs() < a.radius && across < 1.5).then_some(1.0)
        }
    }
}

fn push_dir(area: &Area, p: [f32; 3]) -> [f32; 2] {
    match area.ability.shape {
        Shape::Line | Shape::Wall => area.dir,
        _ => {
            let (dx, dz) = (p[0] - area.center[0], p[2] - area.center[2]);
            let l = h(dx, dz);
            if l < 1e-3 { area.dir } else { [dx / l, dz / l] }
        }
    }
}

/// The blow on release. Damage falls from full at the centre to half at the edge.
pub fn release_hit(area: &Area, p: [f32; 3]) -> Hit {
    let Some(c) = coverage(area, p) else { return Hit::default() };
    let a = &area.ability;
    let d = push_dir(area, p);
    Hit {
        damage: a.hit * (0.5 + 0.5 * c),
        knock: [d[0] * a.knock, a.lift, d[1] * a.knock],
        slow01: a.slow01,
        inside: true,
    }
}

/// While the area lingers: damage over `dt`, the slow, no knock.
pub fn linger_hit(area: &Area, p: [f32; 3], dt: f32) -> Hit {
    if !area.alive() {
        return Hit::default();
    }
    let Some(_) = coverage(area, p) else { return Hit::default() };
    let a = &area.ability;
    Hit { damage: a.dps * dt.clamp(0.0, 1.0), knock: [0.0; 3], slow01: a.slow01, inside: true }
}

// ─────────────────────────────────────────────────────────────────────────────
// Terrain
// ─────────────────────────────────────────────────────────────────────────────

/// Materials the shell maps onto its blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mat {
    Air,
    Stone,
    Basalt,
    Magma,
    Snow,
    Clay,
    Sand,
    Glass,
    Obsidian,
    Thorn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edit {
    pub pos: [i32; 3],
    pub mat: Mat,
}

fn hash01(seed: u64, i: u64) -> f32 {
    let mut z = seed ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
}

/// How the land changes. `ground(x, z)` is the top solid block's y at a
/// column. At most `max_edits` edits (a phone budget); pillars and walls are
/// built bottom-up so a truncated list is still standing structure.
pub fn terrain(area: &Area, ground: &impl Fn(i32, i32) -> Option<i32>, max_edits: usize) -> Vec<Edit> {
    let a = &area.ability;
    let mut out: Vec<Edit> = Vec::new();
    let c = area.center;
    let mut push = |x: i32, y: i32, z: i32, m: Mat, out: &mut Vec<Edit>| {
        if out.len() < max_edits {
            out.push(Edit { pos: [x, y, z], mat: m });
        }
    };
    let column = |x: f32, z: f32, height: i32, m: Mat, out: &mut Vec<Edit>, push: &mut dyn FnMut(i32, i32, i32, Mat, &mut Vec<Edit>)| {
        let (xi, zi) = (x.floor() as i32, z.floor() as i32);
        if let Some(g) = ground(xi, zi) {
            for dy in 1..=height {
                push(xi, g + dy, zi, m, out);
            }
        }
    };
    let surface = |x: f32, z: f32, m: Mat, depth: i32, out: &mut Vec<Edit>, push: &mut dyn FnMut(i32, i32, i32, Mat, &mut Vec<Edit>)| {
        let (xi, zi) = (x.floor() as i32, z.floor() as i32);
        if let Some(g) = ground(xi, zi) {
            for dy in 0..depth {
                push(xi, g - dy, zi, m, out);
            }
        }
    };
    let disc_cells = |r: f32| {
        let ri = r.ceil() as i32;
        let mut v = Vec::new();
        for dx in -ri..=ri {
            for dz in -ri..=ri {
                if ((dx * dx + dz * dz) as f32).sqrt() <= r {
                    v.push((c[0] + dx as f32, c[2] + dz as f32, (dx * 131 + dz * 7) as u64));
                }
            }
        }
        v
    };
    match a.form {
        Form::Upheaval => {
            // A ring of pillars at the rim and one at the heart.
            let n = (6.0 + a.radius * 1.2) as usize;
            let tall = (3.0 + a.radius * 0.6) as i32;
            column(c[0], c[2], tall + 2, Mat::Basalt, &mut out, &mut push);
            for k in 0..n {
                let th = std::f32::consts::TAU * k as f32 / n as f32;
                let hgt = tall - 1 + (hash01(area.seed, k as u64) * 3.0) as i32;
                column(c[0] + th.cos() * a.radius, c[2] + th.sin() * a.radius, hgt, Mat::Stone, &mut out, &mut push);
            }
        }
        Form::Rampart => {
            let m = if a.primary == 0 { Mat::Thorn } else { Mat::Stone };
            let len = a.radius as i32;
            let tall = (4.0 + a.radius * 0.3) as i32;
            for dy in 1..=tall {
                for s in -len..=len {
                    for t in 0..2 {
                        let x = c[0] - area.dir[1] * s as f32 + area.dir[0] * t as f32;
                        let z = c[2] + area.dir[0] * s as f32 + area.dir[1] * t as f32;
                        let (xi, zi) = (x.floor() as i32, z.floor() as i32);
                        if let Some(g) = ground(xi, zi) {
                            push(xi, g + dy, zi, m, &mut out);
                        }
                    }
                }
            }
        }
        Form::MagmaRift | Form::PlasmaLance => {
            let (m, depth) = if a.form == Form::MagmaRift { (Mat::Magma, 1) } else { (Mat::Obsidian, 1) };
            let len = a.range.min(48.0) as i32;
            let w = a.radius.max(1.0) as i32;
            for s in 2..len {
                for t in -w..=w {
                    let x = area.from[0] + area.dir[0] * s as f32 - area.dir[1] * t as f32 * 0.8;
                    let z = area.from[2] + area.dir[1] * s as f32 + area.dir[0] * t as f32 * 0.8;
                    surface(x, z, m, depth, &mut out, &mut push);
                }
            }
        }
        Form::Downburst => {
            for (x, z, _) in disc_cells(a.radius * 0.5) {
                surface(x, z, Mat::Air, 1, &mut out, &mut push);
            }
        }
        Form::Thunderhead => {
            for k in 0..5u64 {
                let th = hash01(area.seed, k) * std::f32::consts::TAU;
                let r = hash01(area.seed, k + 100).sqrt() * a.radius;
                surface(c[0] + th.cos() * r, c[2] + th.sin() * r, Mat::Glass, 2, &mut out, &mut push);
            }
        }
        Form::Blizzard | Form::Thornbrake | Form::Sandstorm | Form::Quagmire => {
            let (m, keep, above) = match a.form {
                Form::Blizzard => (Mat::Snow, 1.0, true),
                Form::Thornbrake => (Mat::Thorn, 0.4, true),
                Form::Sandstorm => (Mat::Sand, 0.3, true),
                _ => (Mat::Clay, 0.8, false),
            };
            for (x, z, k) in disc_cells(a.radius) {
                if hash01(area.seed, k) < keep {
                    if above {
                        column(x, z, 1, m, &mut out, &mut push);
                    } else {
                        surface(x, z, m, 1, &mut out, &mut push);
                    }
                }
            }
        }
        Form::Boulder => {
            // A giant rock where it lands: a rough sphere resting on the ground.
            let br = (a.radius * 0.7).clamp(1.0, 4.0);
            let ri = br.ceil() as i32;
            if let Some(g) = ground(c[0].floor() as i32, c[2].floor() as i32) {
                let cy = g as f32 + br;
                for dy in -ri..=ri {
                    for dx in -ri..=ri {
                        for dz in -ri..=ri {
                            let rough = br * (0.85 + 0.3 * hash01(area.seed, ((dx + 8) * 289 + (dy + 8) * 17 + dz + 8) as u64));
                            if ((dx * dx + dy * dy + dz * dz) as f32).sqrt() <= rough {
                                let m = if dy < 0 { Mat::Basalt } else { Mat::Stone };
                                push(c[0].floor() as i32 + dx, (cy + dy as f32) as i32, c[2].floor() as i32 + dz, m, &mut out);
                            }
                        }
                    }
                }
            }
        }
        Form::Torrent | Form::Firestorm | Form::MistVeil | Form::SmokeWall => {}
    }
    out
}

/// One line for a log.
pub fn describe_area(area: &Area) -> String {
    format!(
        "{} at ({:.0}, {:.0}, {:.0}), r {:.0} m",
        area.ability.form.name(),
        area.center[0],
        area.center[1],
        area.center[2],
        area.ability.radius
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::dragon_abilities::repertoire;
    use crate::sim::dragon_archetype::Genome;

    fn rep_of(el: u8) -> Repertoire {
        let mut g = Genome::from_seed_element(5, el);
        g.element[el as usize] = 0.95;
        g.bulk = 0.8;
        g.neck_len = 0.8;
        // These tests are about the element's main forms; the Boulder (a
        // Dust master's extra) has its own test.
        let mut r = repertoire(&g);
        r.abilities.retain(|a| a.form != Form::Boulder);
        r
    }

    fn flat(_x: i32, _z: i32) -> Option<i32> {
        Some(63)
    }

    fn cast_to_release(c: &mut Caster, rep: &Repertoire, cunning: f32, aim: impl Fn(f32) -> [f32; 3]) -> (Area, Vec<[f32; 3]>) {
        let mut t = 0.0;
        let mut targets = vec![];
        loop {
            match c.tick(rep, cunning, Some(aim(t)), [0.0, 64.0, 0.0], 0.05) {
                CastTick::Gathering { target, .. } => targets.push(target),
                CastTick::Released(a) => return (a, targets),
                CastTick::Idle => panic!("idle mid-cast"),
            }
            t += 0.05;
            assert!(t < 10.0);
        }
    }

    #[test]
    fn lead_scales_with_cunning_and_windup() {
        let p = [10.0, 64.0, 0.0];
        let v = [4.0, 0.0, 0.0];
        assert_eq!(lead_target(p, v, 1.5, 0.0), p);
        assert!((lead_target(p, v, 1.5, 1.0)[0] - 16.0).abs() < 1e-4);
        assert!(lead_target([0.0; 3], [f32::NAN, 0.0, 1e9], 1.0, 1.0).iter().all(|x| x.is_finite()));
    }

    #[test]
    fn a_cast_winds_up_then_releases_then_cools_down() {
        let rep = rep_of(3); // Upheaval
        let mut c = Caster::new(&rep);
        let idx = c.choose(&rep, 20.0, 3.0, false).expect("in range");
        assert_eq!(rep.abilities[idx].form, Form::Upheaval);
        assert!(c.begin(idx, [0.0, 64.0, 0.0], [20.0, 64.0, 0.0]));
        assert!(c.busy() && c.choose(&rep, 20.0, 3.0, false).is_none());
        let wind = rep.abilities[idx].windup_s;
        let (area, targets) = cast_to_release(&mut c, &rep, 0.0, |_| [20.0, 64.0, 0.0]);
        assert!(((targets.len() + 1) as f32 * 0.05 - wind).abs() < 0.06, "released after {} ticks for {wind} s", targets.len() + 1);
        assert_eq!(area.center, [20.0, 64.0, 0.0]);
        assert!(!c.busy() && c.cooldowns[idx] > 0.0);
        assert!(!c.begin(idx, [0.0; 3], [1.0; 3]), "on cooldown");
    }

    #[test]
    fn it_will_not_drop_an_area_on_itself() {
        let rep = rep_of(3);
        let c = Caster::new(&rep);
        let r = rep.abilities[0].radius;
        assert!(c.choose(&rep, r + 1.0, 3.0, false).is_none(), "too close for a ring on the target");
        assert!(c.choose(&rep, rep.abilities[0].range + 1.0, 3.0, false).is_none(), "out of range");
    }

    #[test]
    fn a_dull_dragon_locks_its_aim_a_cunning_one_tracks_then_commits() {
        let rep = rep_of(5); // Thunderhead, long wind-up
        let moving = |t: f32| [20.0 + 4.0 * t, 64.0, 0.0];
        let mut dull = Caster::new(&rep);
        dull.begin(0, [0.0, 64.0, 0.0], moving(0.0));
        let (a_dull, _) = cast_to_release(&mut dull, &rep, 0.0, moving);
        assert_eq!(a_dull.center[0], 20.0, "dull: where you stood");
        let mut sharp = Caster::new(&rep);
        sharp.begin(0, [0.0, 64.0, 0.0], moving(0.0));
        let (a_sharp, targets) = cast_to_release(&mut sharp, &rep, 1.0, moving);
        assert!(a_sharp.center[0] > 24.0, "sharp: followed you ({})", a_sharp.center[0]);
        // The last stretch is committed: the target stops moving.
        let n = targets.len();
        let tail = &targets[(n as f32 * 0.75) as usize..];
        assert!(tail.windows(2).all(|w| w[0] == w[1]), "still re-aiming in the last quarter");
    }

    #[test]
    fn coverage_follows_each_shape() {
        let rep = rep_of(3);
        let area = |i: usize, center: [f32; 3]| Area {
            ability: rep.abilities[i],
            center,
            dir: [1.0, 0.0],
            from: [0.0, 64.0, 0.0],
            age: 0.0,
            seed: 1,
        };
        let ring = area(0, [20.0, 64.0, 0.0]);
        let r = ring.ability.radius;
        assert!(coverage(&ring, [20.0, 64.0, 0.0]).is_some(), "the heart pillar");
        assert!(coverage(&ring, [20.0 + r, 64.0, 0.0]).is_some(), "the rim");
        assert!(coverage(&ring, [20.0 + r * 0.6, 64.0, 0.0]).is_none(), "between: safe");
        assert!(coverage(&ring, [20.0, 90.0, 0.0]).is_none(), "high above");
        let wall = area(1, [8.0, 64.0, 0.0]);
        assert_eq!(wall.ability.form, Form::Rampart);
        assert!(coverage(&wall, [8.5, 64.0, 2.0]).is_some());
        assert!(coverage(&wall, [12.0, 64.0, 0.0]).is_none());
        // A line from the dragon along +x.
        let lance = Area { ability: rep_of(4).abilities[0], ..area(0, [20.0, 64.0, 0.0]) };
        let mut line = lance;
        line.ability.shape = Shape::Line;
        assert!(coverage(&line, [10.0, 64.0, 0.5]).is_some());
        assert!(coverage(&line, [-5.0, 64.0, 0.0]).is_none(), "behind the dragon");
    }

    #[test]
    fn release_knocks_away_and_lingering_burns_until_it_ends() {
        let rep = rep_of(4); // Firestorm: a burning ring
        let a = Area { ability: rep.abilities[0], center: [20.0, 64.0, 0.0], dir: [1.0, 0.0], from: [0.0, 64.0, 0.0], age: 0.0, seed: 2 };
        let rim = [20.0 + a.ability.radius, 64.0, 0.0];
        let hit = release_hit(&a, rim);
        assert!(hit.inside && hit.damage > 0.0);
        let burn = linger_hit(&a, rim, 0.5);
        assert!((burn.damage - a.ability.dps * 0.5).abs() < 1e-4);
        let mut old = a;
        old.age = a.ability.linger_s + 0.1;
        assert_eq!(linger_hit(&old, rim, 0.5), Hit::default());
        // Upheaval throws you outward and up from its centre.
        let up = rep_of(3);
        let u = Area { ability: up.abilities[0], ..a };
        let h2 = release_hit(&u, [20.0 + u.ability.radius, 64.0, 0.0]);
        assert!(h2.knock[0] > 0.0 && h2.knock[1] > 0.0);
    }

    #[test]
    fn upheaval_raises_a_ring_of_pillars_on_the_ground() {
        let rep = rep_of(3);
        let a = Area { ability: rep.abilities[0], center: [20.0, 64.0, 0.0], dir: [1.0, 0.0], from: [0.0, 64.0, 0.0], age: 0.0, seed: 3 };
        let e = terrain(&a, &flat, 2000);
        assert!(!e.is_empty());
        assert!(e.iter().all(|x| x.pos[1] > 63 && matches!(x.mat, Mat::Stone | Mat::Basalt)));
        // Columns: distinct (x,z) — the heart plus ≥ 6 on the rim.
        let mut cols: Vec<(i32, i32)> = e.iter().map(|x| (x.pos[0], x.pos[2])).collect();
        cols.sort();
        cols.dedup();
        assert!(cols.len() >= 7, "{} columns", cols.len());
        // Deterministic per cast seed; capped.
        assert_eq!(e, terrain(&a, &flat, 2000));
        assert_eq!(terrain(&a, &flat, 10).len(), 10);
    }

    #[test]
    fn rampart_stands_across_the_line_of_attack() {
        let rep = rep_of(3);
        let i = rep.abilities.iter().position(|a| a.form == Form::Rampart).unwrap();
        let mut c = Caster::new(&rep);
        let idx = c.choose(&rep, 30.0, 3.0, true).unwrap();
        assert_eq!(idx, i);
        c.begin(idx, [0.0, 64.0, 0.0], [30.0, 64.0, 0.0]);
        let (a, _) = cast_to_release(&mut c, &rep, 0.5, |_| [30.0, 64.0, 0.0]);
        assert!((a.center[0] - 10.0).abs() < 1e-3, "a third of the way: {}", a.center[0]);
        let e = terrain(&a, &flat, 4000);
        let xs: Vec<i32> = e.iter().map(|x| x.pos[0]).collect();
        let zs: Vec<i32> = e.iter().map(|x| x.pos[2]).collect();
        let span = |v: &Vec<i32>| v.iter().max().unwrap() - v.iter().min().unwrap();
        assert!(span(&zs) > span(&xs) * 3, "a wall across x: span z {} x {}", span(&zs), span(&xs));
    }

    #[test]
    fn a_boulder_lands_as_a_rock_on_the_ground() {
        let mut g = Genome::from_seed_element(5, 3);
        g.element[3] = 0.95;
        let rep = repertoire(&g);
        let b = *rep.abilities.iter().find(|a| a.form == Form::Boulder).unwrap();
        let a = Area { ability: b, center: [20.5, 64.0, 0.5], dir: [1.0, 0.0], from: [0.0, 64.0, 0.0], age: 0.0, seed: 9 };
        let e = terrain(&a, &flat, 5000);
        assert!(e.len() > 20, "{} blocks", e.len());
        let ys: Vec<i32> = e.iter().map(|x| x.pos[1]).collect();
        assert!(*ys.iter().min().unwrap() >= 63, "rests on the ground (63), min y {}", ys.iter().min().unwrap());
        assert!(*ys.iter().max().unwrap() >= 65, "stands above it");
        let xs: Vec<i32> = e.iter().map(|x| x.pos[0]).collect();
        assert!(xs.iter().all(|x| (x - 20).abs() <= 4));
    }

    #[test]
    fn surface_forms_change_the_top_block_not_the_sky() {
        let mut g = Genome::from_seed_element(5, 3);
        g.element[3] = 0.9;
        g.element[4] = 0.8; // bred Dust+Fire
        let rep = repertoire(&g);
        let mr = rep.abilities.iter().find(|a| a.form == Form::MagmaRift).expect("magma rift");
        let a = Area { ability: *mr, center: [20.0, 64.0, 0.0], dir: [1.0, 0.0], from: [0.0, 64.0, 0.0], age: 0.0, seed: 4 };
        let e = terrain(&a, &flat, 5000);
        assert!(!e.is_empty() && e.iter().all(|x| x.pos[1] == 63 && x.mat == Mat::Magma));
        assert!(e.iter().all(|x| x.pos[0] >= 1), "the trench runs away from the dragon");
    }

    #[test]
    fn nothing_non_finite_leaks() {
        let rep = rep_of(2);
        let mut c = Caster::new(&rep);
        assert!(c.choose(&rep, f32::NAN, 3.0, false).is_none());
        c.begin(0, [0.0; 3], [20.0, 0.0, 0.0]);
        for _ in 0..100 {
            if let CastTick::Released(a) = c.tick(&rep, 0.5, Some([f32::NAN; 3]), [0.0; 3], f32::NAN) {
                assert!(a.center.iter().all(|v| v.is_finite()));
            }
        }
    }
}
