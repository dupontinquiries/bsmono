//! `sim::particles` — a small pooled particle system: spawn from emitter
//! presets, step (advect by velocity + gravity + wind, age), retire the dead,
//! and hand the renderer a flat buffer of positions/colors/sizes. The per-frame
//! look of each particle is driven entirely by the existing pure curves —
//! [`crate::particle_alpha_curve`], [`crate::particle_color_ramp`],
//! [`crate::particle_size_curve`], [`crate::world::smoke_field`] — and bursts are
//! seeded from [`crate::spark_burst`]. This module is just the *system* that owns
//! a bounded pool and ticks it.
//!
//! Pure & deterministic: spawn variation comes from a hashed `(seed, spawn#,
//! index)`, integration is plain FP32 arithmetic, and the pool is hard-capped, so
//! `(seed, emits, dt sequence)` fully determines the state. No clock/RNG/IO, no
//! `unwrap`/`expect`/`panic` in non-test code.

use crate::particle_alpha_curve;
use crate::particle_color_ramp;
use crate::particle_size_curve;
use crate::spark_burst;
use crate::world::smoke_field;

/// Default pool capacity if the caller doesn't pick one.
pub const DEFAULT_CAP: usize = 4096;

/// Spark lifetime, matched to `spark_burst`'s internal `LIFE` so the seeded
/// trajectory and the pool agree on when a spark dies.
const SPARK_LIFE: f32 = 1.5;

/// What a particle is — selects its physics response and look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParticleKind {
    Spark,
    Smoke,
    Dust,
}

/// One pooled particle. Fields are public for inspection/tests; the renderer
/// normally reads the flattened [`RenderBuffer`] instead.
#[derive(Clone, Copy, Debug)]
pub struct Particle {
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    pub age: f32,
    pub lifetime: f32,
    pub kind: ParticleKind,
    pub color0: [f32; 3],
    pub color1: [f32; 3],
    pub size0: f32,
    pub size1: f32,
    /// How strongly world gravity affects this particle (1 = full, <0 = buoyant).
    pub grav_scale: f32,
    /// Velocity damping per second (air resistance).
    pub drag: f32,
    /// How strongly wind pushes this particle.
    pub wind_scale: f32,
}

impl Particle {
    /// Alpha this frame, from the kind's curve (smoke uses the smoke-field fade).
    pub fn alpha(&self) -> f32 {
        match self.kind {
            ParticleKind::Smoke => smoke_field::density(self.age, 0.0, [0.0, 0.0]),
            _ => particle_alpha_curve::alpha(self.age, self.lifetime),
        }
    }
    pub fn color(&self) -> [f32; 3] {
        particle_color_ramp::color(self.age, self.lifetime, self.color0, self.color1)
    }
    pub fn size(&self) -> f32 {
        particle_size_curve::size(self.age, self.lifetime, self.size0, self.size1)
    }
}

/// A render-ready, flattened view of the live particles. `positions[i]`,
/// `colors[i]` (RGBA, alpha already from the curve) and `sizes[i]` describe one
/// billboard the renderer draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderBuffer {
    pub positions: Vec<[f32; 3]>,
    pub colors: Vec<[f32; 4]>,
    pub sizes: Vec<f32>,
}

impl RenderBuffer {
    pub fn len(&self) -> usize {
        self.positions.len()
    }
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }
}

/// Emitter presets — the three gameplay triggers from the brief.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Preset {
    /// Radial spark burst (e.g. a weapon hit).
    SparkBurst,
    /// Rising smoke puffs (e.g. from fire).
    Smoke,
    /// Low horizontal dust puff (e.g. a footstep).
    Dust,
}

// ── deterministic hashing for per-particle variation ────────────────────────
fn mix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
fn h01(seed: u64, a: u64, b: u64) -> f32 {
    let h = mix64(seed ^ a.wrapping_mul(0xD1B5_4A32_D192_ED03) ^ b.wrapping_mul(0x8589_33D3));
    (h >> 40) as f32 / ((1u64 << 24) as f32)
}

