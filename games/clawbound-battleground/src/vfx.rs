// VFX: turns simulation events into microvoxel particles — sparks, bursts,
// lightning columns, beams, shield shells, dash trails, pull vortices,
// heal glyphs, telegraph discs — plus the weather layer: rain, snow, wind on
// the leaves, and the Sweetrain / Long Winter / Thunder Front map-event
// visuals. One dynamic instance buffer per frame.
//
// Precipitation is *stateless*: each frame the drops are hashed from a grid
// cell + time inside a box around the camera, so a full storm costs ~5k
// instances and no per-particle bookkeeping.

use crate::render::MAX_PARTICLES;
use crate::sim::{Event, Game, Rng, ZoneKind};
use crate::voxel::VoxInst;
use glam::Vec3;

pub struct Particle {
    pub pos: Vec3,
    pub vel: Vec3,
    pub life: f32,
    pub max_life: f32,
    pub size: f32,
    pub color: [f32; 3],
    pub alpha: f32,
    pub gravity: f32,
    pub drag: f32,
}

pub struct Beam {
    pub from: Vec3,
    pub dir: Vec3,
    pub length: f32,
    pub width: f32,
    pub color: [f32; 3],
    pub until: f32,
}

pub struct Ring {
    pub pos: Vec3,
    pub r0: f32,
    pub r1: f32,
    /// Seconds LEFT: counts down to 0 (unlike `Beam::until`, an absolute
    /// time). The channel sigil and the lightning ring were given absolute
    /// times -- stuck at their smallest radius for the whole game clock, a
    /// cream doughnut hiding a nova's dust and its caster.
    pub until: f32,
    pub dur: f32,
    pub color: [f32; 3],
    pub thick: f32,
}

/// A coloured light an ability effect gives off, for a moment (the ultra
/// preview lights its smooth characters with these: `Vfx::lights`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glow {
    pub pos: Vec3,
    pub color: [f32; 3],
    pub radius: f32,
    pub life: f32,
    pub max: f32,
    pub power: f32,
}

/// A cast winding up or channelling (Event::CastStart), drawn until it
/// fires: see `tick_channels`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Channel {
    pub god: usize,
    pub color: [f32; 3],
    pub t: f32,
    pub dur: f32,
    /// A rooted channel (the caster cannot move) or an ultimate: the
    /// bigger telling -- a rising column as well.
    pub big: bool,
}

/// A fading copy of a cat where it just was: a dash leaves a string of
/// them, a blink one at its origin. The scene draws the cat's current pose
/// there, transparent, in `color`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Afterimage {
    pub god: usize,
    pub pos: Vec3,
    pub yaw: f32,
    pub life: f32,
    pub max: f32,
    pub color: [f32; 3],
}

#[derive(Default)]
pub struct Vfx {
    /// Particles that are NOT light -- dust, grit: lit by the sun and shaded
    /// like the ground they came from. Every entry in `particles` is drawn
    /// self-lit (the magic), which turned a nova's dust into cream puffs.
    pub dust: Vec<Particle>,
    /// Afterimages of dashes and blinks (`tick_afterimages`).
    pub afterimages: Vec<Afterimage>,
    ghost_last: Vec<Vec3>,
    ghost_t: Vec<f32>,
    /// Casts being gathered, until they fire or are broken off.
    pub channels: Vec<Channel>,
    /// Coloured light from ability effects, fading (`glow_events`).
    pub glows: Vec<Glow>,
    pub particles: Vec<Particle>,
    pub beams: Vec<Beam>,
    pub rings: Vec<Ring>,
    pub time: f32,
    /// ambient jungle life (seeded lazily on first update)
    fireflies: Vec<(Vec3, f32)>,
    leaves: Vec<Particle>,
    leaf_spots: Vec<Vec3>,
    seeded: bool,
    /// ground-bird flocks that flush when an ancestor comes close
    flocks: Vec<Flock>,
    /// The ancestors' bodies: gait, posture, strikes, leaps. Advanced by
    /// `Scene::draw`, which is where the rigs and the world probe are.
    pub puppets: crate::puppet::Troupe,
    /// Blast fronts and broken ground. Ticked here, where the weather is.
    pub ground: crate::groundfx::GroundFx,
    /// The sky map's mist: tiny trails curling over the stone and ice round
    /// the player, and mist spilling off the islands' edges (`tick_wisps`).
    pub wisps: Vec<Wisp>,
    wisp_seed: u64,
    /// Snow on the ground, drifting and keeping prints (ultra only;
    /// `snowpack`), the map it was laid for, where each cat was last
    /// frame (for the trenches), and the wind last seen.
    pub snow: Option<crate::snowpack::Snowpack>,
    snow_map: Option<crate::map::MapId>,
    snow_last: Vec<Vec3>,
    snow_wind: Vec3,
    /// Dashes through the sky map's cloud, parting it as they pass: the
    /// scene pushes the billows aside along each (`Gust::push`).
    pub gusts: Vec<Gust>,
    crumble_t: f32,
    crumble_seed: u64,
}

/// A dash's wake through cloud: from `from` to `to`, `age` seconds ago.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gust {
    pub from: Vec3,
    pub to: Vec3,
    pub age: f32,
}

/// How long a dash's wake holds the cloud open.
pub const GUST_SECS: f32 = 3.5;

impl Gust {
    /// How far a billow at `p` is pushed aside by this wake, and which way:
    /// out from the dash's line, strongest along it and just after, closing
    /// back over `GUST_SECS` with a little swirl.
    pub fn push(&self, p: Vec3) -> Vec3 {
        let ab = Vec3::new(self.to.x - self.from.x, 0.0, self.to.z - self.from.z);
        let ap = Vec3::new(p.x - self.from.x, 0.0, p.z - self.from.z);
        let t = (ap.dot(ab) / ab.length_squared().max(1e-4)).clamp(0.0, 1.0);
        let off = ap - ab * t;
        let d = off.length();
        let reach = 12.0;
        if d > reach || self.age > GUST_SECS {
            return Vec3::ZERO;
        }
        let fade = 1.0 - self.age / GUST_SECS;
        let near = 1.0 - d / reach;
        let away = if d > 1e-3 { off / d } else { Vec3::new(-ab.z, 0.0, ab.x).normalize_or_zero() };
        // swirl: the parted cloud curls round behind the dash
        let curl = Vec3::new(-away.z, 0.0, away.x) * (self.age * 2.0).sin() * 0.5;
        (away + curl) * 8.0 * near * fade * fade
    }
}

/// One trail of mist: a point that wanders a curling path, leaving a
/// thread of tiny motes behind it.
pub struct Wisp {
    pub pos: Vec3,
    pub age: f32,
    pub life: f32,
    pub seed: f32,
    /// Spilling off an island's edge (outward and down), not curling over it.
    pub spill: Option<Vec3>,
    /// Spindrift: powder the wind is skimming off a blown stretch of snow
    /// (ultra's snowpack), running low and fast downwind.
    pub drift: bool,
}

/// Wisps alive at once round the player.
pub const WISPS: usize = 45;

/// Where a wisp of mist drifts at `p`, `age` into its life: a slowly
/// turning field, and the wisp's own curl tightening as it ages, so each
/// trail is a spiral, not a line.
pub fn wisp_vel(p: Vec3, t: f32, age: f32, seed: f32) -> Vec3 {
    let a = (p.x * 0.21 + t * 0.3).sin() * 2.0 + (p.z * 0.17 - t * 0.25).cos() * 2.0 + seed + age * 1.8;
    Vec3::new(a.cos(), 0.0, a.sin()) * 1.3
}

/// A handful of small birds pecking at a jungle spot. When flushed they burst
/// upward, wheel once, and settle at a new spot.
pub struct Flock {
    pub anchor: Vec3,
    pub next_anchor: Vec3,
    pub n: usize,
    /// 0 = grounded; rises to 1 on flush and decays as they land
    pub flight: f32,
    pub phase: f32,
}

impl Vfx {
    /// A cat arriving: a burst of starry mist. Stars -- tiny, near-white,
    /// flung up and out, slowed by the air and drifting -- over a low ring
    /// of pale mist in the team's colour that rolls outward and lifts, and a
    /// thin column of light straight up. Deterministic per `seed` so the
    /// headless harness renders the same burst every run.
    pub fn star_burst(&mut self, pos: Vec3, team_rgb: [f32; 3], seed: f32) {
        let tint = |c: [f32; 3], k: f32| [c[0] + (1.0 - c[0]) * k, c[1] + (1.0 - c[1]) * k, c[2] + (1.0 - c[2]) * k];
        for i in 0..44 {
            let s = seed * 31.7 + i as f32 * 1.618;
            let dir = Vec3::new(rnd_jitter(s), 0.55 + rnd_jitter(s + 2.0).abs() * 0.9, rnd_jitter(s + 4.0)).normalize_or_zero();
            let speed = 5.0 + rnd_jitter(s + 6.0).abs() * 6.0;
            self.particles.push(Particle {
                pos: pos + Vec3::Y * 0.8,
                vel: dir * speed,
                life: 1.1 + rnd_jitter(s + 8.0).abs() * 0.9,
                max_life: 2.0,
                size: 0.10 + rnd_jitter(s + 9.0).abs() * 0.14,
                color: tint(team_rgb, 0.8),
                alpha: 1.0,
                gravity: -0.4, // stars float up as they slow
                drag: 2.6,
            });
        }
        // The mist is MANY small faint puffs: particles draw as voxel cubes,
        // and the first version's metre-wide puffs read as white slabs.
        for i in 0..70 {
            let s = seed * 17.3 + i as f32 * 2.414;
            let a = i as f32 / 70.0 * std::f32::consts::TAU + rnd_jitter(s) * 0.3;
            let out = Vec3::new(a.cos(), 0.0, a.sin());
            self.particles.push(Particle {
                pos: pos + out * (0.4 + rnd_jitter(s + 7.0).abs() * 0.8) + Vec3::Y * (0.2 + rnd_jitter(s + 8.0).abs() * 0.8),
                vel: out * (1.6 + rnd_jitter(s + 1.0).abs() * 2.2) + Vec3::Y * (0.3 + rnd_jitter(s + 2.0).abs() * 0.6),
                life: 1.4 + rnd_jitter(s + 3.0).abs() * 1.0,
                max_life: 2.4,
                size: 0.22 + rnd_jitter(s + 5.0).abs() * 0.22,
                color: tint(team_rgb, 0.6),
                alpha: 0.22,
                gravity: -0.35,
                drag: 1.4,
            });
        }
        for i in 0..14 {
            let s = seed * 7.9 + i as f32;
            self.particles.push(Particle {
                pos: pos + Vec3::new(rnd_jitter(s) * 0.25, 0.3 + i as f32 * 0.25, rnd_jitter(s + 1.0) * 0.25),
                vel: Vec3::Y * (7.0 + i as f32 * 0.6),
                life: 0.7,
                max_life: 0.7,
                size: 0.16,
                color: tint(team_rgb, 0.9),
                alpha: 0.9,
                gravity: 0.0,
                drag: 1.8,
            });
        }
    }

    fn seed_ambient(&mut self) {
        self.seeded = true;
        let mut rng = Rng::new(0x1F1E_5EED);
        let mut tries = 0usize;
        while self.fireflies.len() < 46 && tries < 600 {
            tries += 1;
            let a = rng.f32() * std::f32::consts::TAU;
            let r = 28.0 + rng.f32() * 102.0;
            let x = a.cos() * r;
            let z = a.sin() * r;
            if crate::consts::lane_clearance(x, z) < 8.0 || crate::consts::river_dist(x, z) < 7.0 {
                continue;
            }
            self.fireflies.push((Vec3::new(x, 1.0 + rng.f32() * 2.0, z), rng.f32() * 40.0));
        }
        let mut tries = 0usize;
        while self.leaf_spots.len() < 24 && tries < 600 {
            tries += 1;
            let a = rng.f32() * std::f32::consts::TAU;
            let r = 24.0 + rng.f32() * 108.0;
            let x = a.cos() * r;
            let z = a.sin() * r;
            if crate::consts::lane_clearance(x, z) < 6.0 || crate::consts::river_dist(x, z) < 6.0 {
                continue;
            }
            self.leaf_spots.push(Vec3::new(x, 0.0, z));
        }
        let mut tries = 0usize;
        while self.flocks.len() < 9 && tries < 400 {
            tries += 1;
            let a = rng.f32() * std::f32::consts::TAU;
            let r = 24.0 + rng.f32() * 100.0;
            let p = Vec3::new(a.cos() * r, 0.0, a.sin() * r);
            if crate::consts::lane_clearance(p.x, p.z) < 7.0 || crate::consts::river_dist(p.x, p.z) < 7.0 {
                continue;
            }
            self.flocks.push(Flock { anchor: p, next_anchor: p, n: 4 + (rng.next_u() % 4) as usize, flight: 0.0, phase: rng.f32() * 6.0 });
        }
        for i in 0..26 {
            let spot = self.leaf_spots[i % self.leaf_spots.len().max(1)];
            self.leaves.push(Particle {
                pos: spot + Vec3::new(0.0, 1.0 + (i % 6) as f32 * 1.2, 0.0),
                vel: Vec3::ZERO,
                life: 4.0 + (i % 9) as f32,
                max_life: 12.0,
                size: 0.2,
                color: [0.2, 0.3, 0.1],
                alpha: 0.9,
                gravity: 0.0,
                drag: 0.0,
            });
        }
    }

    /// Turn this frame's effects into coloured glows, and fade the old ones.
    pub fn glow_events(&mut self, dt: f32, events: &[Event], game: &Game) {
        for g in self.glows.iter_mut() {
            g.life -= dt;
        }
        self.glows.retain(|g| g.life > 0.0);
        let mut add = |pos: Vec3, color: [f32; 3], radius: f32, life: f32, power: f32| {
            if pos.is_finite() && radius > 0.0 && life > 0.0 {
                self.glows.push(Glow { pos, color, radius, life, max: life, power });
            }
        };
        for e in events {
            match e {
                Event::Cast { god, slot, .. } => {
                    if let (Some(g), Some(def)) = (game.gods.get(*god), crate::gods::ROSTER.get(game.gods.get(*god).map_or(0, |g| g.def_id as usize))) {
                        let c = def.abilities[(*slot as usize).min(3)].color;
                        add(g.pos + Vec3::Y * 1.2, c, 7.0, 0.45, 3.0);
                    }
                }
                Event::Explosion { pos, radius, color } | Event::NovaFx { pos, radius, color } => add(*pos + Vec3::Y, *color, radius * 2.2, 0.5, 5.0),
                Event::BoltFx { to, color, .. } => add(*to + Vec3::Y, *color, 5.0, 0.3, 3.0),
                // lightning: blinding, blue-white, brief -- the brightest light
                // there is, so in ultra every cat near it throws a shadow
                Event::StrikeFx { at, color } => {
                    let c = [0.72 + color[0] * 0.28, 0.8 + color[1] * 0.2, 1.0];
                    add(*at + Vec3::Y * 3.0, c, 18.0, 0.2, 16.0);
                }
                Event::StormFx { pos, radius, color, dur } => add(*pos + Vec3::Y * 2.0, *color, radius * 1.8, dur.min(3.0), 2.5),
                // a fallen star: as bright as lightning, and wider
                Event::StarfallFx { at, radius } => add(*at + Vec3::Y * 3.0, [0.88, 0.94, 1.0], radius * 4.0, 0.6, 18.0),
                Event::HealFx { pos, radius } => add(*pos + Vec3::Y, [0.45, 1.0, 0.55], radius * 1.5, 0.6, 2.0),
                Event::ShieldFx { pos, dur, color } => add(*pos + Vec3::Y, *color, 4.0, dur.min(1.5), 1.5),
                Event::BeamFx { from, dir, length, color, dur, .. } => add(*from + *dir * (length * 0.5) + Vec3::Y, *color, length * 0.6 + 2.0, dur.min(2.0), 3.0),
                _ => {}
            }
        }
        // a flurry must not grow without bound
        if self.glows.len() > 64 {
            self.glows.drain(0..self.glows.len() - 64);
        }
    }

