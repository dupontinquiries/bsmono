//! `sim::plant_survival` — genome-gated survival rules for plants.
//!
//! Two heritable behaviours the ecology asked for, kept as **pure, deterministic
//! logic** so the lifecycle / runtime tick can consume them without pulling in the
//! world:
//!
//!  * **Rot** — a plant standing in water slowly dies *unless* its genome tolerates
//!    water (an aquatic lineage). Land genomes accumulate rot while submerged and
//!    eventually give out; aquatic genomes are immune. This is the organic version
//!    of the blunt "cut trees on water" cleanup: instead of vanishing at once, a
//!    doomed water plant rots away over time, and a genuinely aquatic one stays.
//!
//!  * **Unorthodox growth** — most plants are positively gravitropic (grow up,
//!    anchored below). A genome with enough `orientation_freedom` may instead grow
//!    **pendant** (downward, anchored to a ceiling) or **lateral** (sideways,
//!    anchored to a wall) — hanging/creeping trees. But orientation doesn't feed a
//!    plant: an unorthodox segment must still **catch light**, or it starves. So
//!    the genome unlocks the shape; light decides whether it survives there.
//!
//! Everything here is `0..1`-normalised, allocation-free, and panic-free (no
//! `unwrap`/`expect`/IO/RNG/clock). RNG-like values come in as a lineage `seed`.

use crate::util::hash_util::mix64_unit;

/// A plant lineage's heritable survival genes, each normalised `0..1`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SurvivalGenes {
    /// Tolerance to standing water. `0` = rots quickly when submerged; `1` = fully
    /// aquatic, never rots from water.
    pub water_tolerance: f32,
    /// Freedom from strict upward growth. `0` = strictly upright (normal); higher
    /// values unlock pendant (downward) then lateral (sideways) growth.
    pub orientation_freedom: f32,
    /// Trunk girth. `0` = slender (1-block trunk); `1` = stout (thick trunk). Also
    /// stiffens the plant against wind (a thicker beam resists bending far more).
    pub trunk_thickness: f32,
    /// Whippiness in wind. `0` = stiff (barely sways); `1` = supple (sways freely).
    pub flexibility: f32,
}

/// At/above this `water_tolerance` a lineage is treated as aquatic (immune to rot).
/// High on purpose: water-loving trees (mangrove-like) are the exception, ~10% of
/// lineages, so most trees that stray into water rot out.
pub const AQUATIC_THRESHOLD: f32 = 0.9;
/// At/above this `orientation_freedom` a lineage may grow downward/sideways — the
/// unusual hanging/creeping genomes, ~20% of lineages.
pub const UNORTHODOX_THRESHOLD: f32 = 0.8;
/// Light (`0..1`) a plant must catch per tick to break even. Below this it runs an
/// energy deficit; sustained deficit starves it.
pub const LIGHT_MAINTENANCE: f32 = 0.18;
/// Accumulated rot (or starvation deficit) at/above which the plant dies.
pub const DEATH_THRESHOLD: f32 = 1.0;
/// Base rot per tick for a fully-intolerant plant fully submerged (so ~1/RATE ticks
/// to die at tolerance 0). Small: rot is meant to be slow and visible.
pub const ROT_RATE: f32 = 0.02;

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::saturate(v)
}

impl SurvivalGenes {
    /// Clamp every gene into `0..1`.
    pub fn new(
        water_tolerance: f32,
        orientation_freedom: f32,
        trunk_thickness: f32,
        flexibility: f32,
    ) -> SurvivalGenes {
        SurvivalGenes {
            water_tolerance: clamp01(water_tolerance),
            orientation_freedom: clamp01(orientation_freedom),
            trunk_thickness: clamp01(trunk_thickness),
            flexibility: clamp01(flexibility),
        }
    }

