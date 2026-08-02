//! Particles: tiny emissive spheres pushed through the normal scene pipeline.
//! Boost trails, ring-pass confetti, bounce dust, boost-pad sparkles, landing
//! puffs. Fixed-capacity ring buffer, zero allocation after startup.

use glam::{Mat3, Mat4, Vec3};

use crate::render::{ObjDraw, ObjectRaw};
use crate::world::{Rng, M_SPHERE};

const CAP: usize = 700;

#[derive(Clone, Copy)]
struct Particle {
    pos: Vec3,
    vel: Vec3,
    life: f32,
    max: f32,
    size: f32,
    grow: f32, // size multiplier over life (dust grows, sparks shrink)
    color: [f32; 4],
    emissive: f32,
    grav: f32,
    drag: f32,
}

pub struct Fx {
    parts: Vec<Particle>,
    head: usize,
    rng: Rng,
}

impl Fx {
    pub fn new(seed: u64) -> Fx {
        Fx { parts: Vec::with_capacity(CAP), head: 0, rng: Rng::new(seed ^ 0xF00D) }
    }

    fn push(&mut self, p: Particle) {
        if self.parts.len() < CAP {
            self.parts.push(p);
        } else {
            self.head = (self.head + 1) % CAP;
            self.parts[self.head] = p;
        }
    }

    fn jitter(&mut self, s: f32) -> Vec3 {
        Vec3::new(
            self.rng.range(-s, s),
            self.rng.range(-s, s),
            self.rng.range(-s, s),
        )
    }

    /// Two little embers out the back every frame while boosting.
    /// ION THRUSTER: what an electric car boosts with. A combustion flame is
    /// turbulent and orange because it is burning; an ion plume is a
    /// collimated jet of charged gas, so it runs blue-white at the throat,
    /// fades violet, spreads far less and travels much faster. Same particle
    /// system, opposite character.
    pub fn ion_trail(&mut self, tail: Vec3, back: Vec3, car_vel: Vec3) {
        for k in 0..3 {
            let core = k == 0;
            let pos = tail + self.jitter(if core { 0.012 } else { 0.035 });
            // collimated: nearly all the speed is along the nozzle axis
            let vel = car_vel * 0.20 + back * self.rng.range(7.0, 11.0) + self.jitter(0.18);
            let max = self.rng.range(0.10, 0.20);
            let size = if core { self.rng.range(0.030, 0.055) } else { self.rng.range(0.055, 0.095) };
            let cool = self.rng.f();
            let tint = if core {
                [0.92, 0.98, 1.0, 1.0]
            } else if cool > 0.45 {
                [0.35, 0.80, 1.0, 1.0]
            } else {
                [0.55, 0.42, 1.0, 1.0]
            };
            self.push(Particle {
                pos,
                vel,
                life: 0.0,
                max,
                size,
                grow: 0.10,
                // white-hot throat, cyan body, violet as it cools
                color: tint,
                emissive: 1.0,
                grav: 0.0,
                drag: 1.2,
            });
        }
    }

