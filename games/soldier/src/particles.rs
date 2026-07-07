//! particles.rs — the CPU billboard particle system from the GLM-5 `shooter_particles.rs`. Every
//! particle is a single quad that always faces the camera (built from `cam_right`/`cam_up`), so there
//! are no sprites or textures — just lit/emissive flat quads, exactly like the era. Muzzle flashes and
//! plasma bolts glow (emissive), blood and smoke and ejected brass are lit normally. To keep the frame
//! cheap we bake every live particle into just two meshes: one emissive batch and one diffuse batch,
//! each using per-vertex colour (`vcol`) so a single mesh can hold particles of many tints.

use crate::level::Rng;
use crate::math::Vec3;
use crate::mesh::{lerp_c, rgb, Mesh3};
use egui::Color32;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    MuzzleFlash,
    Spark,
    Blood,
    Smoke,
    Shell,
    Plasma,
    /// Wind-blown sand — unlike Smoke, this fades UP toward a bright sunlit tan instead of down toward
    /// black, so a sky full of dust reads as a bright haze, not a swarm of dark specks.
    Dust,
    /// A droplet thrown up when a body plunges into (or wades through) water — an arc of bright blue-white
    /// beads that fall back under gravity. Fades toward a pale foam colour.
    Splash,
    /// A glowing ember rising from a fire or explosion — hot, flickering, drifts up and cools from bright
    /// orange to dark as it fades. Emissive.
    Ember,
    /// A tongue of flame: a fat, bright, short-lived emissive puff rising fast, fading from white-hot to
    /// deep red. The core of a sustained fire plume (with embers + smoke layered over it).
    Flame,
    /// A jetpack ion-thruster particle: a bright cyan-white spark blasted downward, fading to deep blue. The
    /// visible exhaust of the ion drive — emissive, fast, short-lived.
    Ion,
}

impl Kind {
    fn emissive(self) -> bool {
        matches!(self, Kind::MuzzleFlash | Kind::Spark | Kind::Plasma | Kind::Ember | Kind::Flame | Kind::Ion)
    }
}

#[derive(Clone, Copy)]
struct Particle {
    kind: Kind,
    pos: Vec3,
    vel: Vec3,
    life: f32,
    max_life: f32,
    size0: f32,
    size1: f32,
    col0: Color32,
    col1: Color32,
    gravity: f32,
    drag: f32,
    /// Spin shells/smoke a touch as they age (purely cosmetic size flicker).
    flicker: f32,
}

impl Particle {
    #[inline]
    fn age(&self) -> f32 {
        1.0 - (self.life / self.max_life).clamp(0.0, 1.0)
    }
}

pub struct Particles {
    items: Vec<Particle>,
    rng: Rng,
    cap: usize,
}

impl Particles {
    pub fn new(seed: u64) -> Self {
        Self { items: Vec::with_capacity(512), rng: Rng::new(seed ^ 0x9E37_79B9_7F4A_7C15), cap: 2200 }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Iterate the world positions of all live particles (for the liveliness harness).
    pub fn positions(&self) -> impl Iterator<Item = crate::math::Vec3> + '_ {
        self.items.iter().map(|p| p.pos)
    }

    fn push(&mut self, p: Particle) {
        if self.items.len() < self.cap {
            self.items.push(p);
        }
    }

    /// Set the particle budget (the graphics preset scales this — Low caps low for framerate, Ultra high).
    pub fn set_cap(&mut self, cap: usize) {
        self.cap = cap.clamp(200, 8000);
    }

    #[inline]
    fn rand_dir(&mut self, base: Vec3, spread: f32) -> Vec3 {
        let j = Vec3::new(
            (self.rng.f32() - 0.5) * 2.0,
            (self.rng.f32() - 0.5) * 2.0,
            (self.rng.f32() - 0.5) * 2.0,
        );
        (base + j * spread).normalized()
    }