    /// Derive a lineage's genes deterministically from its seed (same splitmix-unit
    /// hashing the other gene modules use). Most lineages are ordinary land, upright
    /// plants; aquatic / unorthodox genomes are the rarer tail of the distribution.
    pub fn from_seed(seed: u64) -> SurvivalGenes {
        SurvivalGenes {
            water_tolerance: mix64_unit(seed ^ 0x0000_00A9_0000_00A9),
            orientation_freedom: mix64_unit(seed ^ 0x0000_0F17_0000_0F17),
            trunk_thickness: mix64_unit(seed ^ 0x0000_7C1D_0000_7C1D),
            flexibility: mix64_unit(seed ^ 0x0000_51E3_0000_51E3),
        }
    }

    /// A lineage that thrives in water (immune to rot).
    pub fn is_aquatic(self) -> bool {
        self.water_tolerance >= AQUATIC_THRESHOLD
    }

    /// A lineage that may grow in unorthodox directions (down / sideways).
    pub fn may_grow_unorthodox(self) -> bool {
        self.orientation_freedom >= UNORTHODOX_THRESHOLD
    }
}

/// A direction a plant can extend a segment into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GrowthDir {
    Up,
    Down,
    North,
    South,
    East,
    West,
}

impl GrowthDir {
    /// Unit block offset for growing one step in this direction.
    pub fn step(self) -> (i32, i32, i32) {
        match self {
            GrowthDir::Up => (0, 1, 0),
            GrowthDir::Down => (0, -1, 0),
            GrowthDir::North => (0, 0, -1),
            GrowthDir::South => (0, 0, 1),
            GrowthDir::East => (1, 0, 0),
            GrowthDir::West => (-1, 0, 0),
        }
    }

    /// Offset from a segment to the block that ANCHORS it when grown this way — the
    /// opposite side. Up-growth is held from below; a pendant segment hangs from the
    /// block above; a lateral segment is braced by the wall behind it.
    pub fn support_offset(self) -> (i32, i32, i32) {
        let (dx, dy, dz) = self.step();
        (-dx, -dy, -dz)
    }

    pub fn is_lateral(self) -> bool {
        !matches!(self, GrowthDir::Up | GrowthDir::Down)
    }
}

/// Whether the genome permits extending a segment in `dir`. Up is always allowed.
/// Down needs enough `orientation_freedom`; lateral needs more still (creeping
/// sideways is the most derived trait).
pub fn permits(genes: SurvivalGenes, dir: GrowthDir) -> bool {
    match dir {
        GrowthDir::Up => true,
        GrowthDir::Down => genes.orientation_freedom >= UNORTHODOX_THRESHOLD,
        _ => genes.orientation_freedom >= (UNORTHODOX_THRESHOLD + 1.0) * 0.5, // ~0.8
    }
}

/// Per-tick rot added to a submerged plant. `0` when not submerged, and `0` for an
/// aquatic lineage. Otherwise it scales with how intolerant the lineage is, so a
/// nearly-aquatic plant lingers far longer than a dryland one. `dt_ticks` scales it
/// for catch-up ticks.
pub fn water_rot_increment(genes: SurvivalGenes, submerged: bool, dt_ticks: f32) -> f32 {
    if !submerged || genes.is_aquatic() {
        return 0.0;
    }
    let intolerance = 1.0 - clamp01(genes.water_tolerance);
    (ROT_RATE * intolerance * dt_ticks.max(0.0)).max(0.0)
}

/// Rot (or any accumulated deficit) has reached the lethal threshold.
pub fn is_dead(accum: f32) -> bool {
    accum >= DEATH_THRESHOLD
}

/// Per-tick energy balance for a plant catching `light01` of light. Positive =
/// thriving, negative = running a deficit (starving). Upright plants in daylight
/// stay positive; an unorthodox segment that grew into the dark goes negative.
pub fn energy_balance(light01: f32, dt_ticks: f32) -> f32 {
    (clamp01(light01) - LIGHT_MAINTENANCE) * dt_ticks.max(0.0)
}