/// A bounded pool of particles that spawns from presets, steps, and retires.
pub struct ParticleSystem {
    cap: usize,
    seed: u64,
    spawn_count: u64,
    particles: Vec<Particle>,
}

impl ParticleSystem {
    pub fn new(cap: usize, seed: u64) -> ParticleSystem {
        ParticleSystem { cap: cap.max(1), seed, spawn_count: 0, particles: Vec::new() }
    }
    pub fn with_default_cap(seed: u64) -> ParticleSystem {
        ParticleSystem::new(DEFAULT_CAP, seed)
    }

    pub fn len(&self) -> usize {
        self.particles.len()
    }
    pub fn is_empty(&self) -> bool {
        self.particles.is_empty()
    }
    pub fn capacity(&self) -> usize {
        self.cap
    }
    pub fn particles(&self) -> &[Particle] {
        &self.particles
    }

    /// Spawn `count` particles of `preset` at `origin`. Spawning is clamped to
    /// the pool capacity (excess is dropped), so the pool never exceeds `cap`.
    /// Returns how many were actually spawned.
    pub fn emit(&mut self, preset: Preset, origin: [f32; 3], count: u32) -> usize {
        let room = self.cap.saturating_sub(self.particles.len());
        let n = (count as usize).min(room);
        let spawn = self.spawn_count;
        self.spawn_count = self.spawn_count.wrapping_add(1);
        for i in 0..n {
            let p = self.make(preset, origin, spawn, i as u64);
            self.particles.push(p);
        }
        n
    }

    fn make(&self, preset: Preset, origin: [f32; 3], spawn: u64, idx: u64) -> Particle {
        let s = self.seed ^ spawn.wrapping_mul(0x100_0001_b3);
        let jitter = h01(s, idx, 7); // 0..1 lifetime jitter
        match preset {
            Preset::SparkBurst => {
                // seed the initial velocity from spark_burst's radial model:
                // sampling its position at a tiny t gives ≈ direction*speed.
                let t0 = 0.05f32;
                let (p0, _a) = spark_burst::spark(idx as u32, t0, s);
                let vel = [p0[0] / t0, p0[1] / t0, p0[2] / t0];
                Particle {
                    pos: origin,
                    vel,
                    age: 0.0,
                    lifetime: SPARK_LIFE * (0.85 + 0.3 * jitter),
                    kind: ParticleKind::Spark,
                    color0: [1.0, 0.95, 0.7],
                    color1: [0.8, 0.2, 0.05],
                    size0: 0.25,
                    size1: 0.0,
                    grav_scale: 1.0,
                    drag: 0.5,
                    wind_scale: 0.1,
                }
            }
            Preset::Smoke => {
                let sx = (h01(s, idx, 1) - 0.5) * 0.3;
                let sz = (h01(s, idx, 2) - 0.5) * 0.3;
                let rise = 0.6 + 0.4 * h01(s, idx, 3);
                Particle {
                    pos: origin,
                    vel: [sx, rise, sz],
                    age: 0.0,
                    lifetime: 3.5 + 2.0 * jitter,
                    kind: ParticleKind::Smoke,
                    color0: [0.55, 0.55, 0.58],
                    color1: [0.14, 0.14, 0.17],
                    size0: 0.3,
                    size1: 2.2,
                    grav_scale: -0.05, // gently buoyant
                    drag: 0.6,
                    wind_scale: 1.0, // smoke drifts with wind
                }
            }
            Preset::Dust => {
                let ang = h01(s, idx, 1) * std::f32::consts::TAU;
                let spd = 0.8 + 0.8 * h01(s, idx, 2);
                Particle {
                    pos: origin,
                    vel: [ang.cos() * spd, 0.25, ang.sin() * spd],
                    age: 0.0,
                    lifetime: 0.6 + 0.3 * jitter,
                    kind: ParticleKind::Dust,
                    color0: [0.76, 0.70, 0.55],
                    color1: [0.58, 0.53, 0.43],
                    size0: 0.4,
                    size1: 0.1,
                    grav_scale: 1.0, // settles back down
                    drag: 2.0, // slows quickly
                    wind_scale: 0.3,
                }
            }
        }
    }

