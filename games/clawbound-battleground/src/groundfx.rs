//! groundfx — what a big hit does to the world, and how the world recovers.
//!
//! State and arithmetic only; the four shaders that draw it read plain
//! arrays from here. Presentation, not simulation: nothing in this module
//! changes where anyone can walk or what anyone can see.
//!
//! * A [`Blast`] is a ring travelling outward. `voxel.wgsl` leans every
//!   swaying thing away from the ring as it passes — trees included, which
//!   is the difference from `puppet`'s brush, where only what is lighter
//!   than a cat gives way. `post.wgsl` bends the frame along the same ring,
//!   so the air is seen to move.
//! * A [`Crack`] is shattered ground that HEALS: slowly when dry, and more
//!   than ten times faster in rain, when the soil swells and closes. It
//!   narrows before it fades, and leaves darker, wetter earth for a while.
//! * [`fit_shadow`] aims the one shadow map at what the camera is looking at
//!   instead of at the whole arena, snapped to whole texels so shadows do
//!   not crawl as the camera moves.

use glam::{Mat4, Vec3, Vec4};

pub const MAX_BLASTS: usize = 4;
pub const MAX_CRACKS: usize = 8;
pub const MAX_RINGS: usize = 8;

/// How fast a shock front travels, u/s. Fast enough to read as a blast and
/// slow enough to watch it cross a clearing.
pub const BLAST_SPEED: f32 = 26.0;
/// Seconds for an open crack to close with no rain at all.
pub const DRY_HEAL_SECS: f32 = 240.0;
/// ...and in a downpour.
pub const RAIN_HEAL_SECS: f32 = 14.0;
/// A hit weaker than this shakes the trees and does not break the ground.
pub const CRACK_POWER: f32 = 0.45;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blast {
    pub pos: Vec3,
    pub age: f32,
    /// How far the front travels before it is spent.
    pub reach: f32,
    pub power: f32,
}

impl Blast {
    pub fn radius(&self) -> f32 {
        self.age * BLAST_SPEED
    }
    /// Falls off as it spreads: the same energy over a longer front.
    pub fn strength(&self) -> f32 {
        let k = (1.0 - self.radius() / self.reach.max(0.1)).clamp(0.0, 1.0);
        self.power * k * k
    }
    pub fn spent(&self) -> bool {
        self.radius() >= self.reach
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crack {
    pub x: f32,
    pub z: f32,
    pub radius: f32,
    /// 1 = just broken, 0 = healed.
    pub open: f32,
}

#[derive(Debug, Clone, Default)]
pub struct GroundFx {
    pub blasts: Vec<Blast>,
    pub cracks: Vec<Crack>,
}

impl GroundFx {
    /// Something hit the ground at `pos`. `radius` is the hit's own size in
    /// world units, `power` `0..=1`. Returns whether the ground broke, so the
    /// caller knows whether to throw dust.
    pub fn strike(&mut self, pos: Vec3, radius: f32, power: f32) -> bool {
        if !pos.is_finite() || !(radius > 0.0) || !(power > 0.0) {
            return false;
        }
        let power = power.min(1.0);
        if self.blasts.len() >= MAX_BLASTS {
            // Drop the most spent: a new blast matters more than the tail of an old one.
            if let Some(i) = (0..self.blasts.len()).min_by(|a, b| self.blasts[*a].strength().total_cmp(&self.blasts[*b].strength())) {
                self.blasts.swap_remove(i);
            }
        }
        self.blasts.push(Blast { pos, age: 0.0, reach: (radius * 4.0).clamp(8.0, 46.0), power });
        if power < CRACK_POWER {
            return false;
        }
        // Breaking ground that is already broken re-opens it rather than
        // stacking a second crack on the first.
        if let Some(c) = self.cracks.iter_mut().find(|c| (c.x - pos.x).hypot(c.z - pos.z) < c.radius * 0.6) {
            c.open = 1.0;
            c.radius = c.radius.max(radius * 1.3);
            return true;
        }
        if self.cracks.len() >= MAX_CRACKS {
            if let Some(i) = (0..self.cracks.len()).min_by(|a, b| self.cracks[*a].open.total_cmp(&self.cracks[*b].open)) {
                self.cracks.swap_remove(i);
            }
        }
        self.cracks.push(Crack { x: pos.x, z: pos.z, radius: (radius * 1.3).clamp(1.2, 12.0), open: 1.0 });
        true
    }

    /// `rain` is the weather's rain `0..=1`; `wetness` how soaked the ground
    /// still is after it stops, which keeps healing going for a while.
    pub fn tick(&mut self, dt: f32, rain: f32, wetness: f32) {
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.25) } else { 0.0 };
        for b in &mut self.blasts {
            b.age += dt;
        }
        self.blasts.retain(|b| !b.spent());
        let wet = finite01(rain).max(finite01(wetness) * 0.5);
        let rate = (1.0 - wet) / DRY_HEAL_SECS + wet / RAIN_HEAL_SECS;
        for c in &mut self.cracks {
            c.open = (c.open - rate * dt).max(0.0);
        }
        self.cracks.retain(|c| c.open > 0.0);
    }