    /// Up to `max` coloured lights for the renderer, the ones that matter
    /// most at `near` (the brightest there, not merely the closest): each
    /// (xyz, radius) and (rgb * intensity, fading out over its life).
    pub fn lights(&self, near: Vec3, max: usize) -> Vec<([f32; 4], [f32; 4])> {
        let mut v: Vec<(f32, ([f32; 4], [f32; 4]))> = self
            .glows
            .iter()
            .map(|g| {
                let fade = (g.life / g.max).clamp(0.0, 1.0);
                let i = g.power * fade * fade;
                let d = (g.pos - near).length();
                let weight = i / (1.0 + d * d * 0.01);
                (weight, ([g.pos.x, g.pos.y, g.pos.z, g.radius], [g.color[0] * i, g.color[1] * i, g.color[2] * i, 0.0]))
            })
            .collect();
        v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        v.into_iter().take(max).map(|x| x.1).collect()
    }

    /// Channelling, made visible: while a cast winds up, motes of its colour
    /// spiral in to the caster's chest -- faster and thicker as the charge
    /// builds, so its timing can be read -- a sigil ring grows under it to
    /// full size at the moment of release, and a coloured light brightens
    /// (in ultra it casts traced shadows). A rooted channel or an ultimate
    /// raises a column of motes too. It ends when the cast fires, or just
    /// stops if the caster dies.
    pub fn tick_channels(&mut self, dt: f32, events: &[Event], game: &Game) {
        for e in events {
            match e {
                Event::CastStart { god, slot, dur, .. } => {
                    let Some(g) = game.gods.get(*god) else { continue };
                    let ab = crate::gods::ROSTER[g.def_id as usize].abilities[(*slot as usize).min(3)];
                    let (_, move_mul) = crate::abilities::cast_timing(&ab);
                    let big = move_mul <= 0.0 || ab.ult;
                    self.channels.retain(|c| c.god != *god);
                    self.channels.push(Channel { god: *god, color: ab.color, t: 0.0, dur: dur.max(0.05), big });
                    // the sigil: grows from nothing to full at the release
                    self.rings.push(Ring { pos: g.pos + Vec3::Y * 0.06, r0: 0.3, r1: if big { 2.6 } else { 1.6 }, until: *dur, dur: *dur, color: ab.color, thick: if big { 0.22 } else { 0.14 } });
                }
                Event::Cast { god, .. } => self.channels.retain(|c| c.god != *god),
                _ => {}
            }
        }
        let mut rng = crate::sim::Rng(((self.time * 1000.0) as u64) ^ 0x51ED_C0DE | 1);
        let mut born: Vec<Particle> = Vec::new();
        let mut lights: Vec<Glow> = Vec::new();
        self.channels.retain_mut(|c| {
            let Some(g) = game.gods.get(c.god) else { return false };
            if g.respawn_t > 0.0 || c.t > c.dur + 0.3 {
                return false;
            }
            c.t += dt;
            let charge = (c.t / c.dur).clamp(0.0, 1.0);
            let core = g.pos + Vec3::Y * 1.0;
            let bright = [c.color[0] * 1.4, c.color[1] * 1.4, c.color[2] * 1.4];
            // gathering motes: more, and quicker, as the charge builds
            let rate = (40.0 + 140.0 * charge) * if c.big { 1.6 } else { 1.0 };
            let n = (rate * dt + rng.f32()) as usize;
            for _ in 0..n {
                let a = rng.f32() * std::f32::consts::TAU;
                let r = 1.4 + rng.f32() * (if c.big { 1.4 } else { 0.8 });
                let from = core + Vec3::new(a.cos() * r, (rng.f32() - 0.3) * 1.4, a.sin() * r);
                let life = 0.45 - 0.2 * charge;
                // inward, with a swirl
                let inward = (core - from) / life;
                let swirl = Vec3::new(-a.sin(), 0.0, a.cos()) * (2.0 + 2.0 * charge);
                born.push(Particle { pos: from, vel: inward + swirl, life, max_life: life, size: 0.10 + 0.08 * charge, color: bright, alpha: 0.9, gravity: 0.0, drag: 0.0 });
            }
            if c.big {
                let n = ((30.0 + 60.0 * charge) * dt + rng.f32()) as usize;
                for _ in 0..n {
                    let a = rng.f32() * std::f32::consts::TAU;
                    let from = g.pos + Vec3::new(a.cos() * 0.8, 0.1, a.sin() * 0.8);
                    born.push(Particle { pos: from, vel: Vec3::new(0.0, 3.5 + 2.0 * charge, 0.0), life: 0.7, max_life: 0.7, size: 0.09, color: bright, alpha: 0.8, gravity: 0.0, drag: 0.0 });
                }
            }
            // the charge's light: a glow that lives one frame, re-lit each
            lights.push(Glow { pos: core, color: c.color, radius: 6.0 + 4.0 * charge, life: dt * 1.5 + 0.001, max: dt * 1.5 + 0.001, power: 1.0 + 5.0 * charge * charge });
            true
        });
        self.particles.extend(born);
        self.glows.extend(lights);
    }

    /// A lightning strike: a jagged bolt from the sky with a fork or two,
    /// drawn in HDR white tinted by the storm (it blooms), a flash ring and
    /// sparks where it lands. Its light is `glow_events`'.
    pub fn lightning(&mut self, at: Vec3, color: [f32; 3]) {
        let mut rng = crate::sim::Rng(((self.time * 7919.0) as u64) ^ ((at.x * 131.0) as i64 as u64) ^ 0xB017 | 1);
        let white = [2.6 + color[0] * 0.8, 2.8 + color[1] * 0.6, 3.4];
        let top = at + Vec3::Y * 14.0;
        // the main channel: zigzag, wildest in the middle, pinned at both ends
        let segs = 12;
        let mut path = vec![top];
        for k in 1..segs {
            let u = k as f32 / segs as f32;
            let wild = (u * std::f32::consts::PI).sin() * 0.9;
            let p = top.lerp(at, u) + Vec3::new((rng.f32() - 0.5) * 2.0 * wild, 0.0, (rng.f32() - 0.5) * 2.0 * wild);
            path.push(p);
        }
        path.push(at);
        let mut bolt = |a: Vec3, b: Vec3, size: f32, v: &mut Vec<Particle>| {
            let n = ((b - a).length() / 0.14).ceil().max(1.0) as usize;
            for i in 0..=n {
                let p = a.lerp(b, i as f32 / n as f32);
                v.push(Particle { pos: p, vel: Vec3::ZERO, life: 0.16, max_life: 0.16, size, color: white, alpha: 1.0, gravity: 0.0, drag: 0.0 });
            }
        };
        let mut out = Vec::new();
        for w in path.windows(2) {
            bolt(w[0], w[1], 0.16, &mut out);
        }
        // a fork or two off the upper half
        for f in 0..2 {
            let from = path[2 + f * 3];
            let dir = Vec3::new(rng.f32() - 0.5, -0.9, rng.f32() - 0.5).normalize();
            let mut p = from;
            for _ in 0..4 {
                let q = p + dir * 1.1 + Vec3::new((rng.f32() - 0.5) * 0.8, 0.0, (rng.f32() - 0.5) * 0.8);
                bolt(p, q, 0.09, &mut out);
                p = q;
            }
        }
        // sparks where it lands
        for i in 0..18 {
            let a = i as f32 / 18.0 * std::f32::consts::TAU;
            out.push(Particle { pos: at + Vec3::Y * 0.2, vel: Vec3::new(a.cos() * 5.0, 3.0 + rng.f32() * 3.0, a.sin() * 5.0), life: 0.35, max_life: 0.35, size: 0.1, color: white, alpha: 1.0, gravity: 14.0, drag: 0.5 });
        }
        self.particles.extend(out);
        self.rings.push(Ring { pos: at + Vec3::Y * 0.08, r0: 0.3, r1: 2.8, until: 0.28, dur: 0.28, color: [color[0] * 0.5 + 0.5, color[1] * 0.5 + 0.5, 1.0], thick: 0.2 });
    }

    /// Mist on the sky map (nothing elsewhere): keeps [`WISPS`] trails alive
    /// within 35 u of the player's cat -- most curling low over the floor,
    /// some spilling off a nearby edge, out and down into the sky -- and
    /// lays a thread of tiny motes behind each.
    pub fn tick_wisps(&mut self, dt: f32, game: &Game) {
        let sky = crate::map::active() == crate::map::MapId::Sky;
        if !(sky || self.snow.is_some()) || game.gods.is_empty() || dt <= 0.0 {
            self.wisps.clear();
            return;
        }
        let focus = game.gods[0].pos;
        let floor = crate::sky::floor();
        let wind = game.weather.atmosphere().wind;
        let mut rnd = |s: &mut u64| {
            *s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((*s >> 33) as f32) / (1u64 << 31) as f32
        };
        let mut seed = self.wisp_seed | 1;
        // retire the old and the strayed; top up
        self.wisps.retain(|w| w.age < w.life && Vec3::new(w.pos.x - focus.x, 0.0, w.pos.z - focus.z).length() < 45.0);
        let mut tries = 0;
        while self.wisps.len() < WISPS && tries < 12 {
            tries += 1;
            let a = rnd(&mut seed) * std::f32::consts::TAU;
            let r = 4.0 + rnd(&mut seed).sqrt() * 31.0;
            let p = Vec3::new(focus.x + a.cos() * r, 0.0, focus.z + a.sin() * r);
            let d = if sky { floor.sdf(p.x, p.z) } else { -5.0 };
            let wseed = rnd(&mut seed) * 6.28;
            let blown = self.snow.as_ref().map_or(0.0, |s| if s.depth_at(p.x, p.z) > 0.02 { s.blow_at(p.x, p.z) } else { 0.0 });
            if blown > 0.55 && wind.length() > 1.0 && d < -0.5 {
                self.wisps.push(Wisp { pos: p + Vec3::Y * 0.1, age: 0.0, life: 1.2 + rnd(&mut seed) * 1.3, seed: wseed, spill: None, drift: true });
            } else if !sky {
                continue;
            } else if (-1.5..1.0).contains(&d) && rnd(&mut seed) < 0.5 {
                // at an edge: spill outward (down the distance gradient) and down
                let e = 0.5;
                let out = Vec3::new(floor.sdf(p.x + e, p.z) - floor.sdf(p.x - e, p.z), 0.0, floor.sdf(p.x, p.z + e) - floor.sdf(p.x, p.z - e)).normalize_or_zero();
                self.wisps.push(Wisp { pos: p + Vec3::Y * 0.4, age: 0.0, life: 5.0 + rnd(&mut seed) * 3.0, seed: wseed, spill: Some(out), drift: false });
            } else if d < -0.5 {
                self.wisps.push(Wisp { pos: p + Vec3::Y * 0.2, age: 0.0, life: 2.5 + rnd(&mut seed) * 2.5, seed: wseed, spill: None, drift: false });
            }
        }
        self.wisp_seed = seed;
        let t = self.time;
        let emit = (t * 60.0) as i64 % 2 == 0;
        let mut motes = Vec::new();
        for w in self.wisps.iter_mut() {
            w.age += dt;
            match w.spill {
                None if w.drift => {
                    let v = wind * 1.3 + wisp_vel(w.pos, t, w.age, w.seed) * 0.3;
                    w.pos.x += v.x * dt;
                    w.pos.z += v.z * dt;
                    w.pos.y = 0.08 + 0.14 * (w.age * 5.0 + w.seed).sin().abs();
                }
                Some(out) => {
                    // over the lip, then falling away and curling as it goes
                    let fall = (w.age - 0.8).max(0.0);
                    w.pos += (out * 1.4 + wisp_vel(w.pos, t, w.age, w.seed) * 0.4 - Vec3::Y * fall * 0.9) * dt;
                }
                None => {
                    let v = wisp_vel(w.pos, t, w.age, w.seed);
                    w.pos.x += v.x * dt;
                    w.pos.z += v.z * dt;
                    w.pos.y = 0.18 + 0.22 * (w.age * 2.1 + w.seed).sin().abs();
                }
            }
            if emit {
                // fade in and out over the wisp's life
                let k = (w.age / 0.6).min(1.0) * ((w.life - w.age) / 0.8).clamp(0.0, 1.0);
                motes.push(Particle { pos: w.pos, vel: Vec3::Y * 0.05, life: 1.1, max_life: 1.1, size: 0.09, color: [0.92, 0.95, 1.0], alpha: 0.75 * k, gravity: 0.0, drag: 0.0 });
            }
        }
        self.particles.extend(motes);
    }

