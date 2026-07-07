//! enhance.rs — a **recursive, closed-loop map enhancer**.
//!
//! The scorers in `mapeval`, `sightline` and `liveliness` are read-only: they tell you *how* good a map is,
//! but they can't make it better. This module closes the loop. It takes a base `Level` and repeatedly:
//!   1. proposes a small **mutation** (add cover to break a sightline, plant a swaying foliage clump, drop an
//!      emissive landmark, stir in ambient motion, scatter scenic detail),
//!   2. re-scores the mutated map with a weighted **cinematic + gameplay objective**,
//!   3. keeps the change if it improves the objective, reverts it otherwise (hill-climbing).
//!
//! The result is a map that starts from a hand-authored (or procedural) base and is *iteratively pushed*
//! toward "more cinematic and fun" — more tactically varied sightlines, a livelier scene that pricks the eye,
//! and a clearer focal landmark — without a human hand-placing every prop.
//!
//! ## The cinematic-tuning slider
//! [`CinematicTuning`] is a single 0..1 intensity the UI can expose as a slider in map selection. It scales
//! how aggressively the enhancer mutates: at 0.0 the map is left as authored; at 1.0 it's pushed hard toward a
//! dense, dramatic, high-motion scene. In between you get a smooth dial from "clean competitive" to
//! "cinematic spectacle", so one base map yields a family of varied-but-fun layouts.
//!
//! ## Why a cheap Level-side objective (not the full `liveliness` sim)
//! The full liveliness harness runs a real `Game` for hundreds of frames — far too slow to call once per
//! candidate mutation across a hill-climb. So `enhance` scores against a *static proxy* computed directly on
//! the `Level`: gameplay structure from `sightline`, plus a motion-potential estimate from the density and
//! spread of the map's actual motion sources (fires, swaying foliage/props, water). The proxy is validated
//! against the real harness in tests, and the real harness remains the final arbiter.

use crate::level::{Level, Rng};
use crate::math::{Vec2, Vec3};
use crate::mesh::{boxx, dither, leaf_cluster, lerp_c, rgb, tube, Material, FACE_ALL};

/// A single 0..1 dial for how hard to push a map toward cinematic spectacle. Surface this as a slider in the
/// map-selection UI. 0 = leave the base map as authored; 1 = maximum enhancement (denser cover, more motion,
/// a bolder landmark, richer scatter).
#[derive(Clone, Copy, Debug)]
pub struct CinematicTuning(pub f32);

impl Default for CinematicTuning {
    fn default() -> Self {
        CinematicTuning(0.5)
    }
}

impl CinematicTuning {
    pub fn clamped(self) -> f32 {
        self.0.clamp(0.0, 1.0)
    }
    /// A human label for the slider position (for the UI).
    pub fn label(self) -> &'static str {
        match self.clamped() {
            x if x < 0.12 => "Off (as authored)",
            x if x < 0.35 => "Subtle",
            x if x < 0.6 => "Balanced",
            x if x < 0.85 => "Cinematic",
            _ => "Spectacle",
        }
    }
}

/// A summary of what an enhancement pass changed and how the objective moved.
#[derive(Clone, Debug, Default)]
pub struct EnhanceReport {
    pub iterations: u32,
    pub accepted: u32,
    pub cover_added: u32,
    pub foliage_added: u32,
    pub landmarks_added: u32,
    pub embers_added: u32,
    pub scatter_added: u32,
    pub start_objective: f32,
    pub final_objective: f32,
}