    /// For `voxel.wgsl`: `a = (x, y, z, ring radius)`, `b = (strength, width, 0, 0)`.
    pub fn blast_uniform(&self) -> ([[f32; 4]; MAX_BLASTS], [[f32; 4]; MAX_BLASTS]) {
        let (mut a, mut b) = ([[0.0; 4]; MAX_BLASTS], [[0.0; 4]; MAX_BLASTS]);
        for (i, bl) in self.blasts.iter().take(MAX_BLASTS).enumerate() {
            a[i] = [bl.pos.x, bl.pos.y, bl.pos.z, bl.radius()];
            b[i] = [bl.strength(), 2.6, 0.0, 0.0];
        }
        (a, b)
    }

    /// For `terrain.wgsl`: `(x, z, radius, open)`.
    pub fn crack_uniform(&self) -> [[f32; 4]; MAX_CRACKS] {
        let mut out = [[0.0; 4]; MAX_CRACKS];
        for (i, c) in self.cracks.iter().take(MAX_CRACKS).enumerate() {
            out[i] = [c.x, c.z, c.radius, c.open];
        }
        out
    }
}

/// A blast's shock shell: its thickness for a front of `radius`.
pub fn shell_thickness(radius: f32) -> f32 {
    0.6 + radius * 0.12
}

/// How far along a ray one sphere of radius `r` about `c` lies, clipped to
/// [0, `t_max`]: the part in front of the camera and in front of the scene.
fn sphere_span(ro: Vec3, rd: Vec3, c: Vec3, r: f32, t_max: f32) -> f32 {
    let oc = ro - c;
    let b = oc.dot(rd);
    let disc = b * b - (oc.length_squared() - r * r);
    if disc <= 0.0 {
        return 0.0;
    }
    let h = disc.sqrt();
    let (t0, t1) = ((-b - h).max(0.0), (-b + h).min(t_max));
    (t1 - t0).max(0.0)
}

/// How much blast-front air a view ray crosses: the length of the ray
/// inside the shell between `radius - th/2` and `radius + th/2` about `c`,
/// counting only what lies in front of the scene surface (`t_scene` along
/// the ray; a large number for sky). `post.wgsl` computes exactly this per
/// pixel against the depth buffer.
///
/// What it changes from the old screen-space rings: a surface in front of
/// the blast occludes it (a cat between you and a nova is not warped); the
/// shell has perspective (a ground blast is a dome, cut by the ground); and
/// it is strongest where the ray grazes the shell -- its silhouette -- as
/// real refraction through a shell of denser air is.
pub fn shell_path(ro: Vec3, rd: Vec3, c: Vec3, radius: f32, t_scene: f32) -> f32 {
    let th = shell_thickness(radius);
    let outer = sphere_span(ro, rd, c, radius + th * 0.5, t_scene);
    let inner = sphere_span(ro, rd, c, (radius - th * 0.5).max(0.0), t_scene);
    (outer - inner).max(0.0)
}