    /// The garage's boost trails. Each is a different physical story rather
    /// than a recolour: what the exhaust IS decides how it moves.
    pub fn trail_of(&mut self, kind: usize, tail: Vec3, back: Vec3, car_vel: Vec3) {
        match kind {
            1 => self.ion_trail(tail, back, car_vel),
            2 => {
                // RIBBON: a flat sheet, nearly stationary once laid down, so
                // it hangs in the air and marks the line you drove
                for k in 0..2 {
                    let side = if k == 0 { 1.0 } else { -1.0 };
                    let pos = tail + Vec3::new(side * 0.11, 0.0, 0.0) + self.jitter(0.01);
                    let __r0 = self.rng.range(0.55, 0.85);
                    let __r1 = self.rng.range(0.10, 0.15);
                    self.push(Particle {
                        pos,
                        vel: car_vel * 0.04,
                        life: 0.0,
                        max: __r0,
                        size: __r1,
                        grow: -0.08,
                        color: [0.55, 0.85, 1.0, 1.0],
                        emissive: 0.85,
                        grav: 0.0,
                        drag: 0.4,
                    });
                }
            }
            3 => {
                // EMBER: sparks with real weight — they fall and go out
                for _ in 0..3 {
                    let pos = tail + self.jitter(0.05);
                    let vel = car_vel * 0.3 + back * self.rng.range(1.5, 3.5) + self.jitter(1.6);
                    let __r0 = self.rng.range(0.45, 0.9);
                    let __r1 = self.rng.range(0.02, 0.045);
                    self.push(Particle {
                        pos,
                        vel,
                        life: 0.0,
                        max: __r0,
                        size: __r1,
                        grow: -0.2,
                        color: [1.0, 0.62, 0.18, 1.0],
                        emissive: 1.0,
                        grav: 5.5,
                        drag: 0.9,
                    });
                }
            }
            4 => {
                // VOID: dark, slow, and it drinks the light — emissive zero
                // with a near-black colour reads as a hole in the scene
                for _ in 0..2 {
                    let pos = tail + self.jitter(0.09);
                    let __r0 = self.rng.range(0.5, 1.4);
                    let __r1 = self.rng.range(0.5, 0.9);
                    let __r2 = self.rng.range(0.10, 0.20);
                    let __j0 = self.jitter(0.5);
                    self.push(Particle {
                        pos,
                        vel: car_vel * 0.12 + back * __r0 + __j0,
                        life: 0.0,
                        max: __r1,
                        size: __r2,
                        grow: 0.5,
                        color: [0.04, 0.02, 0.07, 1.0],
                        emissive: 0.0,
                        grav: -0.6,
                        drag: 1.6,
                    });
                }
            }
            5 => {
                // PRISM: shards, each carrying one part of the spectrum, so
                // the trail separates into colours as it spreads
                for k in 0..3 {
                    let hue = [[1.0, 0.25, 0.35, 1.0], [0.30, 1.0, 0.45, 1.0], [0.35, 0.45, 1.0, 1.0]][k];
                    let pos = tail + self.jitter(0.04);
                    let spread = 1.2 + k as f32 * 0.5;
                    let __r0 = self.rng.range(3.0, 5.0);
                    let __r1 = self.rng.range(0.25, 0.45);
                    let __r2 = self.rng.range(0.035, 0.07);
                    let __j0 = self.jitter(spread);
                    self.push(Particle {
                        pos,
                        vel: car_vel * 0.2 + back * __r0 + __j0,
                        life: 0.0,
                        max: __r1,
                        size: __r2,
                        grow: -0.1,
                        color: hue,
                        emissive: 1.0,
                        grav: 0.0,
                        drag: 1.1,
                    });
                }
            }
            _ => self.boost_trail(tail, back, car_vel),
        }
    }