    /// A short, bright muzzle flash plus a few hot sparks and a wisp of smoke at the barrel.
    pub fn muzzle_flash(&mut self, pos: Vec3, dir: Vec3, color: Color32) {
        // The core flash: a couple of big, very short-lived emissive puffs.
        for i in 0..3 {
            let d = self.rand_dir(dir, 0.25);
            self.push(Particle {
                kind: Kind::MuzzleFlash,
                pos: pos + d * (0.1 * i as f32),
                vel: d * 4.0,
                life: 0.06,
                max_life: 0.06,
                size0: 0.32 - 0.05 * i as f32,
                size1: 0.05,
                col0: lerp_c(color, rgb(255, 255, 255), 0.5),
                col1: color,
                gravity: 0.0,
                drag: 8.0,
                flicker: 0.0,
            });
        }
        // A spray of sparks shooting forward.
        for _ in 0..7 {
            let d = self.rand_dir(dir, 0.5);
            let speed = 7.0 + self.rng.f32() * 9.0;
            let life = 0.18 + self.rng.f32() * 0.18;
            self.push(Particle {
                kind: Kind::Spark,
                pos,
                vel: d * speed,
                life,
                max_life: 0.36,
                size0: 0.05,
                size1: 0.012,
                col0: rgb(255, 240, 180),
                col1: rgb(255, 120, 40),
                gravity: 9.0,
                drag: 2.0,
                flicker: 0.0,
            });
        }
    }

    /// Sparks bouncing off a wall/crate at an impact point with a surface normal.
    pub fn impact_sparks(&mut self, pos: Vec3, normal: Vec3) {
        for _ in 0..10 {
            let d = self.rand_dir(normal, 0.9);
            let speed = 4.0 + self.rng.f32() * 8.0;
            let life = 0.16 + self.rng.f32() * 0.22;
            self.push(Particle {
                kind: Kind::Spark,
                pos: pos + normal * 0.02,
                vel: d * speed,
                life,
                max_life: 0.38,
                size0: 0.045,
                size1: 0.01,
                col0: rgb(255, 235, 170),
                col1: rgb(220, 90, 30),
                gravity: 12.0,
                drag: 1.5,
                flicker: 0.0,
            });
        }
        // A small puff of dust/smoke.
        self.smoke_puff(pos + normal * 0.05, 2);
    }

    /// A red blood mist when an enemy is hit, thrown along the shot direction.
    pub fn blood(&mut self, pos: Vec3, dir: Vec3) {
        for _ in 0..12 {
            let d = self.rand_dir(dir, 0.7);
            let speed = 2.5 + self.rng.f32() * 6.0;
            let life = 0.4 + self.rng.f32() * 0.4;
            self.push(Particle {
                kind: Kind::Blood,
                pos,
                vel: d * speed,
                life,
                max_life: 0.8,
                size0: 0.12,
                size1: 0.02,
                col0: rgb(170, 22, 22),
                col1: rgb(70, 8, 8),
                gravity: 14.0,
                drag: 1.0,
                flicker: 0.0,
            });
        }
    }

    /// A rising, expanding, fading smoke puff.
    /// Footstep / landing dust: a small, low, short-lived scuff of dust — much smaller than the general
    /// `smoke_puff` (which is sized for cannons and explosions) so feet kick up a scuff, not a smoke bomb.
    pub fn foot_puff(&mut self, pos: Vec3, count: usize) {
        for _ in 0..count {
            let d = self.rand_dir(Vec3::UP, 0.8);
            let life = 0.22 + self.rng.f32() * 0.2;
            self.push(Particle {
                kind: Kind::Smoke,
                pos,
                vel: d * 0.7 + Vec3::new(0.0, 0.25, 0.0),
                life,
                max_life: 0.42,
                size0: 0.06,
                size1: 0.2,
                col0: rgb(150, 140, 124),
                col1: rgb(96, 88, 78),
                gravity: -0.2,
                drag: 3.4,
                flicker: 0.0,
            });
        }
    }

    pub fn smoke_puff(&mut self, pos: Vec3, count: usize) {
        for _ in 0..count {
            let d = self.rand_dir(Vec3::UP, 0.6);
            let life = 0.5 + self.rng.f32() * 0.5;
            self.push(Particle {
                kind: Kind::Smoke,
                pos,
                vel: d * 1.2 + Vec3::new(0.0, 0.6, 0.0),
                life,
                max_life: 1.0,
                size0: 0.18,
                size1: 0.6,
                col0: rgb(120, 120, 128),
                col1: rgb(40, 40, 46),
                gravity: -0.6, // floats up
                drag: 2.5,
                flicker: 0.0,
            });
        }
    }