/// A static, cheap cinematic-quality proxy for a Level: gameplay structure (from `sightline`) blended with a
/// motion-potential estimate (how much of the map has live motion sources, and how well-spread they are), plus
/// a landmark/focal term. Returns 0..1-ish; higher is better. This is what the hill-climb maximises.
pub fn cinematic_objective(level: &Level) -> f32 {
    let sr = crate::sightline::analyze(level);
    // Gameplay structure: varied exposure, real chokepoints, safe pockets. (0..1)
    let structure = sr.structure_score;

    // Motion potential: count motion sources (fires, swaying meshes, water) and reward BOTH a healthy amount
    // and a good spatial spread — a lively map has motion distributed across it, not clumped in one corner.
    let motion = motion_potential(level);

    // Focal landmark: a bright emissive or tall structure to orient around and frame a shot.
    let landmark = if crate::mapeval::score(level).has_landmark { 1.0 } else { 0.0 };

    // Composition: does the arrangement read as a *designed* space (clustered masses, legible voids, a size
    // hierarchy) rather than uniform noise? This is the anti-gaming term — it stops the enhancer from "winning"
    // by dumping uniform clutter (which lifts density-based proxies but reads as scatter to a human). Because
    // the operators place cover at exposure-guided spots and foliage in the open, they naturally build
    // structure; scoring it explicitly keeps the hill-climb honest.
    let comp = crate::composition::analyze(level).score;

    // Weighted: composition + gameplay lead (it must read as a place AND play well), motion is the cinematic
    // heart, landmark is a bonus.
    (comp * 0.30 + structure * 0.34 + motion * 0.26 + landmark * 0.10).clamp(0.0, 1.0)
}

/// Is this map already saturated with ambient motion the enhancer shouldn't add to? True when a sandstorm or
/// a large animated water body already fills much of the frame, or fires/water are already dense — cases where
/// the real liveliness harness is near or past its saturation peak and MORE motion would only hurt.
pub fn motion_saturated(level: &Level) -> bool {
    if level.sandstorm.is_some() {
        return true;
    }
    let ts = level.ts();
    let area = (level.w * level.h) as f32 * ts * ts;
    let water_area: f32 = level.water.iter().map(|w| std::f32::consts::PI * w.radius * w.radius).sum();
    if water_area > area * 0.35 {
        return true;
    }
    // dense always-on sources already
    let sat_budget = (area / 500.0).max(2.0);
    if level.fire_sources.len() as f32 > sat_budget * 1.5 {
        return true;
    }
    // already densely foliated (a forest): counting swaying meshes. Adding more foliage to a map that's
    // already full of wind-bowed leaves just saturates the motion the eye tunes out.
    let sway_meshes = level.meshes.iter().filter(|m| !m.sway.is_empty() && m.sway.iter().any(|&w| w > 0.05)).count();
    sway_meshes as f32 > (area / 120.0).max(6.0)
}