    /// Goal explosions. Same rule: each has its own behaviour, not its own
    /// palette — a shockwave expands, a singularity collapses first.
    pub fn explosion_of(&mut self, kind: usize, at: Vec3) {
        match kind {
            1 => {
                // SUPERNOVA: a hard flash, then a slow shell
                for _ in 0..40 {
                    let d = self.jitter(1.0).normalize_or(Vec3::Y);
                    let __r0 = self.rng.range(14.0, 22.0);
                    self.push(Particle { pos: at, vel: d * __r0, life: 0.0, max: 0.35, size: 0.5, grow: -0.5, color: [1.0, 0.97, 0.85, 1.0], emissive: 1.0, grav: 0.0, drag: 6.0 });
                }
                for _ in 0..60 {
                    let d = self.jitter(1.0).normalize_or(Vec3::Y);
                    let __r0 = self.rng.range(4.0, 8.0);
                    self.push(Particle { pos: at, vel: d * __r0, life: 0.0, max: 1.6, size: 0.3, grow: 0.4, color: [1.0, 0.55, 0.20, 1.0], emissive: 0.9, grav: 1.5, drag: 0.7 });
                }
            }
            2 => {
                // HEXBURST: a lattice — particles on a ring of rings
                for i in 0..7 {
                    for k in 0..12 {
                        let a = k as f32 / 12.0 * std::f32::consts::TAU;
                        let tilt = (i as f32 / 6.0 - 0.5) * 1.5;
                        let d = Vec3::new(a.cos(), tilt, a.sin()).normalize();
                        self.push(Particle { pos: at, vel: d * (9.0 + i as f32 * 1.5), life: 0.0, max: 1.1, size: 0.16, grow: 0.1, color: [0.35, 1.0, 0.85, 1.0], emissive: 1.0, grav: 0.0, drag: 1.4 });
                    }
                }
            }
            3 => {
                // INKFALL: plumes that rise, then rain back down
                for _ in 0..70 {
                    let d = (self.jitter(1.0) + Vec3::Y * 1.6).normalize_or(Vec3::Y);
                    let __r0 = self.rng.range(6.0, 13.0);
                    let __r1 = self.rng.range(0.2, 0.45);
                    self.push(Particle { pos: at, vel: d * __r0, life: 0.0, max: 2.2, size: __r1, grow: 0.5, color: [0.10, 0.05, 0.16, 1.0], emissive: 0.05, grav: 7.0, drag: 0.9 });
                }
            }
            4 => {
                // PRISMATIC: the spectrum thrown in every direction
                for k in 0..96 {
                    let d = self.jitter(1.0).normalize_or(Vec3::Y);
                    let h = (k % 6) as f32 / 6.0;
                    let col = [0.5 + 0.5 * (h * 6.28).sin(), 0.5 + 0.5 * (h * 6.28 + 2.1).sin(), 0.5 + 0.5 * (h * 6.28 + 4.2).sin(), 1.0];
                    let __r0 = self.rng.range(7.0, 18.0);
                    self.push(Particle { pos: at, vel: d * __r0, life: 0.0, max: 1.4, size: 0.14, grow: 0.0, color: col, emissive: 1.0, grav: 0.0, drag: 1.0 });
                }
            }
            5 => {
                // SINGULARITY: everything rushes IN first — particles are
                // spawned out wide travelling inward, and the outward burst
                // only follows once they have collapsed
                for _ in 0..80 {
                    let d = self.jitter(1.0).normalize_or(Vec3::Y);
                    let r = self.rng.range(6.0, 11.0);
                    let __r0 = self.rng.range(9.0, 15.0);
                    self.push(Particle { pos: at + d * r, vel: -d * __r0, life: 0.0, max: 0.75, size: 0.12, grow: -0.05, color: [0.65, 0.35, 1.0, 1.0], emissive: 1.0, grav: 0.0, drag: 0.2 });
                }
                for _ in 0..30 {
                    let d = self.jitter(1.0).normalize_or(Vec3::Y);
                    self.push(Particle { pos: at, vel: d * 3.0, life: -0.75, max: 1.5, size: 0.3, grow: 0.6, color: [0.95, 0.90, 1.0, 1.0], emissive: 1.0, grav: 0.0, drag: 1.2 });
                }
            }
            _ => {
                // SHOCKWAVE: one clean expanding ring
                for k in 0..80 {
                    let a = k as f32 / 80.0 * std::f32::consts::TAU;
                    let d = Vec3::new(a.cos(), 0.06, a.sin());
                    self.push(Particle { pos: at, vel: d * 16.0, life: 0.0, max: 1.0, size: 0.22, grow: 0.35, color: [0.55, 0.95, 0.65, 1.0], emissive: 1.0, grav: 0.0, drag: 2.2 });
                }
            }
        }
    }

    pub fn boost_trail(&mut self, tail: Vec3, back: Vec3, car_vel: Vec3) {
        for _ in 0..2 {
            let hot = self.rng.f();
            let pos = tail + self.jitter(0.05);
            let vel = car_vel * 0.25 + back * self.rng.range(2.5, 4.5) + self.jitter(0.8);
            let max = self.rng.range(0.18, 0.34);
            let size = self.rng.range(0.05, 0.10);
            self.push(Particle {
                pos,
                vel,
                life: 0.0,
                max,
                size,
                grow: 0.25,
                color: if hot > 0.5 { [1.0, 0.72, 0.15, 1.0] } else { [1.0, 0.40, 0.05, 1.0] },
                emissive: 1.0,
                grav: 0.0,
                drag: 3.0,
            });
        }
    }

