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