/// The blasts for `post.wgsl`, in world space: per blast `(centre xyz,
/// radius)` and `(strength, thickness, 0, 0)`; dead slots have strength 0.
pub fn shell_blasts(blast_a: &[[f32; 4]], blast_b: &[[f32; 4]]) -> ([[f32; 4]; MAX_RINGS], [[f32; 4]; MAX_RINGS]) {
    let (mut a_out, mut b_out) = ([[0.0; 4]; MAX_RINGS], [[0.0; 4]; MAX_RINGS]);
    let mut n = 0;
    for (a, b) in blast_a.iter().zip(blast_b) {
        if b[0] <= 0.002 || n >= MAX_RINGS || !a[3].is_finite() {
            continue;
        }
        a_out[n] = [a[0], a[1] + 1.0, a[2], a[3].max(0.01)];
        b_out[n] = [b[0], shell_thickness(a[3]), 0.0, 0.0];
        n += 1;
    }
    (a_out, b_out)
}

/// Half-extent of the shadow box for a camera this far from its subject.
/// Close play gets 45 u (a 90 u box: 0.044 u/texel at 2048, against the old
/// fixed 300 u box's 0.146); a pulled-back overview gets the whole arena.
pub fn shadow_half_extent(cam_dist: f32) -> f32 {
    (finite_or(cam_dist, 150.0) * 3.0 + 25.0).clamp(45.0, 150.0)
}

/// The light's view-projection, aimed at `focus` and snapped to texels.
///
/// Without the snap, moving the camera slides the shadow map's sample grid
/// across the world by fractions of a texel and every shadow edge crawls.
pub fn fit_shadow(focus: Vec3, sun: Vec3, half: f32, map_size: u32) -> Mat4 {
    let sun = if sun.is_finite() && sun.length_squared() > 1e-6 { sun.normalize() } else { Vec3::Y };
    let focus = if focus.is_finite() { Vec3::new(focus.x, 0.0, focus.z) } else { Vec3::ZERO };
    let up = if sun.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    // Snap in the light's own frame, where texels are axis-aligned.
    let basis = Mat4::look_at_rh(sun * 130.0, Vec3::ZERO, up);
    let texel = 2.0 * half / map_size.max(1) as f32;
    let l = basis.transform_vector3(focus);
    let snapped = Vec3::new((l.x / texel).round() * texel, (l.y / texel).round() * texel, l.z);
    let focus = basis.inverse().transform_vector3(snapped);
    let view = Mat4::look_at_rh(focus + sun * 130.0, focus, up);
    Mat4::orthographic_rh(-half, half, -half, half, 1.0, 320.0) * view
}