    /// Advance every particle by `dt` under `gravity` and `wind` (world m/s²,
    /// m/s), then retire any that reached end-of-life. The pool only shrinks or
    /// stays the same here, so it never exceeds `cap`.
    pub fn step(&mut self, dt: f32, gravity: [f32; 3], wind: [f32; 3]) {
        if dt <= 0.0 {
            return;
        }
        for p in &mut self.particles {
            p.age += dt;
            for k in 0..3 {
                p.vel[k] += gravity[k] * p.grav_scale * dt + wind[k] * p.wind_scale * dt;
            }
            let damp = (1.0 - p.drag * dt).max(0.0);
            for k in 0..3 {
                p.vel[k] *= damp;
                p.pos[k] += p.vel[k] * dt;
            }
        }
        self.particles.retain(|p| p.age < p.lifetime);
    }

    /// Flatten the live particles into a render-ready buffer (positions, RGBA
    /// colors with curve-driven alpha, sizes), one entry per live particle.
    pub fn render_buffer(&self) -> RenderBuffer {
        let n = self.particles.len();
        let mut out = RenderBuffer {
            positions: Vec::with_capacity(n),
            colors: Vec::with_capacity(n),
            sizes: Vec::with_capacity(n),
        };
        for p in &self.particles {
            let c = p.color();
            out.positions.push(p.pos);
            out.colors.push([c[0], c[1], c[2], p.alpha()]);
            out.sizes.push(p.size());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: [f32; 3] = [0.0, -9.8, 0.0];
    const NO_WIND: [f32; 3] = [0.0, 0.0, 0.0];

    #[test]
    fn emit_spawns_expected_count_and_kinds() {
        let mut ps = ParticleSystem::new(1000, 1);
        assert_eq!(ps.emit(Preset::SparkBurst, [0.0; 3], 20), 20);
        assert_eq!(ps.len(), 20);
        assert!(ps.particles().iter().all(|p| p.kind == ParticleKind::Spark));
        assert_eq!(ps.emit(Preset::Smoke, [0.0; 3], 8), 8);
        assert_eq!(ps.emit(Preset::Dust, [0.0; 3], 5), 5);
        assert_eq!(ps.len(), 33);
    }

    #[test]
    fn pool_never_exceeds_cap() {
        let mut ps = ParticleSystem::new(16, 42);
        // try to massively over-spawn
        for _ in 0..50 {
            ps.emit(Preset::SparkBurst, [0.0; 3], 100);
            assert!(ps.len() <= 16, "pool overflowed: {}", ps.len());
        }
        // a full pool accepts nothing more
        assert_eq!(ps.emit(Preset::Smoke, [0.0; 3], 10), 0);
        // stepping keeps it within cap
        for _ in 0..10 {
            ps.step(0.1, G, NO_WIND);
            assert!(ps.len() <= 16);
        }
    }

    #[test]
    fn particles_age_and_eventually_all_retire() {
        let mut ps = ParticleSystem::new(256, 7);
        ps.emit(Preset::Dust, [0.0; 3], 30); // dust lives < ~0.9s
        assert_eq!(ps.len(), 30);
        let a0 = ps.particles()[0].age;
        ps.step(0.1, G, NO_WIND);
        assert!(ps.particles().get(0).map_or(true, |p| p.age > a0));
        // step well past the longest dust lifetime
        for _ in 0..40 {
            ps.step(0.1, G, NO_WIND);
        }
        assert_eq!(ps.len(), 0, "all dust should have retired");
    }

    #[test]
    fn curves_drive_alpha_and_size_over_lifetime() {
        // one dust particle, sampled across its life via the render buffer.
        let mut ps = ParticleSystem::new(4, 3);
        ps.emit(Preset::Dust, [0.0; 3], 1);
        let life = ps.particles()[0].lifetime;
        // birth: alpha fades in (starts at 0)
        let birth = ps.render_buffer();
        assert!(birth.colors[0][3] < 0.2, "alpha starts low (fade-in)");
        let birth_size = birth.sizes[0];
        // step to mid-life
        ps.step(life * 0.5, G, NO_WIND);
        let mid = ps.render_buffer();
        assert!(mid.colors[0][3] > birth.colors[0][3], "alpha rises after birth");
        let mid_size = mid.sizes[0];
        // dust shrinks (size0 0.4 → size1 0.1), so size decreases over life
        assert!(mid_size < birth_size + 1e-6, "dust size should shrink: {birth_size}->{mid_size}");
        // near death, alpha falls again
        ps.step(life * 0.45, G, NO_WIND);
        let late = ps.render_buffer();
        if !late.colors.is_empty() {
            assert!(late.colors[0][3] < mid.colors[0][3], "alpha falls toward death");
        }
    }

    #[test]
    fn render_buffer_lengths_match_live_count() {
        let mut ps = ParticleSystem::new(256, 9);
        ps.emit(Preset::SparkBurst, [1.0, 2.0, 3.0], 12);
        ps.step(0.1, G, [1.0, 0.0, 0.0]);
        let rb = ps.render_buffer();
        assert_eq!(rb.positions.len(), ps.len());
        assert_eq!(rb.colors.len(), ps.len());
        assert_eq!(rb.sizes.len(), ps.len());
        // colors + sizes finite and in range
        for c in &rb.colors {
            assert!(c.iter().all(|v| v.is_finite()) && (0.0..=1.0).contains(&c[3]));
        }
        assert!(rb.sizes.iter().all(|s| s.is_finite() && *s >= 0.0));
    }

    #[test]
    fn spark_burst_velocities_are_radial_and_distinct() {
        let mut ps = ParticleSystem::new(64, 5);
        ps.emit(Preset::SparkBurst, [0.0; 3], 24);
        let speeds: Vec<f32> = ps
            .particles()
            .iter()
            .map(|p| (p.vel[0] * p.vel[0] + p.vel[1] * p.vel[1] + p.vel[2] * p.vel[2]).sqrt())
            .collect();
        assert!(speeds.iter().all(|&s| s > 0.5), "sparks should fly outward");
        // directions differ between sparks
        assert!(ps.particles()[0].vel != ps.particles()[1].vel);
    }

    #[test]
    fn deterministic_given_seed_and_dt() {
        let run = || {
            let mut ps = ParticleSystem::new(512, 123);
            ps.emit(Preset::SparkBurst, [0.0, 1.0, 0.0], 16);
            ps.emit(Preset::Smoke, [2.0, 0.0, 0.0], 8);
            for i in 0..20 {
                ps.step(0.05, G, [0.3, 0.0, -0.1]);
                if i == 10 {
                    ps.emit(Preset::Dust, [0.0, 0.0, 0.0], 6);
                }
            }
            ps.render_buffer()
        };
        assert_eq!(run(), run(), "same seed + dt sequence ⇒ identical buffer");
    }

    #[test]
    fn stays_finite_under_long_run() {
        let mut ps = ParticleSystem::new(2048, 77);
        for f in 0..200 {
            if f % 5 == 0 {
                ps.emit(Preset::Smoke, [0.0, 0.0, 0.0], 4);
            }
            if f % 13 == 0 {
                ps.emit(Preset::SparkBurst, [1.0, 1.0, 1.0], 10);
            }
            ps.step(0.033, G, [0.5, 0.0, 0.2]);
            assert!(ps.len() <= ps.capacity());
            for p in ps.particles() {
                assert!(p.pos.iter().all(|v| v.is_finite()) && p.vel.iter().all(|v| v.is_finite()));
            }
        }
    }
}