/// Fold a per-tick energy balance into a running starvation accumulator, clamped at
/// `0` (surplus doesn't bank past healthy). Reaches [`DEATH_THRESHOLD`] ⇒ starved.
pub fn accumulate_starvation(prev_deficit: f32, energy_balance: f32) -> f32 {
    (prev_deficit - energy_balance).max(0.0)
}

// ─── Physics-based wind sway ───────────────────────────────────────────────────
//
// Cantilever intuition: wind DRAG on the foliage rises with the square of wind
// speed (drag ∝ v²). That load bends the trunk, resisted by STIFFNESS — which
// climbs steeply with trunk thickness (a thicker beam has a far larger second
// moment of area) and drops with genetic flexibility. Deflection is ~0 at the
// anchored base and grows toward the tip. The mesher/shader consume the amplitude;
// this is pure, deterministic, panic-free.

/// Largest trunk radius (in blocks) the thickest genome expresses to.
pub const MAX_TRUNK_RADIUS: u32 = 4;

/// Bending stiffness resisting sway. Rises fast with thickness (∝ 1 + 6·t²,
/// beam-like), falls with flexibility. Always ≥ a small floor so nothing divides by 0.
pub fn stiffness(trunk_thickness: f32, flexibility: f32) -> f32 {
    let t = clamp01(trunk_thickness);
    let flex = clamp01(flexibility);
    let base = 1.0 + 6.0 * t * t; // thick trunks are dramatically stiffer
    (base * (1.0 - 0.7 * flex)).max(0.15)
}

/// Peak (tip) horizontal deflection for a plant of `height_blocks` in wind of
/// `wind_speed`. Drag ∝ speed², moment ∝ height, resisted by stiffness. Clamped to a
/// fraction of height so a plant can bend but never fold flat.
pub fn tip_deflection(
    wind_speed: f32,
    height_blocks: f32,
    trunk_thickness: f32,
    flexibility: f32,
) -> f32 {
    let drag = wind_speed.max(0.0);
    let drag = drag * drag; // ∝ v²
    let h = height_blocks.max(0.0);
    let defl = 0.004 * drag * h / stiffness(trunk_thickness, flexibility);
    defl.min(h * 0.5) // never bend past ~45° worth of tip travel
}

/// Deflection fraction at a normalised height `y_frac` (0 = base, 1 = tip). Base is
/// anchored (0) and it grows ~quadratically toward the tip — the classic cantilever
/// shape, so the crown moves and the trunk base stays put.
pub fn height_profile(y_frac: f32) -> f32 {
    let y = clamp01(y_frac);
    y * y
}

/// Full horizontal sway offset `[dx, dz]` at time `t` for a segment at height
/// `y_frac`, given the (unit) wind direction and speed. It's a mean DOWNWIND bend
/// (∝ v²) plus a slow sway and faster flutter oscillating around it, all scaled by
/// the tip deflection and the cantilever height profile. Real foliage leans downwind
/// and shivers about that lean rather than swinging symmetrically about vertical.
pub fn sway_offset(
    wind_dir: [f32; 2],
    wind_speed: f32,
    height_blocks: f32,
    y_frac: f32,
    t: f32,
    genes: SurvivalGenes,
) -> [f32; 2] {
    let (dx, dz) = (wind_dir[0], wind_dir[1]);
    let len = (dx * dx + dz * dz).sqrt();
    let (ux, uz) = if len > 1e-4 { (dx / len, dz / len) } else { (1.0, 0.0) };
    let tip = tip_deflection(wind_speed, height_blocks, genes.trunk_thickness, genes.flexibility);
    let prof = height_profile(y_frac);
    let flex = clamp01(genes.flexibility);
    // slow sway + faster flutter; flexible genomes oscillate a bit wider.
    let osc = 0.6 * (t * 1.4).sin() + 0.4 * (t * 3.1 + 1.7).sin();
    let mean_bend = 0.7; // fraction of tip deflection that is steady downwind lean
    let amp = tip * prof * (mean_bend + (1.0 - mean_bend) * (0.5 + 0.5 * flex) * osc);
    [ux * amp, uz * amp]
}