fn finite01(x: f32) -> f32 {
    if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 }
}
fn finite_or(x: f32, d: f32) -> f32 {
    if x.is_finite() { x } else { d }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ask, as a number: cracks "heal in the rain". Rain must be the
    /// thing that does it -- more than ten times faster than dry ground --
    /// and dry ground must still heal, or a long match ends on a moonscape.
    #[test]
    fn cracks_close_slowly_when_dry_and_more_than_ten_times_faster_in_rain() {
        let heal_secs = |rain: f32, wetness: f32| {
            let mut g = GroundFx::default();
            assert!(g.strike(Vec3::ZERO, 5.0, 1.0));
            let mut t = 0.0;
            while !g.cracks.is_empty() {
                g.tick(0.25, rain, wetness);
                t += 0.25;
                assert!(t < 2000.0, "a crack that never heals");
            }
            t
        };
        let (dry, rain, after) = (heal_secs(0.0, 0.0), heal_secs(1.0, 1.0), heal_secs(0.0, 1.0));
        assert!((200.0..=260.0).contains(&dry), "dry: {dry}s");
        assert!((10.0..=18.0).contains(&rain), "downpour: {rain}s");
        assert!(dry / rain > 10.0, "rain is what heals: {dry}s dry vs {rain}s wet");
        assert!(after < dry * 0.2 && after > rain * 1.5, "soaked ground keeps healing after the rain stops, more slowly: {after}s");
    }

    #[test]
    fn a_light_hit_shakes_the_trees_and_does_not_break_the_ground() {
        let mut g = GroundFx::default();
        assert!(!g.strike(Vec3::ZERO, 2.0, CRACK_POWER - 0.05));
        assert_eq!((g.blasts.len(), g.cracks.len()), (1, 0));
        assert!(g.strike(Vec3::new(30.0, 0.0, 0.0), 6.0, 0.9));
        assert_eq!((g.blasts.len(), g.cracks.len()), (2, 1));
        // Hitting broken ground re-opens it; it does not stack.
        g.tick(0.25, 1.0, 1.0);
        let before = g.cracks[0].open;
        assert!(before < 1.0);
        g.strike(Vec3::new(31.0, 0.0, 0.5), 6.0, 0.9);
        assert_eq!(g.cracks.len(), 1);
        assert_eq!(g.cracks[0].open, 1.0);
        for bad in [f32::NAN, -1.0, 0.0] {
            assert!(!g.strike(Vec3::ZERO, bad, 1.0) && !g.strike(Vec3::ZERO, 3.0, bad));
        }
    }

    #[test]
    fn a_blast_front_travels_weakens_and_ends_and_the_lists_stay_bounded() {
        let mut g = GroundFx::default();
        g.strike(Vec3::ZERO, 6.0, 1.0);
        let (mut last_r, mut last_s) = (-1.0f32, f32::MAX);
        let mut frames = 0;
        while let Some(b) = g.blasts.first().copied() {
            assert!(b.radius() > last_r && b.strength() <= last_s, "outward and weakening");
            (last_r, last_s) = (b.radius(), b.strength());
            g.tick(1.0 / 60.0, 0.0, 0.0);
            frames += 1;
            assert!(frames < 600);
        }
        assert!((18.0..=26.0).contains(&last_r), "reached ~4x the hit radius: {last_r}");
        assert!((40..70).contains(&frames), "about a second to cross: {frames} frames");
        for i in 0..40 {
            g.strike(Vec3::new(i as f32 * 20.0, 0.0, 0.0), 6.0, 1.0);
        }
        assert!(g.blasts.len() <= MAX_BLASTS && g.cracks.len() <= MAX_CRACKS);
        let (a, b) = g.blast_uniform();
        assert!(a.iter().zip(&b).all(|(a, b)| a.iter().chain(b).all(|v| v.is_finite())));
    }

    /// The shadow map is aimed at the camera's subject, is three times
    /// sharper up close, and does not crawl.
    #[test]
    fn the_shadow_box_follows_the_camera_and_snaps_to_texels() {
        assert_eq!(shadow_half_extent(8.0), 49.0);
        assert_eq!(shadow_half_extent(2.0), 45.0);
        assert_eq!(shadow_half_extent(90.0), 150.0);
        assert!(150.0 / shadow_half_extent(8.0) > 3.0, "three times the texel density in close play");

        let sun = Vec3::new(0.4, 0.8, 0.3);
        let (half, size) = (45.0, 2048);
        let focus = Vec3::new(37.3, 0.0, -61.9);
        let m = fit_shadow(focus, sun, half, size);
        let ndc = m.project_point3(focus);
        assert!(ndc.x.abs() < 0.01 && ndc.y.abs() < 0.01, "the subject is mid-map: {ndc}");
        let edge = m.project_point3(focus + Vec3::X * 200.0);
        assert!(edge.x.abs() > 1.0 || edge.y.abs() > 1.0, "and the far arena is outside it");

        // A world point's position in the map, in texels, as the camera
        // creeps: the FRACTIONAL part must not change, or edges shimmer.
        let probe = Vec3::new(40.0, 0.0, -60.0);
        let frac = |f: Vec3| {
            let p = fit_shadow(f, sun, half, size).project_point3(probe);
            let t = (p.x * 0.5 + 0.5) * size as f32;
            t - t.floor()
        };
        let base = frac(focus);
        let mut checked = 0;
        for k in 1..40 {
            let f = frac(focus + Vec3::new(k as f32 * 0.013, 0.0, k as f32 * -0.007));
            let d = (f - base).abs().min(1.0 - (f - base).abs());
            assert!(d < 0.02, "step {k}: the probe slid {d} of a texel");
            checked += 1;
        }
        assert_eq!(checked, 39);
        assert!(fit_shadow(Vec3::splat(f32::NAN), Vec3::ZERO, half, size).is_finite());
    }

    /// The old screen-ring test's promise, for the world-space shells: a
    /// blast behind the camera does not bend the frame (every view ray
    /// points ahead of the camera and misses it), and a blast ahead does.
    #[test]
    fn a_blast_behind_the_camera_does_not_bend_the_frame() {
        let eye = Vec3::new(0.0, 6.0, -10.0);
        let ahead = Vec3::new(0.0, 1.0, 10.0);
        let behind = Vec3::new(0.0, 1.0, -40.0);
        let mut hit_ahead = 0;
        for k in 0..400 {
            // a fan of view rays across a 90-degree field
            let a = (k % 20) as f32 / 19.0 - 0.5;
            let b = (k / 20) as f32 / 19.0 - 0.5;
            let rd = (Vec3::Z + Vec3::X * a * 1.8 + Vec3::Y * b * 1.0).normalize();
            assert_eq!(shell_path(eye, rd, behind, 4.0, 1.0e6), 0.0, "a ray ahead crossed a blast behind");
            hit_ahead += (shell_path(eye, rd, ahead, 4.0, 1.0e6) > 0.0) as u32;
        }
        assert!(hit_ahead > 20, "the blast ahead is seen");
    }
}