    /// Wind-blown sand for the sandstorm: soft, sparse, sunlit tan wisps drifting mostly sideways along
    /// `wind`. Deliberately larger and dimmer than smoke, and few in number — a haze in the middle distance,
    /// not a cloud of dark quads stacked in front of the camera.
    pub fn dust_puff(&mut self, pos: Vec3, wind: Vec3, count: usize) {
        for _ in 0..count {
            let d = self.rand_dir(Vec3::UP, 0.35);
            let life = 1.6 + self.rng.f32() * 1.2;
            let speed = 1.4 + self.rng.f32() * 0.8;
            self.push(Particle {
                kind: Kind::Dust,
                pos,
                vel: wind * speed + d * 0.4,
                life,
                max_life: life,
                size0: 0.5,
                size1: 1.8,
                col0: rgb(212, 190, 148),
                col1: rgb(196, 172, 128),
                gravity: 0.0,
                drag: 0.6,
                flicker: 0.0,
            });
        }
    }

    /// Wind-blown sand: low, fast grains that skitter across the ground along the wind, staying near the
    /// surface (unlike dust_puff's slow rising cloud). Spawned continuously across the play area on sandy maps
    /// so the desert floor reads as *alive* — a faint, restless drift that pricks the eye without obscuring it.
    pub fn sand_drift(&mut self, pos: Vec3, wind: Vec3, count: usize) {
        for _ in 0..count {
            let r_life = self.rng.f32();
            let r_speed = self.rng.f32();
            let r_up = self.rng.f32();
            let r_jx = self.rng.f32();
            let r_jz = self.rng.f32();
            let r_y = self.rng.f32();
            let life = 0.7 + r_life * 0.8;
            let speed = 3.0 + r_speed * 3.5;
            let up = r_up * 0.35;
            self.push(Particle {
                kind: Kind::Dust,
                pos: pos + Vec3::new(0.0, 0.05 + r_y * 0.25, 0.0),
                vel: wind * speed + Vec3::new((r_jx - 0.5) * 0.6, up, (r_jz - 0.5) * 0.6),
                life,
                max_life: life,
                size0: 0.12,
                size1: 0.5,
                col0: rgb(220, 200, 160),
                col1: rgb(200, 180, 140),
                gravity: 1.5,
                drag: 0.4,
                flicker: 0.0,
            });
        }
    }

    /// A water splash: a crown of droplets flung up and out when a body plunges into or wades through water.
    /// `strength` (roughly the impact speed) scales the count, spread and height so a dive throws a big crown
    /// and a wade kicks up a few beads. Droplets arc under gravity and fade from bright to pale foam.
    pub fn splash(&mut self, pos: Vec3, strength: f32, col: Color32) {
        let s = strength.clamp(0.5, 12.0);
        let count = (4.0 + s * 2.2) as usize;
        for _ in 0..count {
            // mostly-up cone, wider as the impact gets harder
            let spread = (0.35 + s * 0.05).min(0.9);
            let d = self.rand_dir(Vec3::UP, spread);
            let rand_speed = self.rng.f32();
            let speed = 1.5 + s * 0.55 + rand_speed * s * 0.4;
            let life = 0.4 + self.rng.f32() * 0.5;
            let jx = (self.rng.f32() - 0.5) * 0.3;
            let jz = (self.rng.f32() - 0.5) * 0.3;
            let sz0 = 0.06 + self.rng.f32() * 0.04;
            self.push(Particle {
                kind: Kind::Splash,
                pos: pos + Vec3::new(jx, 0.05, jz),
                vel: d * speed,
                life,
                max_life: life,
                size0: sz0,
                size1: 0.02,
                col0: lerp_c(col, rgb(235, 245, 250), 0.6),
                col1: rgb(210, 228, 235),
                gravity: 16.0,
                drag: 1.2,
                flicker: 0.0,
            });
        }
    }

