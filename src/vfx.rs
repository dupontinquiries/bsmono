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
    pub until: f32,
    pub dur: f32,
    pub color: [f32; 3],
    pub thick: f32,
}

#[derive(Default)]
pub struct Vfx {
    pub particles: Vec<Particle>,
    pub beams: Vec<Beam>,
    pub rings: Vec<Ring>,
    pub time: f32,
    /// ambient jungle life (seeded lazily on first update)
    fireflies: Vec<(Vec3, f32)>,
    leaves: Vec<Particle>,
    leaf_spots: Vec<Vec3>,
    seeded: bool,
}

impl Vfx {
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

    pub fn update(&mut self, dt: f32, events: &[Event], game: &Game) {
        self.time += dt;
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
            for (i, g) in game.gods.iter().enumerate() {
                if g.respawn_t > 0.0 || g.move_dir.length_squared() < 0.01 {
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
                Event::Explosion { pos, radius, color } => {
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
                    for _ in 0..40 {
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
        for p in self.particles.iter_mut() {
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
        if self.particles.len() > MAX_PARTICLES {
            self.particles.drain(0..self.particles.len() - MAX_PARTICLES);
        }
        self.beams.retain(|b| b.until > self.time);
        self.rings.retain(|r| r.until > 0.0);
        for r in self.rings.iter_mut() {
            r.until -= dt;
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

        // persistent shield shells on shielded gods
        for g in &game.gods {
            if g.shield > 0.0 && g.respawn_t <= 0.0 {
                let color = if g.team == crate::consts::Team::Blue { [0.45, 0.75, 1.0] } else { [1.0, 0.5, 0.45] };
                for i in 0..30 {
                    let a = i as f32 / 30.0 * std::f32::consts::TAU + self.time * 1.6;
                    let wob = (self.time * 3.0 + i as f32).sin() * 0.12;
                    out.push(VoxInst {
                        off_ao: [
                            g.pos.x + a.cos() * 1.25,
                            g.pos.y + 1.15 + wob + ((i % 3) as f32 - 1.0) * 0.55,
                            g.pos.z + a.sin() * 1.25,
                            1.0,
                        ],
                        color: [color[0], color[1], color[2], 0.5],
                        misc: [0.3, 0.0, 0.0, 0.0],
                    });
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
        if atm.snow > 0.02 {
            let n = (1800.0 * atm.snow) as usize;
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
                out.push(VoxInst { off_ao: [x, y, z, 1.0], color: [0.95, 0.97, 1.0, 0.85 * atm.snow], misc: [0.11, 0.0, 0.0, 0.0] });
            }
        }
    }
}

fn rnd_jitter(seed: f32) -> f32 {
    let x = (seed * 12.9898).sin() * 43758.5453;
    (x - x.floor()) * 2.0 - 1.0
}