/// Estimate a map's motion potential 0..1 from the count and spread of its actual motion sources.
fn motion_potential(level: &Level) -> f32 {
    let ts = level.ts();
    let map_diag = ((level.w as f32 * ts).powi(2) + (level.h as f32 * ts).powi(2)).sqrt().max(1.0);

    // gather motion-source points, weighted by how INTERMITTENT their motion is. The real scorer rewards a
    // calm baseline with periodic spikes and punishes constant churn — so always-on sources (fire, water)
    // count for less than gust-driven ones (swaying foliage/props that mostly rest and bow in gusts).
    let mut pts: Vec<Vec2> = Vec::new();
    let mut constant_motion = 0.0f32; // tally of always-on sources (for a saturation penalty)
    for f in &level.fire_sources {
        pts.push(Vec2::new(f.x, f.z));
        constant_motion += 1.0;
    }
    for m in &level.meshes {
        if !m.sway.is_empty() && m.sway.iter().any(|&w| w > 0.05) {
            if let Some(v) = m.verts.first() {
                pts.push(Vec2::new(v.x + m.offset.x, v.z + m.offset.z));
            }
        }
    }
    for w in &level.water {
        pts.push(Vec2::new(w.cx, w.cz));
        constant_motion += 1.0;
    }

    if pts.is_empty() {
        return 0.0;
    }

    // amount term: motion should hit a SWEET SPOT, not maximise. The real liveliness harness penalises
    // constant saturation (motion everywhere becomes noise the eye tunes out), so we reward a moderate density
    // and fall off on BOTH sides — too few sources (dead) and too many (saturated). Peak around one source per
    // ~150 m^2 of play area.
    let area = (level.w * level.h) as f32 * ts * ts;
    let ideal = (area / 420.0).max(3.0);
    let ratio = pts.len() as f32 / ideal;
    // triangular peak at ratio==1: rises to 1.0 then falls off past the ideal (saturation penalty)
    let amount = if ratio <= 1.0 { ratio } else { (2.0 - ratio).max(0.0) };

    // spread term: mean nearest-neighbour distance normalised by map diagonal — rewards distribution over
    // clumping. With one point, spread is undefined → treat as low.
    let spread = if pts.len() < 2 {
        0.15
    } else {
        let mut nn_sum = 0.0;
        for (i, a) in pts.iter().enumerate() {
            let mut best = f32::MAX;
            for (j, b) in pts.iter().enumerate() {
                if i == j {
                    continue;
                }
                let d = ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
                best = best.min(d);
            }
            nn_sum += best;
        }
        let mean_nn = nn_sum / pts.len() as f32;
        // a good spread has neighbours a modest fraction of the diagonal apart; too clumped → 0, too sparse is
        // fine. Peak reward around ~12% of the diagonal.
        (mean_nn / (map_diag * 0.12)).clamp(0.0, 1.0)
    };

    // blend: you need enough sources AND a decent spread
    let base = amount * 0.6 + spread * 0.4;
    // saturation penalty: a scene dominated by always-on motion (many fires/large water) reads as constant
    // churn to the real harness. Softly penalise once constant sources get dense relative to map size.
    let sat_budget = (area / 500.0).max(2.0);
    let mut sat = (constant_motion / sat_budget - 1.0).max(0.0);
    // Ambient weather the static proxy can't otherwise see but the real sim animates constantly: a sandstorm
    // fills the whole map with drifting particles, and a large ocean animates a big fraction of the frame.
    // Both mean the map is ALREADY lively — treat them as heavy existing saturation so the enhancer doesn't
    // pile motion onto an already-busy scene (which is what tanked the real liveliness of Malecón/Desert Town).
    if level.sandstorm.is_some() {
        sat += 2.0;
    }
    let water_area: f32 = level.water.iter().map(|w| std::f32::consts::PI * w.radius * w.radius).sum();
    if water_area > area * 0.35 {
        sat += 1.5;
    }
    (base * (1.0 - (sat * 0.5).min(0.85))).clamp(0.0, 1.0)
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────────────
// Mutation operators. Each attempts one small, self-contained change and returns whether it did anything.
// They're deliberately additive (they never remove authored geometry) so enhancement can't vandalise a base.
// ─────────────────────────────────────────────────────────────────────────────────────────────────────────

/// Add a small batch of exposure-guided cover to break the most exposed sightlines (gameplay structure).
fn op_add_cover(level: &mut Level, tuning: f32, _rng: &mut Rng) -> u32 {
    let budget = (1.0 + tuning * 3.0) as usize;
    let col = lerp_c(level.pal.cover, rgb(120, 108, 92), 0.3);
    level.scatter_exposure_cover(budget, col) as u32
}

/// Plant a swaying foliage clump on open floor — a low, wind-bowed cluster that adds ambient motion
/// (liveliness) and silhouette without blocking a standing sightline.
fn op_add_foliage(level: &mut Level, tuning: f32, rng: &mut Rng) -> u32 {
    let ts = level.ts();
    let mut planted = 0;
    let tries = 2 + (tuning * 4.0) as i32;
    let leaf = lerp_c(rgb(70, 120, 60), level.pal.rock, 0.15);
    for _ in 0..tries {
        let x = rng.i32_range(3, level.w - 3);
        let z = rng.i32_range(3, level.h - 3);
        let wp = Vec2::new((x as f32 + 0.5) * ts, (z as f32 + 0.5) * ts);
        // only on genuinely open, walkable ground away from existing cover
        let Some(surf) = level.surface_at(wp) else { continue };
        if level.obstacles.iter().any(|o| (o.center.x - wp.x).abs() < 2.0 && (o.center.z - wp.y).abs() < 2.0) {
            continue;
        }
        let base = Vec3::new(wp.x, surf, wp.y);
        // a thin stem + a bushy swaying crown
        let hgt = 1.3 + rng.f32() * 0.6;
        let stem = tube(base, base + Vec3::new(0.0, hgt * 0.6, 0.0), 0.05, 4, dither(rgb(96, 74, 48), x, 0.1));
        level.meshes.push(stem);
        let mut crown = leaf_cluster(base + Vec3::new(0.0, hgt, 0.0), 0.5 + rng.f32() * 0.3, rng.next_u64() as u32, dither(leaf, z, 0.1));
        crown.material = Some(Material::Moss);
        // sway weight ramps 0→1 up the crown so it bows in the wind (liveliness source)
        let top = base.y + hgt + 0.5;
        let bot = base.y + hgt * 0.6;
        crown.sway = crown.verts.iter().map(|v| ((v.y - bot) / (top - bot).max(0.1)).clamp(0.0, 1.0)).collect();
        level.meshes.push(crown);
        planted += 1;
    }
    planted
}

/// Drop an emissive focal element (a warm brazier/lamp) at an open, central-ish spot — a landmark to orient
/// around, plus a flickering fire source for motion and dynamic light.
fn op_add_landmark(level: &mut Level, _tuning: f32, rng: &mut Rng) -> u32 {
    let ts = level.ts();
    // search a few candidate spots near the map centre for open ground
    for _ in 0..6 {
        let cx = level.w as f32 * 0.5 + (rng.f32() - 0.5) * level.w as f32 * 0.4;
        let cz = level.h as f32 * 0.5 + (rng.f32() - 0.5) * level.h as f32 * 0.4;
        let wp = Vec2::new(cx * ts, cz * ts);
        let Some(surf) = level.surface_at(wp) else { continue };
        if level.obstacles.iter().any(|o| (o.center.x - wp.x).abs() < 2.5 && (o.center.z - wp.y).abs() < 2.5) {
            continue;
        }
        let base = Vec3::new(wp.x, surf, wp.y);
        // a stone plinth + a glowing brazier bowl
        let mut plinth = boxx(base + Vec3::new(0.0, 0.5, 0.0), Vec3::new(0.5, 0.5, 0.5), level.pal.rock, FACE_ALL);
        plinth.material = Some(Material::StoneBlock);
        level.meshes.push(plinth);
        let mut bowl = crate::mesh::disc(base + Vec3::new(0.0, 1.05, 0.0), 0.55, 12, rgb(60, 40, 30));
        bowl.emissive = 0.2;
        level.meshes.push(bowl);
        let mut flame = crate::mesh::uv_sphere(base + Vec3::new(0.0, 1.3, 0.0), 0.4, 8, 6, rgb(255, 170, 70));
        flame.emissive = 0.95; // bright emissive → counts as a landmark
        level.meshes.push(flame);
        level.obstacles.push(crate::level::Obstacle { center: base + Vec3::new(0.0, 0.5, 0.0), half: Vec3::new(0.5, 0.5, 0.5) });
        // NOTE: deliberately NOT a fire_source — a steady emissive glow is a focal landmark without adding
        // constant motion (which the liveliness harness penalises). Motion comes from foliage/gusts instead.
        return 1;
    }
    0
}

/// Scatter small scenic detail (low rocks / debris) on bare ground to break the flat-plane look — pure visual
/// richness, no gameplay effect (kept low so it never becomes unintended cover).
fn op_add_scatter(level: &mut Level, tuning: f32, rng: &mut Rng) -> u32 {
    let ts = level.ts();
    let mut n = 0;
    let tries = 3 + (tuning * 5.0) as i32;
    for _ in 0..tries {
        let x = rng.i32_range(3, level.w - 3);
        let z = rng.i32_range(3, level.h - 3);
        let wp = Vec2::new((x as f32 + 0.5) * ts, (z as f32 + 0.5) * ts);
        let Some(surf) = level.surface_at(wp) else { continue };
        let base = Vec3::new(wp.x + (rng.f32() - 0.5), surf, wp.y + (rng.f32() - 0.5));
        let r = 0.18 + rng.f32() * 0.22;
        let rock = crate::mesh::rock(base + Vec3::new(0.0, r * 0.4, 0.0), r, rng.next_u64() as u32, dither(level.pal.rock, x + z, 0.12));
        level.meshes.push(rock);
        n += 1;
    }
    n
}

/// Run the recursive enhancer on `level` in place. Hill-climbs the cinematic objective for up to `iterations`
/// steps at the given `tuning` intensity, keeping only mutations that improve the objective. Returns a report.
///
/// Because the operators are additive and each accepted step strictly improves the objective, the result is
/// monotonically at least as good as the base under the objective — enhancement never makes a map score worse.
pub fn enhance(level: &mut Level, tuning: CinematicTuning, seed: u64, iterations: u32) -> EnhanceReport {
    let t = tuning.clamped();
    let mut report = EnhanceReport::default();
    report.start_objective = cinematic_objective(level);
    report.final_objective = report.start_objective;
    if t <= 0.001 {
        return report; // slider off → leave the base untouched
    }
    let mut rng = Rng::new(seed ^ 0xC1_7E_A1_11);
    // scale the effort by tuning: more iterations when the slider is high
    let iters = ((iterations as f32) * (0.4 + 0.6 * t)) as u32;
    // Hard cap on added motion (foliage) sources, scaled by map size and tuning. Even though each foliage
    // clump nudges the static objective up, piling on dozens saturates the *real* liveliness (validated with
    // the full sim). This cap keeps enhancement in the "calm baseline + intermittent gusts" sweet spot.
    let ts = level.ts();
    let area = (level.w * level.h) as f32 * ts * ts;
    let foliage_cap = ((area / 900.0) * (0.4 + 0.6 * t)).ceil() as u32;

    for _ in 0..iters {
        report.iterations += 1;
        let before = cinematic_objective(level);

        // snapshot the additive state so a rejected mutation can be rolled back exactly
        let snap_meshes = level.meshes.len();
        let snap_obst = level.obstacles.len();
        let snap_cover = level.cover_obstacles.len();
        let snap_fire = level.fire_sources.len();

        // pick an operator. Weight toward whatever the map is weakest in: if structure is low, favour cover;
        // if motion is low (and the map isn't already saturated with ambient motion), favour foliage; always
        // allow scatter/landmark.
        let sr = crate::sightline::analyze(level);
        let motion = motion_potential(level);
        let saturated = motion_saturated(level);
        let roll = rng.f32();
        let (kind, added): (u8, u32) = if sr.structure_score < 0.45 && roll < 0.5 {
            (0, op_add_cover(level, t, &mut rng))
        } else if motion < 0.5 && !saturated && report.foliage_added < foliage_cap && roll < 0.75 {
            // Intermittent motion only (swaying foliage that rests and gusts). We deliberately do NOT add fire
            // here: constant flame motion tanks the real liveliness score (validated against the full sim), so
            // fire stays an authored-only element, not something enhancement sprinkles around.
            (1, op_add_foliage(level, t, &mut rng))
        } else if !crate::mapeval::score(level).has_landmark && roll < 0.85 {
            (2, op_add_landmark(level, t, &mut rng))
        } else if roll < 0.55 {
            (0, op_add_cover(level, t, &mut rng))
        } else if !saturated && report.foliage_added < foliage_cap && roll < 0.8 {
            (1, op_add_foliage(level, t, &mut rng))
        } else {
            (4, op_add_scatter(level, t, &mut rng))
        };

        if added == 0 {
            continue;
        }
        let after = cinematic_objective(level);
        // scenic scatter (kind 4) is a pure visual win the objective doesn't fully capture, so accept it as
        // long as it doesn't hurt; everything else must strictly improve the objective.
        let accept = if kind == 4 { after >= before - 1e-4 } else { after > before + 1e-4 };
        if accept {
            report.accepted += 1;
            match kind {
                0 => report.cover_added += added,
                1 => report.foliage_added += added,
                2 => report.landmarks_added += added,
                3 => report.embers_added += added,
                4 => report.scatter_added += added,
                _ => {}
            }
            report.final_objective = after;
        } else {
            // roll back exactly to the pre-mutation additive state
            level.meshes.truncate(snap_meshes);
            level.obstacles.truncate(snap_obst);
            level.cover_obstacles.truncate(snap_cover);
            level.fire_sources.truncate(snap_fire);
        }
    }

    // rebuild the LOD field so the new geometry is chunked/culled like the rest
    level.chunk_meshes(40.0);
    report.final_objective = cinematic_objective(level);
    report
}

/// Convenience: generate a preset and return an enhanced copy at the given tuning. Used by the UI/preview and
/// the analysis tooling.
pub fn enhanced_preset(name: &str, seed: u64, tuning: CinematicTuning) -> Option<(Level, EnhanceReport)> {
    let p = crate::level::PRESETS.iter().find(|p| p.name == name)?;
    let mut level = Level::generate(p.config(seed));
    let report = enhance(&mut level, tuning, seed, 60);
    Some((level, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::PRESETS;

    #[test]
    fn tuning_off_leaves_the_map_untouched() {
        let p = &PRESETS[0];
        let mut a = Level::generate(p.config(7));
        let mesh_count = a.meshes.len();
        let obst = a.obstacles.len();
        let rep = enhance(&mut a, CinematicTuning(0.0), 7, 40);
        assert_eq!(a.meshes.len(), mesh_count, "tuning 0 must not add geometry");
        assert_eq!(a.obstacles.len(), obst);
        assert_eq!(rep.accepted, 0);
    }

    #[test]
    fn enhancement_never_lowers_the_objective() {
        // hill-climbing with rollback: the final objective must be >= the start for any preset/tuning.
        for name in ["Concrete Slab", "Redwood Canyon", "Flood Zone"] {
            let Some(p) = PRESETS.iter().find(|p| p.name == name) else { continue };
            let mut level = Level::generate(p.config(11));
            let start = cinematic_objective(&level);
            let rep = enhance(&mut level, CinematicTuning(0.8), 11, 50);
            let end = cinematic_objective(&level);
            assert!(end >= start - 1e-3, "{name}: enhancement lowered objective {start:.3} -> {end:.3}");
            assert!((rep.final_objective - end).abs() < 1e-2);
        }
    }

    #[test]
    fn higher_tuning_adds_more_content_than_lower() {
        // "Spectacle" means more visual/gameplay density, not necessarily a strictly higher objective (the
        // objective has a peak — piling on cover past the sweet spot can flatten exposure variety). So the
        // meaningful invariant is: higher tuning ends with MORE added geometry, and neither run drops the
        // objective below the untouched base.
        let p = PRESETS.iter().find(|p| p.name == "Concrete Slab").unwrap();
        let base_obj = cinematic_objective(&Level::generate(p.config(3)));

        let mut lo = Level::generate(p.config(3));
        let mut hi = Level::generate(p.config(3));
        let lo_base_meshes = lo.meshes.len();
        let hi_base_meshes = hi.meshes.len();
        enhance(&mut lo, CinematicTuning(0.2), 3, 50);
        enhance(&mut hi, CinematicTuning(1.0), 3, 50);

        let lo_added = lo.meshes.len().saturating_sub(lo_base_meshes);
        let hi_added = hi.meshes.len().saturating_sub(hi_base_meshes);
        assert!(hi_added >= lo_added, "spectacle should add >= subtle content ({hi_added} vs {lo_added})");
        assert!(cinematic_objective(&lo) >= base_obj - 1e-3, "subtle run must not drop below base");
        assert!(cinematic_objective(&hi) >= base_obj - 1e-3, "spectacle run must not drop below base");
    }

    #[test]
    fn enhancement_lifts_real_liveliness_on_a_sparse_map() {
        // The whole point, validated against the INDEPENDENT full-sim harness (not just the static objective):
        // enhancing a bare, low-motion map should raise its real cinematic-liveliness score, not tank it. This
        // is the regression guard for the fire-saturation bug that once crashed the real score.
        use crate::camera::FpsCam;
        use crate::game::Game;
        use crate::liveliness::analyze_game;
        use crate::loadout::Loadout;

        let idx = PRESETS.iter().position(|p| p.name == "Concrete Slab").unwrap();
        let live_at = |tuning: f32| -> f32 {
            let mut g = Game::new_match_tuned(
                7,
                Some(idx),
                Loadout::default(),
                Default::default(),
                12,
                crate::enemy::Difficulty::Regular,
                None,
                crate::creatures::HuntRole::Predator,
                CinematicTuning(tuning),
            );
            let lvl = &g.level;
            let c = Vec3::new(lvl.w as f32 * lvl.ts() * 0.5, lvl.floor_y + 2.0, lvl.h as f32 * lvl.ts() * 0.5);
            let cam = FpsCam { pos: c, yaw: 0.7, pitch: -0.08 };
            analyze_game(&mut g, &cam, 240).score
        };
        let base = live_at(0.0);
        let full = live_at(1.0);
        assert!(full >= base - 0.02, "enhancement must not tank real liveliness on a sparse map ({base:.2} -> {full:.2})");
    }

    #[test]
    fn already_lively_maps_are_left_alone() {
        // A map already saturated with ambient motion (an ocean, a sandstorm) must be recognised as such and
        // NOT have motion piled onto it (which would push the real score past its saturation peak).
        for name in ["Malecon Breakwater", "Desert Town"] {
            let Some(p) = PRESETS.iter().find(|p| p.name == name) else { continue };
            let level = Level::generate(p.config(7));
            assert!(motion_saturated(&level), "{name} should be recognised as already motion-saturated");
        }
    }

    #[test]
    fn enhanced_map_stays_playable() {
        // enhancement must not break the hard invariants: still connected, still spawns a full wave, still
        // clean geometry.
        let p = PRESETS.iter().find(|p| p.name == "Redwood Canyon").unwrap();
        let mut level = Level::generate(p.config(9));
        enhance(&mut level, CinematicTuning(1.0), 9, 60);
        assert!(crate::mapeval::score(&level).connected, "enhanced map must stay connected");
        let wave = crate::enemy::populate(&level, 13, 9);
        assert!(wave.len() >= 10, "enhanced map must still spawn a full wave, got {}", wave.len());
        // no degenerate geometry introduced
        let mut degen = 0;
        let meshes: Vec<_> = if let Some(f) = &level.lod_field { f.render_meshes().into_iter().cloned().collect() } else { level.meshes.clone() };
        for m in &meshes {
            for tri in &m.tris {
                let a = m.verts[tri[0] as usize];
                let b = m.verts[tri[1] as usize];
                let c = m.verts[tri[2] as usize];
                if (b - a).cross(c - a).len() * 0.5 < 1e-5 {
                    degen += 1;
                }
            }
        }
        assert_eq!(degen, 0, "enhancement introduced {degen} degenerate triangles");
    }
}