/// Map the trunk-thickness gene to a discrete trunk RADIUS in blocks (girth
/// variation for the grower). `1..=MAX_TRUNK_RADIUS`.
pub fn trunk_radius_from_gene(trunk_thickness: f32) -> u32 {
    let steps = (MAX_TRUNK_RADIUS - 1) as f32;
    1 + (clamp01(trunk_thickness) * steps + 0.5) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genes_clamp_and_classify() {
        let g = SurvivalGenes::new(2.0, -1.0, 0.5, 0.5);
        assert_eq!(g.water_tolerance, 1.0);
        assert_eq!(g.orientation_freedom, 0.0);
        assert!(g.is_aquatic());
        assert!(!g.may_grow_unorthodox());
    }

    #[test]
    fn from_seed_is_deterministic_and_in_range() {
        for s in 0..2000u64 {
            let a = SurvivalGenes::from_seed(s);
            let b = SurvivalGenes::from_seed(s);
            assert_eq!(a, b);
            assert!((0.0..=1.0).contains(&a.water_tolerance));
            assert!((0.0..=1.0).contains(&a.orientation_freedom));
        }
    }

    #[test]
    fn aquatic_lineages_never_rot() {
        let aq = SurvivalGenes::new(0.9, 0.0, 0.5, 0.5);
        assert_eq!(water_rot_increment(aq, true, 1.0), 0.0);
    }

    #[test]
    fn dryland_plant_rots_to_death_in_water_over_time() {
        let land = SurvivalGenes::new(0.0, 0.0, 0.5, 0.5);
        // not submerged → no rot
        assert_eq!(water_rot_increment(land, false, 1.0), 0.0);
        // submerged → accumulates to death in finite time
        let mut rot = 0.0;
        let mut ticks = 0;
        while !is_dead(rot) && ticks < 100_000 {
            rot += water_rot_increment(land, true, 1.0);
            ticks += 1;
        }
        assert!(is_dead(rot));
        assert!(ticks > 1, "rot should be gradual, not instant");
        assert!(ticks < 1000, "rot shouldn't take forever ({ticks})");
    }

    #[test]
    fn tolerant_plant_outlasts_intolerant_one() {
        let tough = SurvivalGenes::new(0.6, 0.0, 0.5, 0.5);
        let frail = SurvivalGenes::new(0.1, 0.0, 0.5, 0.5);
        assert!(water_rot_increment(tough, true, 1.0) < water_rot_increment(frail, true, 1.0));
    }

    #[test]
    fn orientation_gates_growth() {
        let upright = SurvivalGenes::new(0.0, 0.0, 0.5, 0.5);
        let pendant = SurvivalGenes::new(0.0, 0.85, 0.5, 0.5);
        let creeper = SurvivalGenes::new(0.0, 0.95, 0.5, 0.5);
        assert!(permits(upright, GrowthDir::Up));
        assert!(!permits(upright, GrowthDir::Down));
        assert!(permits(pendant, GrowthDir::Down));
        assert!(!permits(pendant, GrowthDir::East));
        assert!(permits(creeper, GrowthDir::West));
    }

    #[test]
    fn support_is_opposite_the_growth_step() {
        assert_eq!(GrowthDir::Up.support_offset(), (0, -1, 0)); // held from below
        assert_eq!(GrowthDir::Down.support_offset(), (0, 1, 0)); // hangs from above
        assert_eq!(GrowthDir::East.support_offset(), (-1, 0, 0)); // braced behind
        assert!(GrowthDir::North.is_lateral());
        assert!(!GrowthDir::Up.is_lateral());
    }

    #[test]
    fn light_decides_starvation() {
        // bright: positive balance, no deficit
        let bal = energy_balance(0.9, 1.0);
        assert!(bal > 0.0);
        assert_eq!(accumulate_starvation(0.5, bal), 0.5 - bal);
        // dark: negative balance, deficit climbs to death
        let mut def = 0.0;
        let mut t = 0;
        while !is_dead(def) && t < 100_000 {
            def = accumulate_starvation(def, energy_balance(0.0, 1.0));
            t += 1;
        }
        assert!(is_dead(def));
        assert!(t > 1 && t < 1000);
    }

    #[test]
    fn surplus_light_does_not_bank_below_zero() {
        // repeated surplus keeps deficit pinned at 0, never negative
        let mut def = 0.0;
        for _ in 0..50 {
            def = accumulate_starvation(def, energy_balance(1.0, 1.0));
        }
        assert_eq!(def, 0.0);
    }

    #[test]
    fn thicker_trunks_sway_less() {
        let thin = SurvivalGenes::new(0.0, 0.0, 0.1, 0.5);
        let stout = SurvivalGenes::new(0.0, 0.0, 0.95, 0.5);
        let d_thin = tip_deflection(8.0, 10.0, thin.trunk_thickness, thin.flexibility);
        let d_stout = tip_deflection(8.0, 10.0, stout.trunk_thickness, stout.flexibility);
        assert!(d_stout < d_thin, "stout {d_stout} !< thin {d_thin}");
    }

    #[test]
    fn sway_scales_superlinearly_with_wind() {
        // drag ∝ v² ⇒ doubling wind should more than double deflection
        let d1 = tip_deflection(4.0, 10.0, 0.4, 0.5);
        let d2 = tip_deflection(8.0, 10.0, 0.4, 0.5);
        assert!(d2 > 2.0 * d1 * 0.99, "expected ~4x, got {d1} -> {d2}");
    }

    #[test]
    fn taller_and_more_flexible_sway_more() {
        assert!(tip_deflection(6.0, 20.0, 0.4, 0.5) > tip_deflection(6.0, 6.0, 0.4, 0.5));
        assert!(tip_deflection(6.0, 10.0, 0.4, 0.9) > tip_deflection(6.0, 10.0, 0.4, 0.1));
    }

    #[test]
    fn sway_is_anchored_at_base_and_grows_to_tip() {
        assert_eq!(height_profile(0.0), 0.0);
        assert!(height_profile(1.0) > height_profile(0.5));
        let g = SurvivalGenes::new(0.0, 0.0, 0.4, 0.5);
        let base = sway_offset([1.0, 0.0], 8.0, 12.0, 0.0, 3.0, g);
        assert_eq!(base, [0.0, 0.0]); // base vertex doesn't move
        let tip = sway_offset([1.0, 0.0], 8.0, 12.0, 1.0, 3.0, g);
        assert!(tip[0].abs() > 0.0);
    }

    #[test]
    fn no_wind_no_sway() {
        let g = SurvivalGenes::new(0.0, 0.0, 0.4, 0.5);
        assert_eq!(tip_deflection(0.0, 12.0, 0.4, 0.5), 0.0);
        assert_eq!(sway_offset([1.0, 0.0], 0.0, 12.0, 1.0, 3.0, g), [0.0, 0.0]);
    }

    #[test]
    fn trunk_radius_maps_gene_to_blocks() {
        assert_eq!(trunk_radius_from_gene(0.0), 1);
        assert_eq!(trunk_radius_from_gene(1.0), MAX_TRUNK_RADIUS);
        // monotonic non-decreasing
        let mut prev = 0;
        for i in 0..=10 {
            let r = trunk_radius_from_gene(i as f32 / 10.0);
            assert!(r >= prev && (1..=MAX_TRUNK_RADIUS).contains(&r));
            prev = r;
        }
    }
}