#[cfg(test)]
mod shell_tests {
    use super::*;

    const FAR: f32 = 1.0e6;

    /// A surface between the camera and the blast hides it entirely: the
    /// old rings bent whatever stood in front of them.
    #[test]
    fn a_surface_in_front_occludes_the_blast() {
        let (ro, rd, c) = (Vec3::ZERO, Vec3::Z, Vec3::new(0.0, 0.0, 20.0));
        assert!(shell_path(ro, rd, c, 5.0, FAR) > 0.5, "open air: the shell is crossed");
        assert_eq!(shell_path(ro, rd, c, 5.0, 10.0), 0.0, "a wall at 10 u, the blast at 15-25 u: nothing");
        // a surface INSIDE the shell's hollow: only the near wall counts
        let near_wall = shell_path(ro, rd, c, 5.0, 20.0);
        assert!((near_wall - shell_thickness(5.0)).abs() < 1e-3, "{near_wall}");
    }

    /// Straight through the middle: two walls. Grazing the edge: more --
    /// the silhouette is where the air is thickest along the view.
    #[test]
    fn the_shell_is_strongest_at_its_silhouette() {
        let (c, r) = (Vec3::new(0.0, 0.0, 30.0), 6.0);
        let th = shell_thickness(r);
        let through = shell_path(Vec3::ZERO, Vec3::Z, c, r, FAR);
        assert!((through - 2.0 * th).abs() < 1e-3, "{through} vs {}", 2.0 * th);
        let graze_dir = (c + Vec3::X * (r + th * 0.25)).normalize();
        let graze = shell_path(Vec3::ZERO, graze_dir, c, r, FAR);
        assert!(graze > through * 1.5, "grazing {graze} vs through {through}");
        let miss = shell_path(Vec3::ZERO, (c + Vec3::X * (r + th)).normalize(), c, r, FAR);
        assert_eq!(miss, 0.0);
    }

    /// Behind the camera is nothing; inside the blast, the far wall only.
    #[test]
    fn behind_and_inside() {
        assert_eq!(shell_path(Vec3::ZERO, Vec3::Z, Vec3::new(0.0, 0.0, -30.0), 5.0, FAR), 0.0);
        let inside = shell_path(Vec3::ZERO, Vec3::Z, Vec3::ZERO, 5.0, FAR);
        assert!((inside - shell_thickness(5.0)).abs() < 1e-3, "{inside}");
    }

    #[test]
    fn only_live_blasts_are_sent() {
        let a = [[1.0, 0.0, 2.0, 4.0], [5.0, 0.0, 5.0, 3.0]];
        let b = [[0.0, 0.0, 0.0, 0.0], [0.8, 0.0, 0.0, 0.0]];
        let (sa, sb) = shell_blasts(&a, &b);
        assert_eq!(sa[0], [5.0, 1.0, 5.0, 3.0]);
        assert_eq!(sb[0][0], 0.8);
        assert_eq!(sb[1][0], 0.0);
    }
}