    /// A burst of rising, flickering embers from a fire or explosion. Hot orange, drifting up and cooling as
    /// they fade — emissive, so they glow against the scene. `count` scales with the blast size.
    pub fn embers(&mut self, pos: Vec3, count: usize) {
        for _ in 0..count {
            let d = self.rand_dir(Vec3::UP, 0.5);
            let up_speed = 1.5 + self.rng.f32() * 3.5;
            let life = 0.7 + self.rng.f32() * 1.1;
            let jx = (self.rng.f32() - 0.5) * 0.5;
            let jz = (self.rng.f32() - 0.5) * 0.5;
            let sz0 = 0.05 + self.rng.f32() * 0.05;
            self.push(Particle {
                kind: Kind::Ember,
                pos: pos + Vec3::new(jx, 0.1, jz),
                vel: Vec3::new(d.x * 0.8, up_speed, d.z * 0.8),
                life,
                max_life: life,
                size0: sz0,
                size1: 0.01,
                col0: rgb(255, 200, 90),
                col1: rgb(140, 40, 20),
                gravity: -1.2, // slight buoyant rise (hot air)
                drag: 0.8,
                flicker: 1.0, // twinkle
            });
        }
    }

    /// A tongue of flame rising from a fire source: fat, bright, short-lived puffs that shoot up and fade
    /// from white-hot through orange to deep red. Layer with `embers` and `smoke_puff` for a full plume.
    pub fn flame(&mut self, pos: Vec3, count: usize) {
        for _ in 0..count {
            let jx = (self.rng.f32() - 0.5) * 0.5;
            let jz = (self.rng.f32() - 0.5) * 0.5;
            let up = 2.4 + self.rng.f32() * 2.6;
            let life = 0.35 + self.rng.f32() * 0.35;
            let sz0 = 0.22 + self.rng.f32() * 0.16;
            self.push(Particle {
                kind: Kind::Flame,
                pos: pos + Vec3::new(jx, 0.05, jz),
                vel: Vec3::new(jx * 0.6, up, jz * 0.6),
                life,
                max_life: life,
                size0: sz0,
                size1: 0.06,
                col0: rgb(255, 240, 180), // white-hot core
                col1: rgb(200, 50, 20),   // deep red as it cools
                gravity: -3.0,            // strong buoyant rise
                drag: 1.4,
                flicker: 1.0,
            });
        }
    }

    pub fn eject_shell(&mut self, pos: Vec3, right: Vec3, up: Vec3) {
        let d = (right * 1.0 + up * 0.6 + self.rand_dir(Vec3::UP, 1.0) * 0.2).normalized();
        let sp = 2.6 + self.rng.f32() * 1.5;
        self.push(Particle {
            kind: Kind::Shell,
            pos,
            vel: d * sp,
            life: 0.9,
            max_life: 0.9,
            size0: 0.05,
            size1: 0.05,
            col0: rgb(220, 180, 90),
            col1: rgb(150, 110, 50),
            gravity: 16.0,
            drag: 0.4,
            flicker: 0.5,
        });
    }

    /// A glowing plasma impact splash.
    pub fn plasma_splash(&mut self, pos: Vec3, normal: Vec3, color: Color32) {
        for _ in 0..9 {
            let d = self.rand_dir(normal, 1.0);
            let sp = 3.0 + self.rng.f32() * 6.0;
            let life = 0.2 + self.rng.f32() * 0.2;
            self.push(Particle {
                kind: Kind::Plasma,
                pos: pos + normal * 0.02,
                vel: d * sp,
                life,
                max_life: 0.4,
                size0: 0.14,
                size1: 0.02,
                col0: lerp_c(color, rgb(255, 255, 255), 0.4),
                col1: color,
                gravity: 2.0,
                drag: 3.0,
                flicker: 0.0,
            });
        }
    }

    /// Jetpack ion exhaust: a downward blast of bright cyan-white ion particles from a thruster port, fading to
    /// deep blue. Emit a few per frame while the jetpack fires for a continuous plume. `port` is the world
    /// position of the nozzle; the plume shoots down and back.
    pub fn ion_thrust(&mut self, port: Vec3, back: Vec3) {
        for _ in 0..4 {
            // mostly downward, with a little backward spread and jitter
            let down = Vec3::new(0.0, -1.0, 0.0);
            let dir = (down * 1.6 + back * 0.5 + self.rand_dir(down, 0.4) * 0.4).normalized();
            let sp = 7.0 + self.rng.f32() * 6.0;
            let life = 0.18 + self.rng.f32() * 0.16;
            self.push(Particle {
                kind: Kind::Ion,
                pos: port,
                vel: dir * sp,
                life,
                max_life: 0.34,
                size0: 0.13,
                size1: 0.02,
                col0: rgb(210, 245, 255), // hot cyan-white core
                col1: rgb(40, 90, 220),   // fades to deep ion blue
                gravity: -1.5,            // slight upward buoyancy as it cools (ionized wash)
                drag: 3.5,
                flicker: 0.4,
            });
        }
    }