    /// The sky map, alive: dashes through cloud part it (a wake of mist,
    /// `gusts` for the scene), and bits of the islands' edges crumble off
    /// and fall away into the sky near the player.
    pub fn tick_sky_life(&mut self, dt: f32, events: &[Event], game: &Game) {
        if crate::map::active() != crate::map::MapId::Sky {
            self.gusts.clear();
            return;
        }
        self.gusts.retain_mut(|g| {
            g.age += dt;
            g.age < GUST_SECS
        });
        let mut cs: Option<Vec<crate::skycloud::Cloud>> = None;
        for e in events {
            if let Event::DashFx { pos, dir, .. } = *e {
                let cs = cs.get_or_insert_with(|| crate::skycloud::clouds(game.cloud_seed, game.t));
                let from = pos - dir * 6.0;
                let fog = crate::skycloud::density(cs, pos.x, pos.z).max(crate::skycloud::density(cs, from.x, from.z));
                if fog < 0.1 {
                    continue;
                }
                self.gusts.push(Gust { from, to: pos + dir * 2.0, age: 0.0 });
                if self.gusts.len() > 24 {
                    self.gusts.remove(0);
                }
                // the wake: mist torn off and curling behind the dash
                for k in 0..(24.0 * fog) as usize {
                    let t = k as f32 / 24.0;
                    let p = from.lerp(pos, t);
                    let side = Vec3::new(-dir.z, 0.0, dir.x) * if k % 2 == 0 { 1.0 } else { -1.0 };
                    let j = self.perturb(0.4 + k as f32 * 0.1);
                    self.particles.push(Particle {
                        pos: p + Vec3::Y * (0.8 + j.abs() * 2.0) + side * 0.6,
                        vel: side * (2.5 + j.abs() * 3.0) + dir * 1.5 + Vec3::Y * 0.4,
                        life: 1.4,
                        max_life: 1.4,
                        size: 0.35,
                        color: [0.92, 0.94, 1.0],
                        alpha: 0.45 * fog,
                        gravity: -0.2,
                        drag: 1.6,
                    });
                }
            }
        }
        // crumbling: now and then a piece of an island's edge near the
        // player breaks away and falls, shedding snow as it goes
        self.crumble_t -= dt;
        if self.crumble_t > 0.0 || game.gods.is_empty() {
            return;
        }
        let mut s = self.crumble_seed | 1;
        let mut rnd = || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((s >> 33) as f32) / (1u64 << 31) as f32
        };
        self.crumble_t = 0.35 + rnd() * 0.9;
        let focus = game.gods[0].pos;
        let floor = crate::sky::floor();
        let mut edge = None;
        for _ in 0..24 {
            let a = rnd() * std::f32::consts::TAU;
            let r = 8.0 + rnd() * 55.0;
            let p = Vec3::new(focus.x + a.cos() * r, 0.0, focus.z + a.sin() * r);
            let d = floor.sdf(p.x, p.z);
            if (crate::sky::EDGE - 1.5..crate::sky::EDGE).contains(&d) {
                let e = 0.5;
                let out = Vec3::new(floor.sdf(p.x + e, p.z) - floor.sdf(p.x - e, p.z), 0.0, floor.sdf(p.x, p.z + e) - floor.sdf(p.x, p.z - e)).normalize_or_zero();
                edge = Some((p, out));
                break;
            }
        }
        let big = rnd() < 0.2;
        let seed_out = rnd();
        self.crumble_seed = s;
        let Some((p, out)) = edge else { return };
        let n = if big { 9 } else { 3 };
        for k in 0..n {
            let j = Vec3::new(self.perturb(0.8 + k as f32), self.perturb(0.3 + k as f32 * 0.7).abs() * -1.0, self.perturb(0.8 + k as f32 * 1.3));
            let rock = if (k as f32 + seed_out * 3.0) as usize % 3 == 0 { [0.62, 0.80, 0.94] } else { [0.26, 0.30, 0.42] };
            self.dust.push(Particle {
                pos: p + out * 0.6 + j * 0.8 - Vec3::Y * 0.5,
                vel: out * (0.8 + j.x.abs()) + Vec3::new(j.x, 0.2, j.z) * 0.6,
                life: 4.0,
                max_life: 4.0,
                size: if big { 0.28 + 0.12 * (k % 3) as f32 } else { 0.18 + 0.06 * (k % 2) as f32 },
                color: rock,
                alpha: 1.0,
                gravity: 9.0,
                drag: 0.1,
            });
        }
        // a puff of snow off the lip
        for k in 0..(if big { 14 } else { 6 }) {
            let j = self.perturb(0.6 + k as f32 * 0.4);
            self.dust.push(Particle {
                pos: p + out * 0.3 + Vec3::Y * 0.2,
                vel: out * (1.0 + j.abs() * 2.0) + Vec3::new(j, 0.6 + j.abs(), -j) * 0.8,
                life: 1.6,
                max_life: 1.6,
                size: 0.12,
                color: [0.92, 0.94, 1.0],
                alpha: 0.8,
                gravity: 2.5,
                drag: 1.4,
            });
        }
    }

    /// The snowpack (ultra): laid when the map changes, stepped (fall,
    /// drift, marks filling), ploughed by cats wading drifts, and struck by
    /// what abilities do to the ground -- a blast throws a crater, heat
    /// melts a pan, frost lays fresh snow -- each throwing up snow in
    /// proportion to what was there.
    pub fn tick_snow(&mut self, dt: f32, events: &[Event], game: &Game) {
        use crate::snowpack::Hit;
        let map = crate::map::active();
        if self.snow_map != Some(map) {
            self.snow = crate::snowpack::Snowpack::for_map();
            self.snow_map = Some(map);
            self.snow_last.clear();
        }
        let Some(s) = self.snow.as_mut() else { return };
        let atm = game.weather.atmosphere();
        self.snow_wind = atm.wind;
        if let Some(g) = game.gods.first() {
            s.follow(g.pos);
        }
        s.tick(dt, crate::snowpack::snowfall(atm.snow), atm.wind);
        let mut sprays: Vec<(Vec3, f32, f32)> = Vec::new();
        // bodies wading through drifts leave a trench
        self.snow_last.resize(game.gods.len(), Vec3::NAN);
        let viewer = game.gods.iter().find(|g| g.is_player).map(|g| g.team);
        for (i, g) in game.gods.iter().enumerate() {
            // a hidden cat leaves no trench the player can watch being dug
            let free = viewer.is_none_or(|t| game.sees(t, i)) && g.respawn_t <= 0.0 && g.leap.is_none() && g.dash.is_none() && g.riding.is_none() && g.skyfall.is_none();
            let prev = self.snow_last[i];
            if free && prev.is_finite() && prev.distance(g.pos) < 3.0 && prev.distance(g.pos) > 1e-3 {
                if s.is_deep(g.pos.x, g.pos.z) {
                    s.plough(prev, g.pos, 1.3);
                    if (self.time * 5.0 + i as f32 * 0.31).fract() < dt * 5.0 {
                        sprays.push((g.pos + g.move_dir * 0.6, 0.5, 0.25));
                    }
                }
            }
            self.snow_last[i] = if free { g.pos } else { Vec3::NAN };
        }
        // what abilities do to it
        let kind = |c: [f32; 3]| {
            if c[0] > 0.8 && c[2] < 0.5 {
                Hit::Heat
            } else if c[2] > 0.85 && c[0] < 0.75 {
                Hit::Frost
            } else {
                Hit::Blast
            }
        };
        for e in events {
            let (pos, r, k) = match *e {
                Event::Explosion { pos, radius, color } | Event::NovaFx { pos, radius, color } => (pos, radius, kind(color)),
                Event::StrikeFx { at, .. } => (at, 1.6, Hit::Blast),
                Event::StarfallFx { at, radius } => (at, radius, Hit::Heat),
                Event::DashFx { pos, dir, .. } => {
                    s.plough(pos - dir * 2.6, pos, 1.2);
                    continue;
                }
                Event::BeamFx { from, dir, length, width, color, .. } if from.y < 4.0 => {
                    // a beam along the ground scours a furrow
                    let n = (length / 2.0).ceil().clamp(1.0, 20.0) as usize;
                    for q in 0..=n {
                        let p = from + dir * (length * q as f32 / n as f32);
                        s.hit(Vec3::new(p.x, 0.0, p.z), width * 0.5, kind(color));
                    }
                    continue;
                }
                _ => continue,
            };
            if !pos.is_finite() || pos.y > 4.0 {
                continue;
            }
            let there = s.depth_at(pos.x, pos.z);
            s.hit(pos, r, k);
            if there > 0.05 && k != Hit::Frost {
                sprays.push((pos, r, there));
            }
        }
        for (pos, r, amount) in sprays {
            let n = ((amount * r * 12.0) as usize).clamp(3, 70);
            for q in 0..n {
                let a = q as f32 / n as f32 * std::f32::consts::TAU + self.perturb(0.3);
                let out = r * (0.6 + self.perturb(0.4).abs());
                let up = 2.5 + self.perturb(1.5).abs() * 2.0 + amount * 3.0;
                self.dust.push(Particle {
                    pos: pos + Vec3::new(a.cos(), 0.0, a.sin()) * r * 0.4 + Vec3::Y * 0.2,
                    vel: Vec3::new(a.cos() * out, up, a.sin() * out) + atm.wind * 0.3,
                    life: 1.0,
                    max_life: 1.0,
                    size: 0.12 + 0.1 * (q % 3) as f32 * 0.5,
                    color: [0.94, 0.96, 1.0],
                    alpha: 0.95,
                    gravity: 7.0,
                    drag: 1.2,
                });
            }
        }
    }

    /// A star the Moonwyrm let fall lands: a straight column of white light
    /// out of the sky, a flash ring thrown out to the edge of the burn, and
    /// a spray of sparks.
    pub fn starfall(&mut self, at: Vec3, radius: f32) {
        let white = [2.4, 2.6, 3.2];
        let mut out = Vec::new();
        // a column about a cat wide: a dense white core and a softer halo
        let hash = |k: f32| ((k * 12.9898 + at.x * 0.37).sin() * 43758.547).fract().abs() * 2.0 - 1.0;
        for i in 0..300 {
            let h = hash(i as f32 + 0.5).abs() * 44.0;
            let halo = i % 3 == 0;
            let w = if halo { 1.6 } else { 0.7 };
            let j = Vec3::new(hash(i as f32), 0.0, hash(i as f32 + 91.3)) * w;
            let life = 0.35 + 0.3 * (1.0 - h / 44.0);
            let (size, alpha) = if halo { (0.9, 0.45) } else { (0.5, 1.0) };
            out.push(Particle { pos: at + Vec3::Y * h + j, vel: Vec3::Y * 3.0, life, max_life: life, size, color: white, alpha, gravity: 0.0, drag: 0.0 });
        }
        for i in 0..40 {
            let a = i as f32 / 40.0 * std::f32::consts::TAU + self.perturb(0.1);
            let s = 6.0 + self.perturb(2.0).abs() * 3.0;
            out.push(Particle { pos: at + Vec3::Y * 0.3, vel: Vec3::new(a.cos() * s, 4.0 + self.perturb(2.0).abs() * 2.0, a.sin() * s), life: 0.6, max_life: 0.6, size: 0.14, color: if i % 3 == 0 { [2.2, 2.0, 1.4] } else { white }, alpha: 1.0, gravity: 12.0, drag: 0.6 });
        }
        self.particles.extend(out);
        self.rings.push(Ring { pos: at + Vec3::Y * 0.08, r0: 0.5, r1: radius, until: 0.35, dur: 0.35, color: [1.0, 1.0, 1.0], thick: 0.55 });
        self.rings.push(Ring { pos: at + Vec3::Y * 0.1, r0: radius * 0.8, r1: radius * 1.7, until: 0.6, dur: 0.6, color: [0.75, 0.85, 1.0], thick: 0.3 });
    }

    /// Speed made visible: while a cat dashes it leaves a ghost of itself
    /// every 45 ms along its path; a blink -- a jump further in one frame
    /// than a cat can run, short of a respawn -- leaves a longer-lived one
    /// at its origin. In the cat's accent colour, fading.
    pub fn tick_afterimages(&mut self, dt: f32, game: &Game) {
        for a in self.afterimages.iter_mut() {
            a.life -= dt;
        }
        self.afterimages.retain(|a| a.life > 0.0);
        let n = game.gods.len();
        if self.ghost_last.len() != n {
            self.ghost_last = game.gods.iter().map(|g| g.pos).collect();
            self.ghost_t = vec![0.0; n];
        }
        for (i, g) in game.gods.iter().enumerate() {
            let last = self.ghost_last[i];
            self.ghost_last[i] = g.pos;
            if g.respawn_t > 0.0 {
                continue;
            }
            let color = crate::gods::ROSTER.get(g.def_id as usize).map_or([0.8, 0.9, 1.0], |d| d.palette[3]);
            let jump = (g.pos - last).length();
            if jump > 2.5 && jump < 30.0 {
                self.afterimages.push(Afterimage { god: i, pos: last, yaw: g.yaw, life: 0.5, max: 0.5, color });
            }
            if g.dash.is_some() {
                self.ghost_t[i] -= dt;
                if self.ghost_t[i] <= 0.0 {
                    self.ghost_t[i] = 0.045;
                    self.afterimages.push(Afterimage { god: i, pos: g.pos, yaw: g.yaw, life: 0.3, max: 0.3, color });
                }
            } else {
                self.ghost_t[i] = 0.0;
            }
        }
        if self.afterimages.len() > 48 {
            self.afterimages.drain(0..self.afterimages.len() - 48);
        }
    }

    /// A glowing trail streaming behind every magic bolt in flight (they
    /// were bare orbs: a fast one read as a dot).
    pub fn tick_trails(&mut self, game: &Game) {
        // the stars the flying Moonwyrm lets fall: a bright tail back up
        // toward it
        for sf in &game.starfalls {
            let p = sf.pos();
            let back = (sf.from - sf.at).normalize_or_zero();
            for k in 0..4 {
                let j = Vec3::new(self.perturb(0.4 + k as f32), self.perturb(0.4 + k as f32 * 2.3), self.perturb(0.4 + k as f32 * 5.1));
                self.particles.push(Particle {
                    pos: p + back * (0.8 * k as f32) + j,
                    vel: back * 1.5,
                    life: 0.5,
                    max_life: 0.5,
                    size: 0.2,
                    color: if k == 0 { [2.0, 1.9, 1.5] } else { [1.2, 1.4, 2.0] },
                    alpha: 0.95,
                    gravity: 0.0,
                    drag: 1.0,
                });
            }
        }
        // the Moonwyrm's thrown stars: a long glittering tail, like a
        // shooting star's
        for st in &game.star_bolts {
            let back = -st.vel.normalize_or_zero();
            for k in 0..3 {
                let j = Vec3::new(self.perturb(0.25 + k as f32), self.perturb(0.25 + k as f32 * 2.3), self.perturb(0.25 + k as f32 * 5.1));
                self.particles.push(Particle {
                    pos: st.pos + back * (0.4 * k as f32) + j,
                    vel: back * 2.0 + Vec3::Y * 0.3,
                    life: 0.55,
                    max_life: 0.55,
                    size: 0.13,
                    color: if k == 0 { [1.8, 1.7, 1.3] } else { [1.1, 1.35, 1.9] },
                    alpha: 0.9,
                    gravity: -0.4,
                    drag: 1.2,
                });
            }
        }
        for p in &game.projectiles {
            match p.kind {
                crate::sim::ProjKind::Arrow => {
                    // a short thin streak
                    let back = -p.vel.normalize_or_zero();
                    self.particles.push(Particle { pos: p.pos + back * 0.2, vel: Vec3::ZERO, life: 0.12, max_life: 0.12, size: 0.06, color: [0.95, 0.95, 0.9], alpha: 0.6, gravity: 0.0, drag: 0.0 });
                    continue;
                }
                crate::sim::ProjKind::Lob { .. } => {
                    // a boulder smokes through its arc (lit dust, not light)
                    let j = Vec3::new(self.perturb(0.2), self.perturb(0.2), self.perturb(0.2));
                    self.dust.push(Particle { pos: p.pos + j, vel: Vec3::Y * 0.4, life: 0.8, max_life: 0.8, size: 0.28, color: [0.42, 0.38, 0.33], alpha: 0.55, gravity: -0.2, drag: 1.5 });
                    continue;
                }
                crate::sim::ProjKind::Bolt => {}
            }
            let back = -p.vel.normalize_or_zero();
            for k in 0..2 {
                let j = Vec3::new(self.perturb(0.08 + k as f32), self.perturb(0.08 + k as f32 * 3.1), self.perturb(0.08 + k as f32 * 7.3));
                self.particles.push(Particle {
                    pos: p.pos + back * (0.15 * k as f32) + j,
                    vel: back * 1.5,
                    life: 0.24,
                    max_life: 0.24,
                    size: 0.15,
                    color: [p.color[0] * 1.6, p.color[1] * 1.6, p.color[2] * 1.6],
                    alpha: 0.8,
                    gravity: 0.0,
                    drag: 2.0,
                });
            }
        }
    }

    pub fn update(&mut self, dt: f32, events: &[Event], game: &Game) {
        self.glow_events(dt, events, game);
        self.tick_afterimages(dt, game);
        self.tick_trails(game);
        self.tick_channels(dt, events, game);
        self.tick_wisps(dt, game);
        self.tick_snow(dt, events, game);
        self.tick_sky_life(dt, events, game);
        self.time += dt;
        let atm = game.weather.atmosphere();
        self.ground.tick(dt, atm.rain, atm.wetness);
        if !self.seeded {
            self.seed_ambient();
        }

        // ambient leaves: slow fall + sway + wind, respawn under the canopy
        let wind = game.weather.atmosphere().wind;
        for (i, lf) in self.leaves.iter_mut().enumerate() {
            lf.life -= dt;
            lf.pos.y -= 0.72 * dt;
            lf.pos.x += ((self.time * 1.4 + i as f32 * 0.9).sin() * 0.6 + wind.x * 0.35) * dt;
            lf.pos.z += ((self.time * 1.1 + i as f32 * 1.7).cos() * 0.5 + wind.z * 0.35) * dt;
            if lf.life <= 0.0 || lf.pos.y < 0.12 {
                if !self.leaf_spots.is_empty() {
                    let spot = self.leaf_spots[i % self.leaf_spots.len()];
                    lf.pos = spot + Vec3::new(0.0, 4.5 + (i % 5) as f32 * 1.3, 0.0);
                }
                lf.life = 8.0 + (i % 7) as f32;
            }
        }

        // gust fronts tear loose leaves out of the canopy around the player
        {
            let atm = game.weather.atmosphere();
            if atm.gust > 0.55 && self.particles.len() < MAX_PARTICLES - 256 {
                let n = ((atm.gust - 0.55) * 40.0 * dt * 60.0) as usize;
                let centre = game.gods[0].pos;
                for i in 0..n {
                    let s = self.time * 7.0 + i as f32;
                    let p = centre + Vec3::new(rnd_jitter(s) * 18.0, 3.0 + rnd_jitter(s * 1.7).abs() * 5.0, rnd_jitter(s * 2.3) * 18.0);
                    if crate::consts::lane_clearance(p.x, p.z) < 4.0 {
                        continue;
                    }
                    let tint = match i % 3 {
                        0 => [0.24, 0.36, 0.12],
                        1 => [0.40, 0.30, 0.10],
                        _ => [0.18, 0.32, 0.12],
                    };
                    self.particles.push(Particle {
                        pos: p,
                        vel: wind * 0.9 + Vec3::new(rnd_jitter(s + 9.0), 0.4 + rnd_jitter(s + 4.0) * 0.5, rnd_jitter(s + 5.0)) * 1.5,
                        life: 2.5 + rnd_jitter(s).abs() * 2.0,
                        max_life: 4.5,
                        size: 0.18,
                        color: tint,
                        alpha: 0.9,
                        gravity: 0.9,
                        drag: 0.6,
                    });
                }
            }
        }

        // ground birds: flush when anything comes within 6 u, land elsewhere
        for (k, f) in self.flocks.iter_mut().enumerate() {
            f.phase += dt;
            let threatened = game.gods.iter().any(|g| g.respawn_t <= 0.0 && g.pos.distance(f.anchor) < 6.0)
                || game.wild.prey.iter().any(|p| p.fear > 0.0 && p.pos.distance(f.anchor) < 4.0);
            if threatened && f.flight <= 0.0 {
                f.flight = 1.0;
                // pick a landing spot a little way off, clear of lanes and water
                let mut best = f.anchor + Vec3::new(9.0, 0.0, 6.0);
                for j in 0..8 {
                    let a = (k * 7 + j) as f32 * 0.9 + self.time;
                    let cand = f.anchor + Vec3::new(a.cos(), 0.0, a.sin()) * 14.0;
                    if crate::consts::lane_clearance(cand.x, cand.z) > 6.0 && crate::consts::river_dist(cand.x, cand.z) > 6.0 && cand.length() < 140.0 {
                        best = cand;
                        break;
                    }
                }
                f.next_anchor = best;
            }
            if f.flight > 0.0 {
                f.flight -= dt * 0.22;
                if f.flight <= 0.0 {
                    f.flight = 0.0;
                    f.anchor = f.next_anchor;
                }
            }
        }

        // paw-falls: dust on dry ground, splashes on wet, puffs of snow
        {
            let atm = game.weather.atmosphere();
            let (col, size, up) = if atm.snow > 0.4 {
                ([0.92, 0.95, 1.0], 0.16, 1.4)
            } else if game.weather.wetness > 0.45 {
                ([0.65, 0.75, 0.9], 0.12, 2.2)
            } else {
                ([0.55, 0.47, 0.34], 0.14, 0.9)
            };
            let viewer = game.gods.iter().find(|g| g.is_player).map(|g| g.team);
            for (i, g) in game.gods.iter().enumerate() {
                // (no dust from a cat the cloud hides: it would give it away)
                if g.respawn_t > 0.0 || g.move_dir.length_squared() < 0.01 || viewer.is_some_and(|t| !game.sees(t, i)) {
                    continue;
                }
                // ~6 puffs/s per moving ancestor, staggered by index
                if ((self.time * 6.0 + i as f32 * 0.37).fract() < dt * 6.0) && self.particles.len() < MAX_PARTICLES - 64 {
                    let side = if ((self.time * 6.0 + i as f32 * 0.37) as i32) % 2 == 0 { 0.35 } else { -0.35 };
                    let right = Vec3::new(g.yaw.sin(), 0.0, -g.yaw.cos());
                    self.particles.push(Particle {
                        pos: g.pos + right * side + Vec3::Y * 0.12,
                        vel: Vec3::new(rnd_jitter(self.time + i as f32) * 0.6, up, rnd_jitter(self.time * 1.3 + i as f32) * 0.6) - g.move_dir.normalize_or_zero() * 0.8,
                        life: 0.55,
                        max_life: 0.55,
                        size,
                        color: col,
                        alpha: 0.7,
                        gravity: 1.5,
                        drag: 2.0,
                    });
                }
            }
        }

        for e in events {
            match e {
                Event::Damage { pos, amount, big } => {
                    let n = if *big { 10 } else { 4 };
                    for _ in 0..n {
                        self.spawn_spark(*pos, [1.0, 0.85, 0.45], 2.6);
                    }
                    if *big {
                        for _ in 0..6 {
                            self.spawn_spark(*pos, [1.0, 0.4, 0.2], 3.4);
                        }
                    }
                    let _ = amount;
                }
                Event::CastStart { god, dur, .. } => self.puppets.on_windup(*god, *dur),
                Event::Whiff { god } => self.puppets.on_whiff(*god),
                Event::Cast { god, slot, aim } => {
                    if let Some(a) = game.def(*god).abilities.get(*slot as usize) {
                        self.puppets.on_cast(*god, crate::puppet::cast_style(&a.arch, a.ult));
                        // The identity read: this ancestor's element, thrown
                        // the way this archetype throws it. The archetype
                        // events below stay as the gameplay read.
                        use crate::signature::{emit, Gesture, Motif, Rng};
                        let g = &game.gods[*god];
                        let seed = (self.time * 1000.0) as u32 ^ ((*god as u32) << 16) ^ *slot;
                        let mut rng = Rng(seed | 1);
                        self.particles.extend(emit(Motif::of(g.def_id), Gesture::of(&a.arch), g.pos, *aim, a.ult, &mut rng));
                    }
                }
                Event::Explosion { pos, radius, color } => {
                    // every impact kicks up earth (lit dust)
                    for k in 0..12 {
                        let a = k as f32 / 12.0 * std::f32::consts::TAU + self.perturb(0.2);
                        let d = Vec3::new(a.cos(), 0.0, a.sin());
                        let up = 1.4 + self.perturb(0.6);
                        self.dust.push(Particle { pos: *pos + d * 0.5 + Vec3::Y * 0.2, vel: d * (*radius * 0.9) + Vec3::Y * up, life: 1.0, max_life: 1.0, size: 0.45, color: [0.38 + color[0] * 0.05, 0.31 + color[1] * 0.05, 0.23 + color[2] * 0.05], alpha: 0.65, gravity: 0.5, drag: 2.2 });
                    }
                    self.quake(*pos, *radius, (*radius / 6.0).clamp(0.2, 1.0));
                    let n = (*radius * 10.0).min(90.0) as usize;
                    for i in 0..n {
                        let a = i as f32 / n as f32 * std::f32::consts::TAU;
                        let r = self.perturb(*radius * 0.5);
                        let p = Vec3::new(pos.x + a.cos() * r * 0.4, 0.4 + self.perturb(0.8), pos.z + a.sin() * r * 0.4);
                        let vz = 3.0 + self.perturb(2.0);
                        self.particles.push(Particle {
                            pos: p,
                            vel: Vec3::new(a.cos() * radius * 2.2, vz, a.sin() * radius * 2.2),
                            life: 0.55,
                            max_life: 0.55,
                            size: 0.28,
                            color: *color,
                            alpha: 0.95,
                            gravity: 9.0,
                            drag: 2.4,
                        });
                    }
                }
                Event::NovaFx { pos, radius, color } => {
                    // a ground-hugging dust wave thrown out to the ring: what
                    // makes an impact feel heavy
                    // earth, barely tinted: particles draw with a glow, and the
                    // first mid-tone dust came out as bright cream puffs
                    // Real impact dust LAGS the shockwave: slower than the
                    // ring, billowing up, hanging -- and darker than the path
                    // so it reads. (Racing out with the ring, pale, it was a
                    // 2.7k-pixel rim hidden inside the ring's cubes.)
                    let dust = [0.36 + color[0] * 0.05, 0.28 + color[1] * 0.05, 0.2 + color[2] * 0.05];
                    for k in 0..24 {
                        let a = k as f32 / 24.0 * std::f32::consts::TAU + self.perturb(0.12);
                        let d = Vec3::new(a.cos(), 0.0, a.sin());
                        let up = 1.1 + self.perturb(0.5);
                        let reach = 0.45 + self.perturb(0.1).abs();
                        self.dust.push(Particle {
                            pos: *pos + d * 0.6 + Vec3::Y * 0.3,
                            vel: d * (*radius * reach * 1.6) + Vec3::Y * up,
                            life: 1.3,
                            max_life: 1.3,
                            size: 0.62,
                            color: dust,
                            alpha: 0.72,
                            gravity: 0.35,
                            drag: 2.4,
                        });
                    }
                    self.quake(*pos, *radius * 0.6, (*radius / 9.0).clamp(0.2, 0.9));
                    self.rings.push(Ring { pos: *pos, r0: 0.5, r1: *radius, until: 0.55, dur: 0.55, color: *color, thick: 0.45 });
                    for _ in 0..(*radius * 4.0).min(60.0) as usize {
                        self.spawn_spark(*pos + Vec3::Y * 0.8, *color, 3.0);
                    }
                }
                Event::BoltFx { from, to, color, thick } => {
                    // jagged lightning: cubes along a randomized path
                    let segs = 22;
                    let mut prev = *from;
                    for i in 1..=segs {
                        let t = i as f32 / segs as f32;
                        let jitter = if i < segs { 0.9 } else { 0.0 };
                        let p = Vec3::lerp(*from, *to, t)
                            + Vec3::new(rnd_jitter(i as f32) * jitter, 0.0, rnd_jitter(i as f32 + 9.0) * jitter);
                        self.line_particles(prev, p, *color, *thick * 1.2, 0.16);
                        prev = p;
                    }
                    self.rings.push(Ring { pos: *to, r0: 0.2, r1: 2.2, until: 0.3, dur: 0.3, color: *color, thick: 0.3 });
                }
                Event::BeamFx { from, dir, length, width, color, dur } => {
                    self.beams.push(Beam { from: *from, dir: *dir, length: *length, width: *width, color: *color, until: self.time + dur.max(0.15) });
                }
                Event::ShieldFx { pos, dur, color } => {
                    // instant ring pulse; persistent shell comes from game state
                    self.rings.push(Ring { pos: *pos, r0: 0.8, r1: 1.8, until: 0.4, dur: 0.4, color: *color, thick: 0.3 });
                    let _ = dur;
                }
                Event::DashFx { pos, dir, color } => {
                    for i in 0..14 {
                        let t = i as f32 / 14.0;
                        let wob = self.perturb(0.6);
                        self.particles.push(Particle {
                            pos: *pos - *dir * t * 2.6 + Vec3::Y * wob,
                            vel: -*dir * 2.0,
                            life: 0.4 + t * 0.2,
                            max_life: 0.6,
                            size: 0.34 - t * 0.12,
                            color: *color,
                            alpha: 0.85,
                            gravity: 0.0,
                            drag: 3.0,
                        });
                    }
                }
                Event::RingTelegraph { pos, radius, delay, color } => {
                    // pulsing pre-telegraph ring
                    self.rings.push(Ring { pos: *pos, r0: *radius, r1: *radius, until: *delay, dur: delay.max(0.1), color: *color, thick: 0.35 });
                }
                Event::PullFx { pos, radius, color } => {
                    let n = 70;
                    for i in 0..n {
                        let a = i as f32 / n as f32 * std::f32::consts::TAU * 3.0;
                        let r = radius;
                        let p = Vec3::new(pos.x + a.cos() * r, 0.4 + (i % 5) as f32 * 0.35, pos.z + a.sin() * r);
                        let to_c = (*pos - p).normalize();
                        self.particles.push(Particle {
                            pos: p,
                            vel: to_c * 14.0 + Vec3::new(-to_c.z, 0.0, to_c.x) * 6.0,
                            life: 0.6,
                            max_life: 0.6,
                            size: 0.3,
                            color: *color,
                            alpha: 0.95,
                            gravity: 0.0,
                            drag: 0.5,
                        });
                    }
                }
                Event::HealFx { pos, radius } => {
                    // a green glyph spreading on the ground (a countdown ring)
                    self.rings.push(Ring { pos: *pos + Vec3::Y * 0.05, r0: 0.4, r1: *radius, until: 0.7, dur: 0.7, color: [0.45, 1.0, 0.6], thick: 0.16 });
                    // motes climbing a helix, three strands, round the centre
                    for k in 0..36 {
                        let strand = (k % 3) as f32 * std::f32::consts::TAU / 3.0;
                        let h = (k / 3) as f32 / 12.0;
                        let a = strand + h * std::f32::consts::TAU * 1.5;
                        let r = 0.9;
                        let p = *pos + Vec3::new(a.cos() * r, 0.2 + h * 0.4, a.sin() * r);
                        let tangent = Vec3::new(-a.sin(), 0.0, a.cos());
                        self.particles.push(Particle {
                            pos: p,
                            vel: tangent * 2.2 + Vec3::Y * 2.6,
                            life: 1.0,
                            max_life: 1.0,
                            size: 0.18,
                            color: [0.6, 1.3, 0.7],
                            alpha: 0.9,
                            gravity: -0.5,
                            drag: 0.8,
                        });
                    }
                    for _ in 0..24 {
                        let a = self.perturb(std::f32::consts::TAU);
                        let r = self.perturb(*radius);
                        self.particles.push(Particle {
                            pos: Vec3::new(pos.x + a.cos() * r, 0.2, pos.z + a.sin() * r),
                            vel: Vec3::Y * 3.2,
                            life: 1.1,
                            max_life: 1.1,
                            size: 0.26,
                            color: [0.45, 1.0, 0.55],
                            alpha: 0.95,
                            gravity: -1.0,
                            drag: 0.6,
                        });
                    }
                }
                Event::StormFx { pos, radius, color, dur } => {
                    // persistent swirl is driven per-frame from game zones; initial burst:
                    let n = (*radius * 8.0).min(80.0) as usize;
                    for i in 0..n {
                        let a = i as f32 / n as f32 * std::f32::consts::TAU;
                        self.particles.push(Particle {
                            pos: Vec3::new(pos.x + a.cos() * radius * 0.8, 0.3, pos.z + a.sin() * radius * 0.8),
                            vel: Vec3::new(-a.sin(), 2.2, a.cos()) * (*radius) * 0.35,
                            life: (*dur).min(1.4),
                            max_life: 1.4,
                            size: 0.3,
                            color: *color,
                            alpha: 0.9,
                            gravity: -0.5,
                            drag: 0.9,
                        });
                    }
                }
                Event::StrikeFx { at, color } => self.lightning(*at, *color),
                Event::StarfallFx { at, radius } => self.starfall(*at, *radius),
                Event::LevelUp { pos } => {
                    for i in 0..26 {
                        let a = i as f32 * 0.24;
                        self.particles.push(Particle {
                            pos: Vec3::new(pos.x + a.cos() * 0.7, 0.2 + (i % 4) as f32 * 0.3, pos.z + a.sin() * 0.7),
                            vel: Vec3::Y * 4.5,
                            life: 0.9,
                            max_life: 0.9,
                            size: 0.3,
                            color: [1.0, 0.85, 0.35],
                            alpha: 1.0,
                            gravity: -2.0,
                            drag: 1.0,
                        });
                    }
                }
                Event::TowerDown { pos, .. } => {
                    for i in 0..90 {
                        let a = i as f32 * 0.07;
                        let hy = 1.0 + self.perturb(4.0);
                        let vx = 2.0 + self.perturb(4.0);
                        let vy = 3.0 + self.perturb(5.0);
                        self.particles.push(Particle {
                            pos: Vec3::new(pos.x, hy, pos.z),
                            vel: Vec3::new(a.cos() * vx, vy, a.sin() * vx),
                            life: 1.4,
                            max_life: 1.4,
                            size: 0.55,
                            color: if i % 3 == 0 { [1.0, 0.5, 0.2] } else { [0.5, 0.48, 0.45] },
                            alpha: 0.95,
                            gravity: 5.0,
                            drag: 1.4,
                        });
                    }
                }
                Event::Kill { victim_team, is_god, .. } => {
                    let c = victim_team.rgb();
                    let n = if *is_god { 40 } else { 12 };
                    for _ in 0..n {
                        self.spawn_spark(game.gods.first().map(|g| g.pos).unwrap_or_default() + Vec3::Y * 1.0, c, 3.0);
                    }
                }
                Event::TitanVulnerable { team } => {
                    let c = team.rgb();
                    for i in 0..60 {
                        let a = i as f32 * 0.105;
                        self.particles.push(Particle {
                            pos: Vec3::new(a.cos() * 4.0, 0.2, team.titan_pos().z + a.sin() * 4.0),
                            vel: Vec3::Y * 5.0,
                            life: 1.2,
                            max_life: 1.2,
                            size: 0.4,
                            color: c,
                            alpha: 1.0,
                            gravity: -1.0,
                            drag: 0.8,
                        });
                    }
                }
                Event::PreyCaught { pos, kind, gold: _ } => {
                    // a puff of fur/feathers and a glint of gold
                    let n = if *kind == crate::wild::PreyKind::Stag { 26 } else { 14 };
                    for _ in 0..n {
                        self.spawn_spark(*pos + Vec3::Y * 0.5, [0.75, 0.62, 0.45], 2.4);
                    }
                    for _ in 0..6 {
                        self.spawn_spark(*pos + Vec3::Y * 0.9, [1.0, 0.85, 0.35], 1.6);
                    }
                }
                Event::BossSpawn { kind, pos } => {
                    let c = kind.color();
                    for i in 0..80 {
                        let a = i as f32 * 0.0785;
                        self.particles.push(Particle {
                            pos: *pos + Vec3::new(a.cos() * 2.0, 0.5, a.sin() * 2.0),
                            vel: Vec3::new(a.cos() * 6.0, 6.0 + (i % 5) as f32, a.sin() * 6.0),
                            life: 1.8,
                            max_life: 1.8,
                            size: 0.32,
                            color: c,
                            alpha: 1.0,
                            gravity: 6.0,
                            drag: 0.8,
                        });
                    }
                    self.rings.push(Ring { pos: *pos, r0: 1.0, r1: crate::bosses::PIT_RADIUS, until: 1.6, dur: 1.6, color: c, thick: 0.4 });
                }
                Event::BossDown { kind, pos, .. } => {
                    let c = kind.color();
                    for _ in 0..120 {
                        self.spawn_spark(*pos + Vec3::Y * 2.0, c, 6.0);
                    }
                    self.rings.push(Ring { pos: *pos, r0: 1.0, r1: 22.0, until: 2.2, dur: 2.2, color: c, thick: 0.5 });
                }
                Event::FireIgnite { pos } => {
                    for _ in 0..12 {
                        self.spawn_spark(*pos + Vec3::Y * 0.5, [1.0, 0.6, 0.15], 3.0);
                    }
                }
                Event::FissureOpen { a, b } => {
                    // An earthquake's fissure breaks the ground along its length.
                    for k in 0..3 {
                        let at = a.lerp(*b, (k as f32 + 0.5) / 3.0);
                        self.ground.strike(at, 5.0, 1.0);
                    }
                    // dust and shards thrown up along the tear
                    let n = ((*b - *a).length() * 2.0) as usize;
                    for i in 0..n {
                        let t = i as f32 / n.max(1) as f32;
                        let p = *a + (*b - *a) * t;
                        self.particles.push(Particle {
                            pos: p + Vec3::Y * 0.2,
                            vel: Vec3::new(rnd_jitter(t * 9.0) * 2.0, 5.0 + rnd_jitter(t * 5.0).abs() * 4.0, rnd_jitter(t * 7.0) * 2.0),
                            life: 1.4,
                            max_life: 1.4,
                            size: 0.24,
                            color: [0.45, 0.4, 0.34],
                            alpha: 0.9,
                            gravity: 8.0,
                            drag: 0.5,
                        });
                    }
                }
                Event::TornadoDebris { pos } => {
                    for i in 0..14 {
                        let a = i as f32 * 0.45 + self.time;
                        self.particles.push(Particle {
                            pos: *pos + Vec3::new(a.cos() * 4.0, 2.0 + i as f32 * 0.6, a.sin() * 4.0),
                            vel: Vec3::new(a.cos() * 9.0, 4.0, a.sin() * 9.0),
                            life: 2.5,
                            max_life: 2.5,
                            size: 0.3,
                            color: if i % 3 == 0 { [0.3, 0.2, 0.1] } else { [0.24, 0.36, 0.12] },
                            alpha: 1.0,
                            gravity: 5.0,
                            drag: 0.6,
                        });
                    }
                }
                Event::MapEventStart { kind, pos, radius } => {
                    use crate::map_events::MapEventKind as K;
                    let c = kind.color();
                    match kind {
                        K::Sweetrain => {
                            self.rings.push(Ring { pos: *pos, r0: 0.5, r1: *radius, until: 2.0, dur: 2.0, color: c, thick: 0.45 });
                        }
                        K::LongNight | K::LongWinter | K::EldersBounty | K::ThunderFront | K::PreyRun => {
                            // a sky-wide shimmer: a burst rising from both fountains
                            for team in [crate::consts::Team::Blue, crate::consts::Team::Red] {
                                let f = team.fountain();
                                for i in 0..40 {
                                    let a = i as f32 * 0.157;
                                    self.particles.push(Particle {
                                        pos: f + Vec3::new(a.cos() * 3.0, 0.4, a.sin() * 3.0),
                                        vel: Vec3::Y * 7.0,
                                        life: 2.2,
                                        max_life: 2.2,
                                        size: 0.3,
                                        color: c,
                                        alpha: 0.9,
                                        gravity: -0.5,
                                        drag: 0.4,
                                    });
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // physics update
        for p in self.particles.iter_mut().chain(self.dust.iter_mut()) {
            p.life -= dt;
            p.vel.y -= p.gravity * dt;
            p.vel *= 1.0 / (1.0 + p.drag * dt);
            p.pos += p.vel * dt;
            if p.pos.y < 0.05 {
                p.pos.y = 0.05;
                p.vel.y *= -0.25;
            }
        }
        self.particles.retain(|p| p.life > 0.0);
        self.dust.retain(|p| p.life > 0.0);
        if self.particles.len() > MAX_PARTICLES {
            self.particles.drain(0..self.particles.len() - MAX_PARTICLES);
        }
        self.beams.retain(|b| b.until > self.time);
        self.rings.retain(|r| r.until > 0.0);
        for r in self.rings.iter_mut() {
            r.until -= dt;
        }
    }

    /// Something hit the ground: a blast front through the trees, and if it
    /// was hard enough, broken earth and the dust that comes off it.
    pub fn quake(&mut self, pos: Vec3, radius: f32, power: f32) {
        let broke = self.ground.strike(pos, radius, power);
        // Dust rides low and hangs; a few stones go up and come straight
        // down. Sandy on a lane, dark soil in the jungle.
        let on_lane = crate::consts::lane_clearance(pos.x, pos.z) < 4.5;
        let soil = if on_lane { [0.62, 0.55, 0.42] } else { [0.30, 0.24, 0.17] };
        let n = ((8.0 + radius * 5.0) * power) as usize;
        for i in 0..n.min(48) {
            let a = i as f32 / n.max(1) as f32 * std::f32::consts::TAU + self.perturb(0.4);
            let r = radius * (0.35 + self.perturb(0.3).abs());
            let out = 2.0 + radius * 0.9 * power + self.perturb(1.0).abs();
            let shade = 0.85 + self.perturb(0.15);
            let up = 0.5 + self.perturb(0.5).abs();
            self.particles.push(Particle {
                pos: Vec3::new(pos.x + a.cos() * r, 0.15, pos.z + a.sin() * r),
                vel: Vec3::new(a.cos() * out, up, a.sin() * out),
                life: 1.4, max_life: 1.4, size: 0.34,
                color: [soil[0] * shade, soil[1] * shade, soil[2] * shade], alpha: 0.5, gravity: 0.4, drag: 2.6,
            });
        }
        if broke {
            for i in 0..14 {
                let a = i as f32 * 0.449 * std::f32::consts::TAU;
                let v = 1.5 + self.perturb(1.5).abs();
                let up = 5.0 + self.perturb(3.0).abs() * power;
                self.particles.push(Particle {
                    pos: Vec3::new(pos.x + a.cos() * radius * 0.4, 0.1, pos.z + a.sin() * radius * 0.4),
                    vel: Vec3::new(a.cos() * v, up, a.sin() * v),
                    life: 0.9, max_life: 0.9, size: 0.15,
                    color: [soil[0] * 0.6, soil[1] * 0.6, soil[2] * 0.6], alpha: 1.0, gravity: 20.0, drag: 0.3,
                });
            }
        }
    }

    /// What the bodies did to the world this frame: leaves knocked loose,
    /// water shaken off, dust from a slammed paw.
    pub fn puppet_events(&mut self, events: &[crate::puppet::PuppetEvent]) {
        use crate::puppet::PuppetEvent as E;
        for e in events {
            match *e {
                E::Rustle { pos, strength } => {
                    let n = (4.0 + strength * 8.0) as usize;
                    for i in 0..n {
                        let a = i as f32 / n as f32 * std::f32::consts::TAU + self.perturb(0.6);
                        let v = 1.2 + self.perturb(1.0).abs() * strength * 2.0;
                        let g = 0.30 + self.perturb(0.12);
                        let up = 0.5 + self.perturb(0.3).abs();
                        self.particles.push(Particle {
                            pos: pos + Vec3::new(a.cos() * 0.5, up, a.sin() * 0.5),
                            vel: Vec3::new(a.cos() * v, 1.6 + strength * 1.4, a.sin() * v),
                            life: 0.9, max_life: 0.9, size: 0.13,
                            color: [0.12, g, 0.10], alpha: 0.95, gravity: 5.0, drag: 2.2,
                        });
                    }
                }
                E::Droplets { pos } => {
                    for i in 0..26 {
                        let a = i as f32 / 26.0 * std::f32::consts::TAU;
                        let v = 3.0 + self.perturb(1.5).abs();
                        let vy = 1.5 + self.perturb(1.2).abs();
                        self.particles.push(Particle {
                            pos: pos + Vec3::new(0.0, 0.9, 0.0),
                            vel: Vec3::new(a.cos() * v, vy, a.sin() * v),
                            life: 0.5, max_life: 0.5, size: 0.07,
                            color: [0.75, 0.88, 1.0], alpha: 0.8, gravity: 11.0, drag: 1.0,
                        });
                    }
                }
                E::PawDown { pos, speed } => {
                    let wind = self.snow_wind;
                    let Some(s) = self.snow.as_mut() else { continue };
                    let deep = s.is_deep(pos.x, pos.z);
                    let print = s.paw(pos);
                    if print <= 0.0 || speed < 1.0 {
                        continue;
                    }
                    let n = if deep { 4 } else { 3 };
                    for k in 0..n {
                        let j = Vec3::new(self.perturb(0.3 + k as f32), 0.0, self.perturb(0.7 + k as f32));
                        // a drift throws clods up and they fall back; powder
                        // is kicked low and the wind takes it
                        let (vel, grav, life) = if deep {
                            (Vec3::new(j.x * 2.0, 2.2 + j.z.abs() * 2.0, j.z * 2.0), 9.0, 0.5)
                        } else {
                            (wind * 0.8 + Vec3::new(j.x, 0.4, j.z), 0.4, 1.1)
                        };
                        self.dust.push(Particle { pos: pos + Vec3::Y * 0.15, vel, life, max_life: life, size: if deep { 0.13 } else { 0.08 }, color: [0.93, 0.95, 1.0], alpha: 0.9, gravity: grav, drag: if deep { 1.0 } else { 1.8 } });
                    }
                }
                E::PawSlam { pos } => {
                    self.ground.strike(pos, 1.6, 0.5);
                    for i in 0..14 {
                        let a = i as f32 / 14.0 * std::f32::consts::TAU;
                        self.particles.push(Particle {
                            pos: pos + Vec3::new(0.0, 0.1, 0.0),
                            vel: Vec3::new(a.cos() * 3.2, 0.7, a.sin() * 3.2),
                            life: 0.4, max_life: 0.4, size: 0.16,
                            color: [0.55, 0.48, 0.36], alpha: 0.7, gravity: 1.5, drag: 4.0,
                        });
                    }
                }
            }
        }
    }

    fn perturb(&mut self, s: f32) -> f32 {
        // cheap hash noise from time
        let x = (self.time * 977.0 + s * 131.0).sin() * 43758.5453;
        (x - x.floor()).abs() * s * 2.0 - s
    }

    fn spawn_spark(&mut self, pos: Vec3, color: [f32; 3], speed: f32) {
        let x = (self.time * 733.0 + pos.x * 91.7 + pos.z * 57.1).sin() * 43758.5;
        let r1 = (x - x.floor()).abs();
        let y = (self.time * 391.0 + pos.y * 13.3 + pos.x * 7.7).sin() * 24634.6;
        let r2 = (y - y.floor()).abs();
        self.particles.push(Particle {
            pos: pos + Vec3::new(0.0, 0.3, 0.0),
            vel: Vec3::new((r1 - 0.5) * speed * 2.0, r2 * speed, ((r1 + r2) - 1.0) * speed * 2.0),
            life: 0.4 + r2 * 0.3,
            max_life: 0.7,
            size: 0.22,
            color,
            alpha: 1.0,
            gravity: 8.0,
            drag: 1.6,
        });
    }

    fn line_particles(&mut self, a: Vec3, b: Vec3, color: [f32; 3], width: f32, life: f32) {
        let n = ((b - a).length() / 0.55).ceil().max(1.0) as usize;
        for i in 0..=n {
            let t = i as f32 / n as f32;
            let p = Vec3::lerp(a, b, t);
            let wob = self.perturb(1.2);
            self.particles.push(Particle {
                pos: p,
                vel: Vec3::new(0.0, wob, 0.0),
                life,
                max_life: life,
                size: width * (1.0 - t * 0.4),
                color,
                alpha: 1.0,
                gravity: 0.0,
                drag: 0.0,
            });
        }
    }

    /// Build GPU instance list: particles + beams + persistent shields/zone discs.
    pub fn build_gpu(&mut self, game: &Game, cam_eye: Vec3) -> Vec<VoxInst> {
        let mut out = Vec::with_capacity(self.particles.len() + 256);

        // --- weather: precipitation around the camera ---------------------
        let atm = game.weather.atmosphere();
        self.precipitation(&mut out, cam_eye, &atm);

        // --- the rivers: gelatin blobs, wading splashes, fish, dragonflies --
        self.river_life(&mut out, cam_eye, game, &atm);

        // --- catastrophes ----------------------------------------------------
        self.disaster_visuals(&mut out, cam_eye, game);

        // --- ground birds ----------------------------------------------------
        for f in &self.flocks {
            let fl = f.flight;
            // arc: up fast, wheel, glide down to the next anchor
            let t = 1.0 - fl; // 0 at flush, 1 at landing
            let ease = t * t * (3.0 - 2.0 * t);
            let centre = f.anchor.lerp(f.next_anchor, ease);
            let lift = if fl > 0.0 { (t * std::f32::consts::PI).sin() * 7.0 } else { 0.0 };
            for i in 0..f.n {
                let s = i as f32;
                let (px, pz, py, flap) = if fl > 0.0 {
                    let a = self.time * 2.4 + s * 1.3;
                    (a.cos() * (1.6 + s * 0.2), a.sin() * (1.6 + s * 0.2), lift + (a * 2.0).sin() * 0.5 + s * 0.15, (self.time * 18.0 + s).sin())
                } else {
                    // pecking: tiny hops
                    let hop = ((self.time * 3.0 + f.phase + s * 0.7).sin() * 0.5 + 0.5).powf(6.0) * 0.25;
                    ((s * 1.7).cos() * 1.2, (s * 2.3).sin() * 1.2, 0.1 + hop, 0.0)
                };
                let p = Vec3::new(centre.x + px, crate::render::terrain_height(centre.x + px, centre.z + pz).max(0.0) + py, centre.z + pz);
                let dark = [0.12, 0.1, 0.09, 1.0];
                out.push(VoxInst { off_ao: [p.x, p.y, p.z, 1.0], color: dark, misc: [0.16, 0.0, 0.0, 0.0] });
                // wings: two cubes that beat when flying
                let w = 0.14 + flap.abs() * 0.1;
                out.push(VoxInst { off_ao: [p.x - 0.16, p.y + flap * 0.08, p.z, 1.0], color: dark, misc: [w, 0.0, 0.0, 0.0] });
                out.push(VoxInst { off_ao: [p.x + 0.16, p.y + flap * 0.08, p.z, 1.0], color: dark, misc: [w, 0.0, 0.0, 0.0] });
                // a red-breasted one per flock
                if i == 0 {
                    out.push(VoxInst { off_ao: [p.x, p.y - 0.04, p.z + 0.08, 1.0], color: [0.85, 0.3, 0.2, 1.0], misc: [0.1, 0.0, 0.0, 0.0] });
                }
            }
        }

        // --- glow-moths around the heal trees (denser at night) --------------
        for (ti, tree) in game.wild.trees.iter().enumerate() {
            let n = 6 + (atm.night * 10.0) as usize;
            for i in 0..n {
                let s = (ti * 31 + i) as f32;
                let a = self.time * (0.5 + (s * 0.37).fract() * 0.4) + s;
                let r = 2.5 + (s * 0.61).fract() * 3.5;
                let y = 3.0 + (self.time * 1.3 + s).sin() * 1.2 + (s * 0.23).fract() * 3.0;
                let p = tree.pos + Vec3::new(a.cos() * r, y, a.sin() * r);
                let pulse = 0.5 + 0.5 * (self.time * 4.0 + s).sin();
                out.push(VoxInst { off_ao: [p.x, p.y, p.z, 1.0], color: [1.0, 0.92, 0.6, 0.55 + pulse * 0.4], misc: [0.11, 0.0, 0.0, 0.0] });
            }
        }

        // --- Sweetrain: a slow ring of rain-light marking the mended ground
        if let Some(a) = &game.map_events.active {
            use crate::map_events::MapEventKind as K;
            let c = a.kind.color();
            match a.kind {
                K::Sweetrain => {
                    let segs = 70;
                    for i in 0..segs {
                        let ang = i as f32 / segs as f32 * std::f32::consts::TAU + self.time * 0.15;
                        let bob = (self.time * 2.0 + i as f32 * 0.4).sin() * 0.25;
                        out.push(VoxInst {
                            off_ao: [a.pos.x + ang.cos() * a.radius, 0.35 + bob, a.pos.z + ang.sin() * a.radius, 1.0],
                            color: [c[0], c[1], c[2], 0.85],
                            misc: [0.3, 0.0, 0.0, 0.0],
                        });
                    }
                    // drifting motes of the dragon's rain inside the ring
                    for i in 0..90 {
                        let h = rnd_jitter(i as f32 * 3.7) * 0.5 + 0.5;
                        let h2 = rnd_jitter(i as f32 * 7.3 + 1.0) * 0.5 + 0.5;
                        let r = a.radius * h.sqrt();
                        let ang = h2 * std::f32::consts::TAU + self.time * 0.1;
                        let y = 0.3 + ((self.time * 0.6 + i as f32 * 0.31) % 1.0) * 3.0;
                        out.push(VoxInst {
                            off_ao: [a.pos.x + ang.cos() * r, y, a.pos.z + ang.sin() * r, 1.0],
                            color: [0.6, 0.9, 1.0, 0.6],
                            misc: [0.14, 0.0, 0.0, 0.0],
                        });
                    }
                }
                _ => {}
            }
        }

        // fireflies drifting through the jungle shade — they come out under
        // cloud and at night, and vanish in the rain
        let firefly_gain = (0.45 + 0.55 * atm.night + 0.3 * atm.cloud_cover).min(1.4) * (1.0 - atm.rain) * (1.0 - atm.snow);
        for (anchor, ph) in &self.fireflies {
            let t = self.time;
            let p = Vec3::new(
                anchor.x + (t * 0.4 + ph).sin() * 2.3,
                anchor.y + 0.9 + (t * 0.9 + ph * 2.0).sin() * 0.9,
                anchor.z + (t * 0.33 + ph * 1.3).cos() * 2.3,
            );
            let pulse = (0.32 + 0.34 * (0.5 + 0.5 * (t * 2.3 + ph * 3.0).sin())) * firefly_gain;
            out.push(VoxInst {
                off_ao: [p.x, p.y, p.z, 1.0],
                color: [0.72, 1.0, 0.35, pulse],
                misc: [0.16, 0.0, 0.0, 0.0],
            });
        }

        // falling leaves
        for (i, lf) in self.leaves.iter().enumerate() {
            let col = match i % 5 {
                0 => [0.20, 0.30, 0.10],
                1 => [0.30, 0.27, 0.10],
                2 => [0.16, 0.34, 0.12],
                3 => [0.42, 0.26, 0.10],
                _ => [0.24, 0.32, 0.11],
            };
            out.push(VoxInst {
                off_ao: [lf.pos.x, lf.pos.y, lf.pos.z, 1.0],
                color: [col[0], col[1], col[2], 0.9],
                misc: [0.2, 0.0, 0.0, 0.0],
            });
        }

        for p in &self.particles {
            let lf = (p.life / p.max_life).clamp(0.0, 1.0);
            out.push(VoxInst {
                off_ao: [p.pos.x, p.pos.y, p.pos.z, 1.0],
                color: [p.color[0], p.color[1], p.color[2], p.alpha * lf],
                misc: [p.size * (0.4 + 0.6 * lf), 0.0, 0.0, 0.0],
            });
        }
        // dust: misc.z adds to the particle draw's emissive (1.0) -- minus
        // one makes it an ordinary lit surface
        for p in &self.dust {
            let lf = (p.life / p.max_life).clamp(0.0, 1.0);
            out.push(VoxInst {
                off_ao: [p.pos.x, p.pos.y, p.pos.z, 1.0],
                color: [p.color[0], p.color[1], p.color[2], p.alpha * lf],
                misc: [p.size * (0.5 + 0.5 * lf), 0.0, -1.0, 0.0],
            });
        }

        // beams — cube chains along the segment
        for b in &self.beams {
            let n = (b.length / 0.6).ceil() as usize;
            let lf = ((b.until - self.time) / 0.15).clamp(0.35, 1.0);
            for i in 0..=n {
                let t = i as f32 / n as f32;
                let p = b.from + b.dir * (t * b.length) + Vec3::Y * -0.15 * t;
                let s = b.width * 0.42 * (1.0 - t * 0.25);
                for k in 0..2 {
                    let a = self.time * 6.0 + k as f32 * 2.1;
                    let off = Vec3::new(a.cos(), (k as f32 - 0.5) * 0.35, a.sin()) * 0.22;
                    out.push(VoxInst {
                        off_ao: [p.x + off.x, p.y + off.y, p.z + off.z, 1.0],
                        color: [b.color[0], b.color[1], b.color[2], 0.6 * lf],
                        misc: [s, 0.0, 0.0, 0.0],
                    });
                }
            }
        }

        // expanding rings
        for r in &self.rings {
            let t = 1.0 - (r.until / r.dur.max(0.0001)).clamp(0.0, 1.0);
            let radius = r.r0 + (r.r1 - r.r0) * t;
            let segs = (radius * 7.0).clamp(20.0, 90.0) as usize;
            for i in 0..segs {
                let a = i as f32 / segs as f32 * std::f32::consts::TAU;
                out.push(VoxInst {
                    off_ao: [r.pos.x + a.cos() * radius, 0.35, r.pos.z + a.sin() * radius, 1.0],
                    color: [r.color[0], r.color[1], r.color[2], 0.9 * (1.0 - t * 0.6)],
                    misc: [r.thick, 0.0, 0.0, 0.0],
                });
            }
        }

        // Shields: a GLASS DOME round the cat -- a sphere of small facets,
        // nearly clear face-on and bright at its rim (Fresnel: glass seen
        // edge-on reflects), slowly turning. It was 30 cubes orbiting.
        for g in &game.gods {
            if g.shield > 0.0 && g.respawn_t <= 0.0 {
                let color = if g.team == crate::consts::Team::Blue { [0.35, 0.7, 1.0] } else { [1.0, 0.45, 0.35] };
                for f in shield_facets(g.pos + Vec3::Y * 1.0, 1.35, self.time, cam_eye) {
                    out.push(VoxInst { off_ao: [f.0.x, f.0.y, f.0.z, 1.0], color: [color[0] * 1.6, color[1] * 1.6, color[2] * 1.6, f.1], misc: [0.17, 0.0, 0.0, 0.0] });
                }
            }
        }

        // zone discs: storm swirl + ring telegraphs from the sim
        for z in &game.zones {
            match &z.kind {
                ZoneKind::Storm { .. } => {
                    let segs = (z.radius * 6.0).clamp(24.0, 70.0) as usize;
                    for i in 0..segs {
                        let a = i as f32 / segs as f32 * std::f32::consts::TAU + self.time * 0.9;
                        let wob = (self.time * 4.0 + i as f32 * 0.7).sin() * 0.3;
                        out.push(VoxInst {
                            off_ao: [
                                z.pos.x + a.cos() * z.radius,
                                0.4 + wob,
                                z.pos.z + a.sin() * z.radius,
                                1.0,
                            ],
                            color: [z.color[0], z.color[1], z.color[2], 0.65],
                            misc: [0.35, 0.0, 0.0, 0.0],
                        });
                    }
                }
                ZoneKind::Ring { trigger_t, .. } => {
                    let pulse = (self.time * 6.0).sin() * 0.5 + 0.5;
                    let segs = (z.radius * 6.0).clamp(20.0, 60.0) as usize;
                    for i in 0..segs {
                        let a = i as f32 / segs as f32 * std::f32::consts::TAU;
                        out.push(VoxInst {
                            off_ao: [
                                z.pos.x + a.cos() * z.radius,
                                0.25 + pulse * 0.5,
                                z.pos.z + a.sin() * z.radius,
                                1.0,
                            ],
                            color: [z.color[0], z.color[1], z.color[2], 0.8],
                            misc: [0.4, 0.0, 0.0, 0.0],
                        });
                    }
                    let _ = trigger_t;
                }
            }
        }

        // fountain sparkles
        for team in [crate::consts::Team::Blue, crate::consts::Team::Red] {
            let f = team.fountain();
            let c = team.rgb();
            for i in 0..10 {
                let a = self.time * 1.4 + i as f32 * 0.63;
                let r = 2.5 + (self.time * 0.8 + i as f32).sin() * 1.8;
                out.push(VoxInst {
                    off_ao: [f.x + a.cos() * r, 1.2 + (self.time * 2.0 + i as f32 * 0.7).sin() * 0.9, f.z + a.sin() * r, 1.0],
                    color: [c[0], c[1], c[2], 0.75],
                    misc: [0.26, 0.0, 0.0, 0.0],
                });
            }
        }

        out
    }
}

impl Vfx {
    /// Fire cells (flames, smoke, embers), ash over charred ground, glowing
    /// fissures, the tornado funnel. All hashed from state each frame.
    fn disaster_visuals(&self, out: &mut Vec<VoxInst>, eye: Vec3, game: &Game) {
        use crate::disasters::FIRE_CELL;
        let t = self.time;
        let d = &game.disasters;
        let wind = game.weather.atmosphere().wind;
        // flames: only cells within sight
        for f in &d.fire {
            if f.pos.distance(eye) > 90.0 {
                continue;
            }
            let heat = (f.burn / crate::disasters::FIRE_BURN_SECS).clamp(0.2, 1.0);
            let n = (26.0 * heat) as usize;
            for i in 0..n {
                let s = i as f32 + f.pos.x * 0.37 + f.pos.z * 0.91;
                let ph = (t * (1.6 + rnd_jitter(s).abs()) + rnd_jitter(s * 3.0) * 4.0).fract();
                let x = f.pos.x + rnd_jitter(s * 1.7) * FIRE_CELL * 0.45 + wind.x * 0.06 * ph * 3.0;
                let z = f.pos.z + rnd_jitter(s * 2.3) * FIRE_CELL * 0.45 + wind.z * 0.06 * ph * 3.0;
                let y = 0.2 + ph * (2.2 + rnd_jitter(s * 5.1).abs() * 2.0);
                let col = if ph < 0.5 { [1.0, 0.55 + ph * 0.5, 0.1] } else { [1.0, 0.35, 0.08] };
                let size = 0.32 * (1.0 - ph * 0.7) * heat + 0.06;
                out.push(VoxInst { off_ao: [x, y, z, 1.0], color: [col[0], col[1], col[2], 1.0 - ph * 0.6], misc: [size, 0.0, 0.0, 0.0] });
            }
            // smoke column leaning downwind
            for i in 0..10 {
                let s = i as f32 * 1.3 + f.pos.z * 0.2;
                let ph = (t * 0.35 + rnd_jitter(s) * 2.0).fract();
                let y = 2.5 + ph * 12.0;
                let x = f.pos.x + wind.x * 0.25 * ph * 8.0 + rnd_jitter(s * 2.0) * 1.2;
                let z = f.pos.z + wind.z * 0.25 * ph * 8.0 + rnd_jitter(s * 3.0) * 1.2;
                out.push(VoxInst { off_ao: [x, y, z, 1.0], color: [0.22, 0.2, 0.19, 0.5 * (1.0 - ph)], misc: [0.6 + ph * 1.2, 0.0, 0.0, 0.0] });
            }
            // embers riding up and off
            for i in 0..6 {
                let s = i as f32 * 4.1 + f.pos.x;
                let ph = (t * 0.9 + rnd_jitter(s) * 3.0).fract();
                out.push(VoxInst { off_ao: [f.pos.x + rnd_jitter(s * 2.0) * 3.0 + wind.x * 0.3 * ph * 6.0, 1.0 + ph * 9.0, f.pos.z + rnd_jitter(s * 3.0) * 3.0 + wind.z * 0.3 * ph * 6.0, 1.0], color: [1.0, 0.7, 0.3, 0.9 * (1.0 - ph)], misc: [0.07, 0.0, 0.0, 0.0] });
            }
        }
        // ash drifting over charred ground near the camera
        for c in &d.charred {
            if c.distance(eye) > 45.0 {
                continue;
            }
            for i in 0..3 {
                let s = i as f32 * 2.7 + c.x * 0.13 + c.z * 0.29;
                let ph = (t * 0.2 + rnd_jitter(s) * 3.0).fract();
                out.push(VoxInst { off_ao: [c.x + rnd_jitter(s * 2.0) * 3.0, 0.3 + ph * 3.0, c.z + rnd_jitter(s * 3.0) * 3.0, 1.0], color: [0.5, 0.48, 0.46, 0.5 * (1.0 - ph)], misc: [0.09, 0.0, 0.0, 0.0] });
            }
        }
        // fissures: a jagged line of dark stone with a molten core while hot
        for f in &d.fissures {
            if f.a.distance(eye) > 110.0 && f.b.distance(eye) > 110.0 {
                continue;
            }
            let n = ((f.b - f.a).length() * 2.2) as usize;
            for i in 0..=n {
                let tt = i as f32 / n.max(1) as f32;
                let jag = Vec3::new(rnd_jitter(tt * 13.0 + f.a.x), 0.0, rnd_jitter(tt * 17.0 + f.a.z)) * 0.5;
                let p = f.a + (f.b - f.a) * tt + jag;
                out.push(VoxInst { off_ao: [p.x, 0.06, p.z, 1.0], color: [0.12, 0.1, 0.1, 1.0], misc: [0.55, 0.0, 0.0, 0.0] });
                if f.heat > 0.05 {
                    let pulse = 0.6 + 0.4 * (t * 3.0 + tt * 6.0).sin();
                    out.push(VoxInst { off_ao: [p.x, 0.12, p.z, 1.0], color: [1.0, 0.45 * pulse, 0.1, f.heat], misc: [0.3 * f.heat + 0.05, 0.0, 0.0, 0.0] });
                    if f.heat > 0.5 && i % 3 == 0 {
                        let ph = (t * 1.2 + tt * 5.0).fract();
                        out.push(VoxInst { off_ao: [p.x, 0.3 + ph * 2.0, p.z, 1.0], color: [1.0, 0.6, 0.2, (1.0 - ph) * f.heat], misc: [0.08, 0.0, 0.0, 0.0] });
                    }
                }
            }
        }
        // the funnel: a spiral of dust widening upward, debris in the outer band
        if let Some(tn) = &d.tornado {
            for i in 0..260 {
                let s = i as f32;
                let h = (s / 260.0) * 26.0;
                let r = 1.2 + h * 0.28 + rnd_jitter(s * 1.3).abs() * 0.8;
                let a = tn.spin * (1.0 + h * 0.02) + s * 0.37;
                let p = tn.pos + Vec3::new(a.cos() * r, h + 0.2, a.sin() * r);
                let grey = 0.4 + (h / 26.0) * 0.3;
                out.push(VoxInst { off_ao: [p.x, p.y, p.z, 1.0], color: [grey, grey * 0.95, grey * 0.9, 0.55 - h * 0.012], misc: [0.35 + h * 0.03, 0.0, 0.0, 0.0] });
            }
            // ground skirt
            for i in 0..60 {
                let s = i as f32;
                let a = tn.spin * 1.4 + s * 0.105;
                let r = 4.0 + rnd_jitter(s).abs() * 6.0;
                out.push(VoxInst { off_ao: [tn.pos.x + a.cos() * r, 0.3 + rnd_jitter(s * 2.0).abs() * 1.5, tn.pos.z + a.sin() * r, 1.0], color: [0.45, 0.38, 0.3, 0.6], misc: [0.3, 0.0, 0.0, 0.0] });
            }
        }
    }

    /// River life near the camera: translucent gelatin blobs riding the
    /// surface (the "liquid" read of the water), splash rings around waders,
    /// dark fish silhouettes under the surface with the odd leap, and
    /// dragonflies stitching the air above the channel. All stateless.
    fn river_life(&self, out: &mut Vec<VoxInst>, eye: Vec3, game: &Game, atm: &crate::weather::Atmosphere) {
        use crate::consts::{river_x, water_depth, WATER_LEVEL};
        let t = self.time;
        // only the river nearest the camera, only the stretch in view range
        let side = if eye.x < 0.0 { -1.0 } else { 1.0 };
        let z0 = eye.z - 42.0;
        // gelatin blobs: one every ~1.4 u of channel, jittered across it,
        // bobbing on a wave phase, squashing as they crest
        let mut k = 0;
        let mut z = z0;
        while z < eye.z + 42.0 {
            let s = k as f32 + (z0 / 1.4).floor() * 0.0; // stable per-cell seed
            let cellz = (z / 1.4).floor();
            let h1 = rnd_jitter(cellz * 1.37);
            let h2 = rnd_jitter(cellz * 2.91 + 5.0);
            let cx = river_x(side, z);
            let x = cx + h1 * 3.6;
            let zz = cellz * 1.4 + h2 * 0.6;
            let depth = water_depth(x, zz);
            if depth > 0.15 {
                let wave = (zz * 1.6 - t * 2.6).sin() * 0.06 + (x * 1.3 + zz * 0.7 - t * 1.7).sin() * 0.04;
                let y = WATER_LEVEL + wave + 0.08;
                let sq = 0.22 + wave * 1.2; // crests flatten, troughs bead up
                let a = 0.32 + 0.2 * (t * 1.9 + h1 * 9.0).sin().abs();
                let col = [0.55, 0.85, 0.95, a * (1.0 - atm.night * 0.5)];
                out.push(VoxInst { off_ao: [x, y, zz, 1.0], color: col, misc: [sq, 0.0, 0.0, 0.0] });
                // a smaller bead trailing downstream
                out.push(VoxInst { off_ao: [x + h2 * 0.3, y - 0.04, zz - 0.35, 1.0], color: [0.7, 0.95, 1.0, a * 0.7], misc: [sq * 0.6, 0.0, 0.0, 0.0] });
                // fish: every 4th cell, a dark sliver swimming with the flow
                if k % 4 == 0 {
                    let fz = zz - ((t * 2.2 + h1 * 10.0) % 8.0) + 4.0;
                    let fx = river_x(side, fz) + h2 * 2.0;
                    if water_depth(fx, fz) > 0.6 {
                        // the leap: once every ~12 s per fish, a short arc above the surface
                        let leap_ph = (t * 0.083 + h1 * 3.0).fract();
                        let jump = if leap_ph < 0.09 { (leap_ph / 0.09 * std::f32::consts::PI).sin() * 1.1 } else { 0.0 };
                        let fy = WATER_LEVEL - 0.35 + jump + (t * 3.0 + h2).sin() * 0.05;
                        let fc = if jump > 0.0 { [0.75, 0.8, 0.7, 0.95] } else { [0.05, 0.12, 0.12, 0.55] };
                        out.push(VoxInst { off_ao: [fx, fy, fz, 1.0], color: fc, misc: [0.14, 0.0, 0.0, 0.0] });
                        out.push(VoxInst { off_ao: [fx + h2 * 0.05, fy, fz - 0.16, 1.0], color: fc, misc: [0.12, 0.0, 0.0, 0.0] });
                        out.push(VoxInst { off_ao: [fx, fy + 0.02, fz - 0.3, 1.0], color: fc, misc: [0.09, 0.0, 0.0, 0.0] });
                        if jump > 0.9 {
                            // splash droplets at the top of the leap
                            for j in 0..4 {
                                let a = j as f32 * 1.57 + t;
                                out.push(VoxInst { off_ao: [fx + a.cos() * 0.3, WATER_LEVEL + 0.2 + (a * 2.0).sin().abs() * 0.3, fz + a.sin() * 0.3, 1.0], color: [0.8, 0.9, 1.0, 0.7], misc: [0.07, 0.0, 0.0, 0.0] });
                            }
                        }
                    }
                }
                // dragonflies: every 7th cell, darting above the water by day
                if k % 7 == 3 && atm.night < 0.6 && atm.rain < 0.3 {
                    let dart = ((t * 0.7 + h2 * 4.0).sin() * 0.5 + 0.5).powf(3.0);
                    let dx = x + (t * 1.1 + h1 * 6.0).sin() * 2.0 * dart;
                    let dz = zz + (t * 0.9 + h2 * 6.0).cos() * 2.5 * dart;
                    let dy = WATER_LEVEL + 1.1 + (t * 2.7 + h1).sin() * 0.3;
                    let body = [0.2, 0.55, 0.75, 0.95];
                    out.push(VoxInst { off_ao: [dx, dy, dz, 1.0], color: body, misc: [0.07, 0.0, 0.0, 0.0] });
                    out.push(VoxInst { off_ao: [dx, dy, dz - 0.12, 1.0], color: body, misc: [0.05, 0.0, 0.0, 0.0] });
                    let wing = [0.8, 0.9, 1.0, 0.35 + (t * 40.0).sin().abs() * 0.2];
                    out.push(VoxInst { off_ao: [dx - 0.12, dy + 0.02, dz, 1.0], color: wing, misc: [0.14, 0.0, 0.0, 0.0] });
                    out.push(VoxInst { off_ao: [dx + 0.12, dy + 0.02, dz, 1.0], color: wing, misc: [0.14, 0.0, 0.0, 0.0] });
                }
                let _ = s;
            }
            k += 1;
            z += 1.4;
        }

        // wading: rings of beads around anyone in the water, faster when moving
        for g in game.gods.iter().chain(std::iter::empty()) {
            if g.respawn_t > 0.0 {
                continue;
            }
            let d = water_depth(g.pos.x, g.pos.z);
            if d < 0.15 {
                continue;
            }
            let moving = g.move_dir.length_squared() > 0.01;
            let n = if moving { 14 } else { 8 };
            let spread = if moving { 1.4 } else { 0.9 };
            for i in 0..n {
                let a = i as f32 / n as f32 * std::f32::consts::TAU + t * 0.8;
                let ph = (t * (if moving { 3.5 } else { 1.6 }) + i as f32 * 0.9).fract();
                let r = 0.5 + ph * spread;
                let y = WATER_LEVEL + 0.05 + (1.0 - ph) * 0.12;
                out.push(VoxInst { off_ao: [g.pos.x + a.cos() * r, y, g.pos.z + a.sin() * r, 1.0], color: [0.8, 0.95, 1.0, (1.0 - ph) * 0.6], misc: [0.12 + (1.0 - ph) * 0.08, 0.0, 0.0, 0.0] });
            }
            // a spray of beads kicked up ahead when moving
            if moving {
                let f = g.move_dir.normalize_or_zero();
                for i in 0..5 {
                    let ph = (t * 4.0 + i as f32 * 0.37).fract();
                    let p = g.pos + f * (0.6 + ph * 0.8) + Vec3::Y * (WATER_LEVEL - g.pos.y + 0.1 + (ph * std::f32::consts::PI).sin() * 0.6);
                    out.push(VoxInst { off_ao: [p.x + rnd_jitter(i as f32 + 3.0) * 0.3, p.y, p.z, 1.0], color: [0.85, 0.95, 1.0, 0.7 * (1.0 - ph)], misc: [0.1, 0.0, 0.0, 0.0] });
                }
            }
        }
        // prey wading too (the run pours down the river)
        for p in &game.wild.prey {
            if p.kind == crate::wild::PreyKind::Glowfish {
                // its own halo of beads
                for i in 0..6 {
                    let a = i as f32 * 1.047 + t * 1.5;
                    out.push(VoxInst { off_ao: [p.pos.x + a.cos() * 0.6, WATER_LEVEL + 0.06, p.pos.z + a.sin() * 0.6, 1.0], color: [0.6, 1.0, 0.9, 0.5], misc: [0.09, 0.0, 0.0, 0.0] });
                }
                continue;
            }
            if water_depth(p.pos.x, p.pos.z) > 0.15 && p.vel.length_squared() > 1.0 {
                for i in 0..4 {
                    let ph = (t * 3.0 + i as f32 * 0.5).fract();
                    let a = i as f32 * 1.57 + t;
                    out.push(VoxInst { off_ao: [p.pos.x + a.cos() * (0.3 + ph * 0.6), WATER_LEVEL + 0.1, p.pos.z + a.sin() * (0.3 + ph * 0.6), 1.0], color: [0.8, 0.95, 1.0, 0.5 * (1.0 - ph)], misc: [0.1, 0.0, 0.0, 0.0] });
                }
            }
        }
    }

    /// Stateless rain / snow: drops are hashed from (cell, time) in a box
    /// around the camera. Rain: two stacked slivers per drop, leaning with the
    /// wind, falling fast. Snow: single flakes, drifting on the wind.
    fn precipitation(&self, out: &mut Vec<VoxInst>, eye: Vec3, atm: &crate::weather::Atmosphere) {
        let t = self.time;
        let wind = atm.wind;
        if atm.rain > 0.02 {
            let n = (2600.0 * atm.rain) as usize;
            let (half, height) = (22.0f32, 26.0f32);
            let speed = 24.0;
            let lean = wind * 0.28;
            for i in 0..n {
                let s = i as f32;
                let hx = rnd_jitter(s * 1.371 + 0.1);
                let hz = rnd_jitter(s * 2.917 + 0.2);
                let hy = rnd_jitter(s * 5.113 + 0.3) * 0.5 + 0.5;
                // anchor the field to camera *cells* so it doesn't slide with the eye
                let ax = (eye.x / (half * 2.0)).floor() * half * 2.0 + hx * half + half;
                let az = (eye.z / (half * 2.0)).floor() * half * 2.0 + hz * half + half;
                let fall = (hy * height + t * speed) % height;
                let y = height - fall;
                let x = ax + lean.x * y * 0.1;
                let z = az + lean.z * y * 0.1;
                if (x - eye.x).abs() > half || (z - eye.z).abs() > half {
                    continue;
                }
                let a = 0.55 * atm.rain;
                out.push(VoxInst { off_ao: [x, y, z, 1.0], color: [0.72, 0.8, 0.95, a], misc: [0.07, 0.0, 0.0, 0.0] });
                out.push(VoxInst { off_ao: [x + lean.x * 0.01, y + 0.09, z + lean.z * 0.01, 1.0], color: [0.72, 0.8, 0.95, a * 0.8], misc: [0.06, 0.0, 0.0, 0.0] });
                // splash ring at the ground for the lowest drops
                if y < 0.35 {
                    out.push(VoxInst { off_ao: [x, 0.08, z, 1.0], color: [0.8, 0.88, 1.0, 0.5], misc: [0.16, 0.0, 0.0, 0.0] });
                }
            }
        }
        // (on ultra it snows a little all the time over the snowy maps:
        // `snowpack::snowfall`)
        let fall_snow = crate::snowpack::snowfall(atm.snow);
        if fall_snow > 0.02 {
            let n = (1800.0 * fall_snow) as usize;
            let (half, height) = (20.0f32, 18.0f32);
            let speed = 1.7;
            for i in 0..n {
                let s = i as f32;
                let hx = rnd_jitter(s * 1.731 + 0.4);
                let hz = rnd_jitter(s * 2.197 + 0.5);
                let hy = rnd_jitter(s * 4.331 + 0.6) * 0.5 + 0.5;
                let ax = (eye.x / (half * 2.0)).floor() * half * 2.0 + hx * half + half;
                let az = (eye.z / (half * 2.0)).floor() * half * 2.0 + hz * half + half;
                let fall = (hy * height + t * speed) % height;
                let y = height - fall;
                let sway = (t * 0.9 + s * 0.37).sin() * 0.6;
                let x = ax + sway + wind.x * 0.12 * (height - y);
                let z = az + (t * 0.7 + s * 0.53).cos() * 0.6 + wind.z * 0.12 * (height - y);
                if (x - eye.x).abs() > half || (z - eye.z).abs() > half {
                    continue;
                }
                out.push(VoxInst { off_ao: [x, y, z, 1.0], color: [0.95, 0.97, 1.0, 0.85 * (0.5 + 0.5 * fall_snow)], misc: [0.11, 0.0, 0.0, 0.0] });
            }
        }
    }
}

fn rnd_jitter(seed: f32) -> f32 {
    let x = (seed * 12.9898).sin() * 43758.5453;
    (x - x.floor()) * 2.0 - 1.0
}

#[cfg(test)]
mod glow_tests {
    use super::*;

    /// A cast lights its caster in its ability's colour; the glow fades
    /// and goes; the lights handed on are the brightest at the camera's
    /// target, not merely the nearest.
    #[test]
    fn a_cast_glows_in_its_colour_then_fades() {
        let game = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        let mut v = Vfx::default();
        v.glow_events(0.0, &[Event::Cast { god: 0, slot: 1, aim: Vec3::ZERO }], &game);
        assert_eq!(v.glows.len(), 1);
        let want = crate::gods::ROSTER[game.gods[0].def_id as usize].abilities[1].color;
        assert_eq!(v.glows[0].color, want);
        let l = v.lights(game.gods[0].pos, 8);
        assert_eq!(l.len(), 1);
        assert!(l[0].1[0] > 0.0 || l[0].1[1] > 0.0 || l[0].1[2] > 0.0);
        v.glow_events(1.0, &[], &game);
        assert!(v.glows.is_empty(), "faded and gone");
    }

    #[test]
    fn the_brightest_lights_win_and_there_are_never_too_many() {
        let game = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        let mut v = Vfx::default();
        let near = Vec3::ZERO;
        let dim_close = Event::HealFx { pos: Vec3::new(1.0, 0.0, 0.0), radius: 2.0 };
        let bright_far = Event::Explosion { pos: Vec3::new(12.0, 0.0, 0.0), radius: 6.0, color: [1.0, 0.5, 0.2] };
        v.glow_events(0.0, &[dim_close, bright_far], &game);
        let l = v.lights(near, 1);
        assert_eq!(l[0].0[0], 12.0, "the explosion outshines the heal at the target");
        let flurry: Vec<Event> = (0..200).map(|k| Event::BoltFx { from: Vec3::ZERO, to: Vec3::X * k as f32, color: [1.0; 3], thick: 1.0 }).collect();
        v.glow_events(0.0, &flurry, &game);
        assert!(v.glows.len() <= 64);
        assert_eq!(v.lights(near, 8).len(), 8);
    }
}

#[cfg(test)]
mod channel_tests {
    use super::*;

    fn caster() -> (Game, usize, u32) {
        let mut game = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        // find a cat with a rooted channel (Beam/Storm, or an ultimate)
        for (i, g) in game.gods.iter().enumerate() {
            let def = &crate::gods::ROSTER[g.def_id as usize];
            if let Some(s) = (0..4).find(|&s| def.abilities[s].ult) {
                return (game, i, s as u32);
            }
        }
        game.t = 0.0;
        (game, 0, 3)
    }

    /// A wind-up shows: a sigil ring timed to the release, motes gathering
    /// more thickly as the charge builds, a brightening light; the cast
    /// firing ends it.
    #[test]
    fn a_channel_gathers_builds_and_ends_on_the_cast() {
        let (game, god, slot) = caster();
        let mut v = Vfx::default();
        let dur = 0.9;
        v.update(0.0, &[Event::CastStart { god, slot, aim: Vec3::ZERO, dur }], &game);
        assert_eq!(v.channels.len(), 1);
        assert!(v.channels[0].big, "an ultimate is the big telling");
        let ring = v.rings.last().unwrap();
        assert!((ring.dur - dur).abs() < 1e-6, "the sigil fills at the release");
        let count = |v: &mut Vfx, secs: f32| {
            let before = v.particles.len();
            let mut n = 0;
            for _ in 0..(secs * 60.0) as usize {
                let b = v.particles.len();
                v.tick_channels(1.0 / 60.0, &[], &game);
                n += v.particles.len() - b;
            }
            let _ = before;
            n
        };
        let early = count(&mut v, 0.2);
        let _mid = count(&mut v, 0.4);
        let late = count(&mut v, 0.2);
        assert!(late > early, "the gathering thickens as the charge builds: {early} then {late}");
        let lit = v.lights(game.gods[god].pos, 8);
        assert!(!lit.is_empty(), "the charge gives light");
        v.update(1.0 / 60.0, &[Event::Cast { god, slot, aim: Vec3::ZERO }], &game);
        assert!(v.channels.is_empty(), "the cast fired: the channel is over");
    }

    #[test]
    fn a_channel_stops_if_the_caster_dies() {
        let (mut game, god, slot) = caster();
        let mut v = Vfx::default();
        v.update(0.0, &[Event::CastStart { god, slot, aim: Vec3::ZERO, dur: 0.9 }], &game);
        game.gods[god].respawn_t = 5.0;
        v.tick_channels(1.0 / 60.0, &[], &game);
        assert!(v.channels.is_empty());
    }
}

#[cfg(test)]
mod lightning_tests {
    use super::*;

    /// A strike draws a jagged bolt from the sky to the ground -- pinned at
    /// the strike, wandering in between, forked -- bright enough to bloom,
    /// and gives the brightest light of any effect.
    #[test]
    fn a_strike_is_a_jagged_forked_blinding_bolt() {
        let game = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        let mut v = Vfx::default();
        let at = Vec3::new(4.0, 0.0, -7.0);
        v.update(1.0 / 60.0, &[Event::StrikeFx { at, color: [0.6, 0.6, 1.0] }], &game);
        let bolt: Vec<&Particle> = v.particles.iter().filter(|p| p.vel == Vec3::ZERO).collect();
        assert!(bolt.len() > 80, "{} bolt motes", bolt.len());
        assert!(bolt.iter().any(|p| p.pos.y > 12.0) && bolt.iter().any(|p| p.pos.y < 0.5), "sky to ground");
        let wander = bolt.iter().map(|p| Vec3::new(p.pos.x - at.x, 0.0, p.pos.z - at.z).length()).fold(0.0f32, f32::max);
        assert!(wander > 0.4, "jagged, not a straight line: {wander}");
        assert!(bolt.iter().all(|p| p.color[2] > 1.5), "HDR white: it blooms");
        let strike = v.lights(at, 8)[0];
        assert!(strike.1[2] > 10.0, "the brightest light: {:?}", strike.1);
        assert_eq!(v.rings.len(), 1, "a flash ring where it lands");
    }

    /// The sim's storms strike with StrikeFx, not a projectile bolt.
    #[test]
    fn storms_strike_with_lightning() {
        let mut game = Game::new_teams(&[11], &[0]); // Stillsmoke
        game.gods[1].respawn_t = 999.0;
        game.gods[0].ai = None;
        game.gods[0].mana = 10_000.0;
        let def = &crate::gods::ROSTER[game.gods[0].def_id as usize];
        let slot = (0..4).find(|&s| matches!(def.abilities[s].arch, crate::gods::Archetype::Storm { .. })).expect("a storm") as u32;
        let p = game.gods[0].pos;
        game.cast_queue.push((0, slot, p + Vec3::Z * 6.0));
        let mut strikes = 0;
        for _ in 0..240 {
            game.tick(1.0 / 60.0);
            strikes += game.events.drain(..).filter(|e| matches!(e, Event::StrikeFx { .. })).count();
        }
        assert!(strikes >= 3, "{strikes} strikes in 4 s");
    }
}

#[cfg(test)]
mod ability_fx_tests {
    use super::*;

    /// A dash leaves a string of ghosts along its path; a blink leaves one
    /// where it started; a respawn (a far jump) leaves none; they fade.
    #[test]
    fn dashes_and_blinks_leave_afterimages() {
        let mut game = Game::new_teams(&[0], &[1]);
        let mut v = Vfx::default();
        v.tick_afterimages(1.0 / 60.0, &game);
        // a blink: 6 u in one frame
        let from = game.gods[0].pos;
        game.gods[0].pos += Vec3::X * 6.0;
        v.tick_afterimages(1.0 / 60.0, &game);
        assert_eq!(v.afterimages.len(), 1);
        assert!((v.afterimages[0].pos - from).length() < 1e-4, "the ghost is where it started");
        // a respawn-sized jump is not a blink
        game.gods[0].pos += Vec3::X * 80.0;
        v.tick_afterimages(1.0 / 60.0, &game);
        assert_eq!(v.afterimages.len(), 1);
        // fading out
        for _ in 0..40 {
            v.tick_afterimages(1.0 / 60.0, &game);
        }
        assert!(v.afterimages.is_empty(), "faded");
    }

    #[test]
    fn a_real_dash_leaves_a_string_of_ghosts() {
        let mut game = Game::new_teams(&[0], &[1]);
        game.gods[1].respawn_t = 999.0;
        game.gods[0].ai = None;
        game.gods[0].mana = 10_000.0;
        let def = &crate::gods::ROSTER[game.gods[0].def_id as usize];
        let slot = (0..4).find(|&s| matches!(def.abilities[s].arch, crate::gods::Archetype::Dash { .. })).expect("Goldenmane dashes") as u32;
        let p = game.gods[0].pos;
        game.cast_queue.push((0, slot, p + Vec3::Z * 8.0));
        let mut v = Vfx::default();
        let mut most = 0;
        for _ in 0..40 {
            game.tick(1.0 / 60.0);
            let ev: Vec<Event> = game.events.drain(..).collect();
            v.update(1.0 / 60.0, &ev, &game);
            most = most.max(v.afterimages.len());
        }
        assert!(most >= 3, "{most} ghosts along the dash");
    }

    /// A nova throws dust outward along the ground; a bolt in flight
    /// streams a trail behind it.
    #[test]
    fn novas_throw_dust_and_bolts_trail() {
        let game = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        let mut v = Vfx::default();
        let c = Vec3::new(3.0, 0.0, 3.0);
        v.update(1.0 / 60.0, &[Event::NovaFx { pos: c, radius: 6.0, color: [1.0, 0.6, 0.2] }], &game);
        let dust: Vec<&Particle> = v.dust.iter().collect();
        assert!(dust.len() >= 24);
        assert!(dust.iter().all(|p| Vec3::new(p.vel.x, 0.0, p.vel.z).dot(p.pos - c) > 0.0 && p.pos.y < 0.5), "outward, low");
        let mut game = Game::new_teams(&[0], &[1]);
        let def = &crate::gods::ROSTER[game.gods[0].def_id as usize];
        if let Some(slot) = (0..4).find(|&s| matches!(def.abilities[s].arch, crate::gods::Archetype::Bolt { .. })) {
            game.gods[0].mana = 10_000.0;
            game.gods[0].ai = None;
            let p = game.gods[0].pos;
            game.cast_queue.push((0, slot as u32, p + Vec3::Z * 10.0));
            let mut v = Vfx::default();
            for _ in 0..30 {
                game.tick(1.0 / 60.0);
                game.events.clear();
                v.tick_trails(&game);
            }
            assert!(!v.particles.is_empty(), "the bolt trailed");
        }
    }
}

#[cfg(test)]
mod ring_tests {
    use super::*;

    /// The channel sigil and a lightning ring grow and are gone on time
    /// -- even late in a match, when the game clock is large.
    #[test]
    fn rings_expire_on_time_late_in_a_match() {
        let (game, _, _) = (Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]), 0, 0);
        let mut v = Vfx::default();
        v.time = 900.0; // fifteen minutes in
        v.update(0.0, &[Event::CastStart { god: 0, slot: 3, aim: Vec3::ZERO, dur: 0.6 }, Event::StrikeFx { at: Vec3::X * 5.0, color: [0.6, 0.6, 1.0] }], &game);
        assert_eq!(v.rings.len(), 2);
        for _ in 0..45 {
            v.update(1.0 / 60.0, &[], &game);
        }
        assert!(v.rings.is_empty(), "rings still alive after 0.75 s: {:?}", v.rings.iter().map(|r| r.until).collect::<Vec<_>>());
    }
}

/// The shield dome's facets: points on a sphere about `c` (a Fibonacci
/// spiral, turning with `time`), each with its alpha -- faint where the
/// dome faces the camera, bright where it is seen edge-on.
pub fn shield_facets(c: Vec3, r: f32, time: f32, eye: Vec3) -> Vec<(Vec3, f32)> {
    let n = 110;
    let golden = std::f32::consts::PI * (3.0 - 5.0f32.sqrt());
    let spin = time * 0.35;
    let to_eye = (eye - c).normalize_or_zero();
    (0..n)
        .filter_map(|i| {
            let y = 1.0 - (i as f32 + 0.5) / n as f32 * 2.0;
            let rr = (1.0 - y * y).max(0.0).sqrt();
            let a = golden * i as f32 + spin;
            let nrm = Vec3::new(a.cos() * rr, y, a.sin() * rr);
            // the dome sits on the ground: nothing below it
            let p = c + nrm * r;
            if p.y < c.y - r * 0.75 {
                return None;
            }
            let facing = nrm.dot(to_eye).abs();
            let alpha = 0.1 + 0.75 * (1.0 - facing).powi(2);
            Some((p, alpha))
        })
        .collect()
}

#[cfg(test)]
mod more_fx_tests {
    use super::*;

    /// A shield is a glass dome: faint where it faces the camera, bright
    /// edge-on, on a sphere round the cat, nothing below the ground.
    #[test]
    fn a_shield_is_a_glass_dome() {
        let c = Vec3::new(0.0, 1.0, 0.0);
        let eye = Vec3::new(0.0, 1.0, -10.0);
        let f = shield_facets(c, 1.35, 0.0, eye);
        assert!(f.len() > 50);
        assert!(f.iter().all(|(p, _)| ((*p - c).length() - 1.35).abs() < 1e-3 && p.y >= c.y - 1.35 * 0.75 - 1e-3));
        let facing: Vec<f32> = f.iter().filter(|(p, _)| (*p - c).normalize().dot((eye - c).normalize()).abs() > 0.9).map(|x| x.1).collect();
        let edge: Vec<f32> = f.iter().filter(|(p, _)| (*p - c).normalize().dot((eye - c).normalize()).abs() < 0.2).map(|x| x.1).collect();
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len().max(1) as f32;
        assert!(!facing.is_empty() && !edge.is_empty());
        assert!(mean(&edge) > mean(&facing) * 3.0, "rim {} vs face-on {}", mean(&edge), mean(&facing));
    }

    /// A heal spreads a ring and sends motes up a helix; an explosion
    /// kicks up lit dust.
    #[test]
    fn heals_spiral_and_impacts_raise_dust() {
        let game = Game::new(0, [1, 2, 3, 4], [5, 6, 7, 8, 9]);
        let mut v = Vfx::default();
        v.update(1.0 / 60.0, &[Event::HealFx { pos: Vec3::ZERO, radius: 4.0 }], &game);
        assert_eq!(v.rings.len(), 1);
        // the helix motes move round the centre, not straight up
        let turning = v.particles.iter().filter(|p| Vec3::new(p.vel.x, 0.0, p.vel.z).length() > 1.0).count();
        assert!(turning >= 30, "{turning} motes turning");
        let mut v = Vfx::default();
        v.update(1.0 / 60.0, &[Event::Explosion { pos: Vec3::ZERO, radius: 3.0, color: [1.0, 0.5, 0.2] }], &game);
        assert!(v.dust.len() >= 12, "impact dust: {}", v.dust.len());
    }
}

#[cfg(test)]
mod snow_tests {
    use super::*;

    /// On the snowpack: a blast throws a crater and a spray of snow; a
    /// fire nova melts; a cat wading a drift ploughs a trench behind it.
    #[test]
    fn abilities_and_cats_work_the_snow() {
        crate::map::with(crate::map::MapId::Sky, || {
            let mut game = Game::new_teams(&[0], &[1]);
            let mut v = Vfx::default();
            v.snow = Some(crate::snowpack::Snowpack::new());
            v.snow_map = Some(crate::map::MapId::Sky);
            let s = v.snow.as_ref().unwrap();
            // a deep spot, and another, on the floor
            let spots: Vec<Vec3> = (0..crate::snowpack::N * crate::snowpack::N)
                .filter(|&k| s.holds[k] && s.depth[k] > 0.8)
                .map(|k| Vec3::new(s.x0 + ((k % crate::snowpack::N) as f32 + 0.5) * s.cell, 0.0, s.z0 + ((k / crate::snowpack::N) as f32 + 0.5) * s.cell))
                .filter(|p| (0..6).all(|q| s.depth_at(p.x + q as f32, p.z) > 0.6))
                .take(400)
                .collect();
            let (a, b) = (spots[0], *spots.iter().find(|p| p.distance(spots[0]) > 40.0).unwrap());
            game.gods[0].pos = a;
            let d0 = v.snow.as_ref().unwrap().depth_at(a.x, a.z);
            let dust0 = v.dust.len();
            v.tick_snow(1.0 / 60.0, &[Event::Explosion { pos: a, radius: 4.0, color: [0.8, 0.8, 0.9] }], &game);
            assert!(v.snow.as_ref().unwrap().depth_at(a.x, a.z) < d0 * 0.5, "a crater");
            assert!(v.dust.len() > dust0 + 10, "and a spray of snow");
            // fire melts: the snow is gone, not thrown aside (a blast keeps
            // the total, throwing it onto the rim)
            let (db, t0) = (v.snow.as_ref().unwrap().depth_at(b.x, b.z), v.snow.as_ref().unwrap().total());
            v.tick_snow(1.0 / 60.0, &[Event::NovaFx { pos: b, radius: 3.0, color: [1.0, 0.45, 0.15] }], &game);
            let s = v.snow.as_ref().unwrap();
            assert!(s.depth_at(b.x, b.z) < db * 0.8 && s.total() < t0 - 0.3, "fire melts: {t0} -> {}", s.total());
            // wading: walk the cat across a fresh drift
            let c = *spots.iter().find(|p| p.distance(a) > 20.0 && p.distance(b) > 20.0).unwrap();
            for k in 0..60 {
                game.gods[0].pos = c + Vec3::X * (k as f32 * 0.08);
                v.tick_snow(1.0 / 60.0, &[], &game);
            }
            let s = v.snow.as_ref().unwrap();
            assert!(s.press_at(c.x + 2.5, c.z) > 0.6, "a trench behind it: {}", s.press_at(c.x + 2.5, c.z));
        });
    }
}

#[cfg(test)]
mod sky_life_tests {
    use super::*;

    /// A dash's wake pushes billows aside -- near the line, not far off it
    /// -- and closes again.
    #[test]
    fn a_dash_parts_the_cloud_and_it_closes() {
        let g = Gust { from: Vec3::ZERO, to: Vec3::X * 10.0, age: 0.1 };
        let near = g.push(Vec3::new(5.0, 2.0, 2.0));
        assert!(near.z > 2.0, "pushed away from the line: {near}");
        assert_eq!(g.push(Vec3::new(5.0, 2.0, 20.0)), Vec3::ZERO, "not far off it");
        let later = Gust { age: GUST_SECS * 0.9, ..g }.push(Vec3::new(5.0, 2.0, 2.0));
        assert!(later.length() < near.length() * 0.2, "closing again: {later}");
    }

    /// On the sky map, a dash through a bank leaves a wake; the edges near
    /// the player crumble now and then; off it, neither.
    #[test]
    fn the_sky_map_is_alive() {
        crate::map::with(crate::map::MapId::Sky, || {
            let mut game = Game::new_teams(&[0], &[1]);
            // a time and place with a thick bank
            let (t, bank) = (60..900)
                .map(|s| s as f32 * 4.0)
                .find_map(|t| crate::skycloud::clouds(game.cloud_seed, t).into_iter().find(|c| c.kind == crate::skycloud::Kind::Bank && c.thick > 0.9).map(|c| (t, c)))
                .expect("a bank");
            game.t = t;
            // the player's cat on an island's edge, for the crumbling
            let floor = crate::sky::floor();
            let edge = (0..4000).map(|k| Vec3::new((k % 80) as f32 * 8.0 - 320.0, 0.0, (k / 80) as f32 * 8.0 - 200.0)).find(|p| floor.sdf(p.x, p.z) < -4.0 && floor.sdf(p.x + 10.0, p.z) > 5.0).expect("an edge");
            game.gods[0].pos = edge;
            let mut v = Vfx::default();
            let d0 = v.dust.len();
            for _ in 0..(6 * 60) {
                v.tick_sky_life(1.0 / 60.0, &[], &game);
            }
            assert!(v.dust.len() > d0 + 10, "bits crumbled off: {}", v.dust.len() - d0);
            v.tick_sky_life(1.0 / 60.0, &[Event::DashFx { pos: bank.pos, dir: Vec3::X, color: [1.0; 3] }], &game);
            assert_eq!(v.gusts.len(), 1, "a dash through the bank parts it");
            let cs = crate::skycloud::clouds(game.cloud_seed, game.t);
            let far = (1..40).map(|k| bank.pos + Vec3::new(k as f32 * 20.0, 0.0, 30.0)).find(|p| crate::skycloud::density(&cs, p.x, p.z) < 0.01 && crate::skycloud::density(&cs, p.x - 6.0, p.z) < 0.01).expect("clear air");
            v.tick_sky_life(1.0 / 60.0, &[Event::DashFx { pos: far, dir: Vec3::X, color: [1.0; 3] }], &game);
            assert_eq!(v.gusts.len(), 1, "a dash in clear air parts nothing");
        });
        let mut v = Vfx::default();
        let game = Game::new_teams(&[0], &[1]);
        for _ in 0..300 {
            v.tick_sky_life(1.0 / 60.0, &[], &game);
        }
        assert!(v.dust.is_empty(), "the jungle does not crumble");
    }
}