    /// Confetti burst in the ring's own color when something flies through.
    pub fn ring_burst(&mut self, pos: Vec3, color: [f32; 4], big: bool) {
        let n = if big { 42 } else { 22 };
        for _ in 0..n {
            let d = self.jitter(1.0).normalize_or(Vec3::Y);
            let p = pos + d * self.rng.range(0.2, 1.4);
            let vel = d * self.rng.range(3.0, 9.0) + Vec3::Y * 2.0;
            let max = self.rng.range(0.5, 1.1);
            let size = self.rng.range(0.06, 0.14);
            self.push(Particle {
                pos: p,
                vel,
                life: 0.0,
                max,
                size,
                grow: 0.4,
                color,
                emissive: 0.9,
                grav: 3.5,
                drag: 1.2,
            });
        }
    }

    /// Dust puff along a surface normal (ball bounces, car landings).
    pub fn dust(&mut self, pos: Vec3, n: Vec3, strength: f32) {
        let cnt = (strength * 1.6).clamp(2.0, 12.0) as usize;
        for _ in 0..cnt {
            let j = self.jitter(1.0);
            let t = (j - n * j.dot(n)).normalize_or(Vec3::X);
            let vel = t * self.rng.range(0.6, 2.2) + n * self.rng.range(0.4, 1.4);
            let max = self.rng.range(0.35, 0.7);
            let size = self.rng.range(0.08, 0.16);
            self.push(Particle {
                pos: pos + n * 0.05,
                vel,
                life: 0.0,
                max,
                size,
                grow: 2.4,
                color: [0.82, 0.78, 0.70, 0.55],
                emissive: 0.0,
                grav: 0.6,
                drag: 2.4,
            });
        }
    }

    /// Golden sparkle when a boost pad is taken.
    pub fn pad_sparkle(&mut self, pos: Vec3, big: bool) {
        let n = if big { 26 } else { 10 };
        for _ in 0..n {
            let d = self.jitter(1.0);
            let vel = Vec3::new(d.x * 2.0, self.rng.range(2.0, 5.5), d.z * 2.0);
            let max = self.rng.range(0.3, 0.7);
            let size = self.rng.range(0.04, 0.09);
            self.push(Particle {
                pos: pos + Vec3::Y * 0.3,
                vel,
                life: 0.0,
                max,
                size,
                grow: 0.3,
                color: [1.0, 0.85, 0.25, 1.0],
                emissive: 1.0,
                grav: 4.0,
                drag: 0.8,
            });
        }
    }

    pub fn update(&mut self, dt: f32) {
        let mut i = 0;
        while i < self.parts.len() {
            let p = &mut self.parts[i];
            p.life += dt;
            if p.life >= p.max {
                let last = self.parts.len() - 1;
                self.parts.swap(i, last);
                self.parts.pop();
                if self.head >= self.parts.len() && !self.parts.is_empty() {
                    self.head = 0;
                }
                continue;
            }
            p.vel.y -= p.grav * dt;
            let dr = (-p.drag * dt).exp();
            p.vel *= dr;
            let v = p.vel;
            p.pos += v * dt;
            i += 1;
        }
    }

    pub fn draw(&self, objs: &mut Vec<ObjDraw>) {
        for p in &self.parts {
            let u = (p.life / p.max).clamp(0.0, 1.0);
            let s = p.size * (1.0 + (p.grow - 1.0) * u);
            let fade = 1.0 - u * u;
            let m = Mat4::from_translation(p.pos) * Mat4::from_scale(Vec3::splat(s.max(0.005)));
            let nrm3 = Mat3::IDENTITY;
            let mut raw = ObjectRaw::default();
            raw.model = m.to_cols_array();
            raw.nrm = Mat4::from_mat3(nrm3).to_cols_array();
            raw.color = [p.color[0], p.color[1], p.color[2], p.color[3] * fade];
            raw.params = [0.0, p.emissive * fade, 0.1, 0.0];
            objs.push(ObjDraw { mesh: M_SPHERE, tex: None, raw });
        }
    }
}