    /// Integrate every particle and drop the dead ones.
    pub fn tick(&mut self, floor_y: f32, dt: f32) {
        for p in &mut self.items {
            p.life -= dt;
            p.vel.y -= p.gravity * dt;
            // Exponential-ish drag.
            let damp = (1.0 - p.drag * dt).clamp(0.0, 1.0);
            p.vel = p.vel * damp;
            p.pos = p.pos + p.vel * dt;
            // Shells/blood settle on the floor instead of sinking through it.
            if (p.kind == Kind::Shell || p.kind == Kind::Blood) && p.pos.y < floor_y + 0.03 {
                p.pos.y = floor_y + 0.03;
                p.vel.y = -p.vel.y * 0.25;
                p.vel.x *= 0.6;
                p.vel.z *= 0.6;
            }
        }
        self.items.retain(|p| p.life > 0.0);
    }

    /// Bake all live particles into two batched meshes: `[emissive, diffuse]` (either may be empty).
    pub fn build(&self, cam_right: Vec3, cam_up: Vec3) -> Vec<Mesh3> {
        let mut emis = Mesh3::new(Color32::WHITE);
        emis.vcol = Some(Vec::new());
        emis.emissive = 0.85;
        let mut diff = Mesh3::new(Color32::WHITE);
        diff.vcol = Some(Vec::new());
        diff.emissive = 0.0;

        let n = cam_right.cross(cam_up).normalized();
        for p in &self.items {
            let a = p.age();
            let size = lerpf(p.size0, p.size1, a) * (1.0 + p.flicker * (a * 12.0).sin() * 0.15);
            let col = lerp_c(p.col0, p.col1, a);
            // Fade out near end of life (we have no alpha in the flat pipeline): smoke/blood/shells darken
            // toward near-black as they thin, matching a dark particle dissolving against most backdrops.
            // Dust is the opposite — sunlit sand haze — so it fades UP toward a bright warm tan instead;
            // fading a light particle toward black would turn a dust storm into a field of dark specks.
            let fade = (p.life / p.max_life).clamp(0.0, 1.0);
            let fade_target = if p.kind == Kind::Dust { rgb(224, 208, 172) } else { rgb(8, 8, 10) };
            let col = lerp_c(fade_target, col, fade.powf(0.5));

            let target = if p.kind.emissive() { &mut emis } else { &mut diff };
            let r = cam_right * size;
            let u = cam_up * size;
            let c = p.pos;
            target.push_quad(c - r - u, c + r - u, c + r + u, c - r + u, n, col);
        }

        let mut out = Vec::new();
        if !emis.tris.is_empty() {
            out.push(emis);
        }
        if !diff.tris.is_empty() {
            out.push(diff);
        }
        out
    }
}

#[inline]
fn lerpf(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emitting_then_ticking_eventually_clears_everything() {
        let mut ps = Particles::new(1);
        ps.muzzle_flash(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), rgb(255, 230, 150));
        assert!(!ps.is_empty());
        for _ in 0..200 {
            ps.tick(0.0, 1.0 / 60.0);
        }
        assert!(ps.is_empty(), "all particles should have expired");
    }

    #[test]
    fn build_returns_batched_meshes_with_geometry() {
        let mut ps = Particles::new(2);
        ps.muzzle_flash(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), rgb(255, 230, 150));
        ps.blood(Vec3::new(1.0, 1.0, 1.0), Vec3::new(1.0, 0.0, 0.0));
        let meshes = ps.build(Vec3::new(1.0, 0.0, 0.0), Vec3::UP);
        assert_eq!(meshes.len(), 2, "expected an emissive and a diffuse batch");
        assert!(meshes.iter().all(|m| m.tri_count() > 0));
        // The emissive batch glows, the diffuse one does not.
        assert!(meshes[0].emissive > 0.5);
        assert_eq!(meshes[1].emissive, 0.0);
    }

    #[test]
    fn cap_limits_runaway_emission() {
        let mut ps = Particles::new(3);
        for _ in 0..5000 {
            ps.impact_sparks(Vec3::ZERO, Vec3::UP);
        }
        assert!(ps.len() <= 2200);
    }
}
