//! `sim::legendary_weapon` — procedural legendary weapons with element procs.
//!
//! Produces deterministic weapon stat-blocks from a `(seed, class, element, tier)`
//! tuple. The renderer and hit system consume these; this module is pure logic.
//!
//! ## Element → ProcKind map (index 0..5)
//! | idx | Element | Proc         |
//! |-----|---------|--------------|
//! | 0   | Wood    | FrostNova    |
//! | 1   | Water   | WaterJet     |
//! | 2   | Air     | GaleCut      |
//! | 3   | Dust    | StoneSpikes  |
//! | 4   | Fire    | FlameWave    |
//! | 5   | Storm   | LightningArc |
//!
//! ## Splitmix64 salts (documented, R20 style)
//! base_damage_salt  = 0xDAMAGE_000_0001 = 0x0000_00DA_MAG1_0001  (conceptual)
//! actual salts used: see `SM64_SALT_*` constants below.

// ---------------------------------------------------------------------------
// splitmix64
// ---------------------------------------------------------------------------

fn sm64(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Map a sm64 output to [0, 1).
fn sm64_f32(v: u64) -> f32 {
    (v >> 40) as f32 / (1u64 << 24) as f32
}

// Salt constants (one per stat field, documented).
// Salts are arbitrary distinct constants (one per stat field).
const S_DAMAGE:   u64 = 0x0D1A_A9E7_0001_0001;
const S_SWING:    u64 = 0x5316_6600_0002_0002;
const S_REACH:    u64 = 0x4EA4_C800_0003_0003;
const S_CRIT_C:   u64 = 0xC417_0000_0004_0004;
const S_CRIT_M:   u64 = 0xC417_4000_0005_0005;
const S_PROC_POW: u64 = 0x7D0C_9100_0006_0006;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Weapon class archetype.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeaponClass {
    Dagger,
    Blade,
    Greatsword,
    Axe,
    Greataxe,
    Spear,
    Staff,
    Wand,
}

impl WeaponClass {
    /// All weapon classes (r332-swords-creative: enables item::legendary_palette()).
    pub const ALL: [WeaponClass; 8] = [
        WeaponClass::Dagger, WeaponClass::Blade, WeaponClass::Greatsword, WeaponClass::Axe,
        WeaponClass::Greataxe, WeaponClass::Spear, WeaponClass::Staff, WeaponClass::Wand,
    ];
}

/// Element-tied signature proc effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcKind {
    None,
    LightningArc,
    WaterJet,
    FlameWave,
    FrostNova,
    StoneSpikes,
    GaleCut,
}

/// A legendary weapon stat-block (pure data; no mutation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weapon {
    pub class:       WeaponClass,
    pub element_idx: u8,
    pub tier:        u8,
    pub base_damage: f32,
    pub swing_time:  f32,
    pub reach:       f32,
    pub crit_chance: f32,
    pub crit_mult:   f32,
    pub proc:        ProcKind,
    pub proc_power:  f32,
}

// ---------------------------------------------------------------------------
// Archetype tables  (base_damage, swing_time, reach, crit_chance, crit_mult, proc_power)
// each is a (min, max) range; `forge` lerps a seeded jitter within it.
// ---------------------------------------------------------------------------

struct Arch {
    dmg:   (f32, f32),
    swing: (f32, f32),
    reach: (f32, f32),
    crit_c:(f32, f32),
    crit_m:(f32, f32),
    proc_p:(f32, f32),
}

fn archetype(c: WeaponClass) -> Arch {
    match c {
        // Fast, low-reach, high crit, low-ish damage
        WeaponClass::Dagger    => Arch { dmg:(4.,8.),  swing:(0.4,0.6), reach:(0.8,1.2), crit_c:(0.20,0.30), crit_m:(1.8,2.5), proc_p:(0.4,0.7) },
        // Balanced
        WeaponClass::Blade     => Arch { dmg:(8.,16.), swing:(0.6,0.9), reach:(1.2,1.8), crit_c:(0.12,0.20), crit_m:(1.5,2.0), proc_p:(0.5,0.9) },
        // Slow, high damage, long reach
        WeaponClass::Greatsword=> Arch { dmg:(18.,30.),swing:(1.2,1.8), reach:(2.0,3.0), crit_c:(0.08,0.15), crit_m:(1.6,2.2), proc_p:(0.6,1.0) },
        // Heavy, high damage, short reach
        WeaponClass::Axe       => Arch { dmg:(12.,22.),swing:(0.8,1.2), reach:(1.0,1.5), crit_c:(0.10,0.18), crit_m:(1.7,2.3), proc_p:(0.5,0.8) },
        // Very heavy, very high damage
        WeaponClass::Greataxe  => Arch { dmg:(25.,45.),swing:(1.5,2.2), reach:(1.2,1.8), crit_c:(0.06,0.12), crit_m:(1.8,2.8), proc_p:(0.5,0.9) },
        // Long reach, moderate speed
        WeaponClass::Spear     => Arch { dmg:(10.,18.),swing:(0.7,1.0), reach:(2.5,4.0), crit_c:(0.10,0.18), crit_m:(1.5,2.0), proc_p:(0.5,0.8) },
        // Low base damage, very high proc power
        WeaponClass::Staff     => Arch { dmg:(3.,7.),  swing:(0.9,1.3), reach:(1.5,2.5), crit_c:(0.08,0.14), crit_m:(1.4,1.8), proc_p:(1.5,2.5) },
        // Fastest, lowest damage, highest proc power
        WeaponClass::Wand      => Arch { dmg:(1.,4.),  swing:(0.3,0.5), reach:(0.5,1.0), crit_c:(0.15,0.25), crit_m:(1.5,2.0), proc_p:(2.0,3.5) },
    }
}

/// Tier multiplier: scales damage, proc_power, and (slightly) crit.
fn tier_mul(tier: u8) -> f32 {
    1.0 + tier as f32 * 0.35
}

// ---------------------------------------------------------------------------
// Element → ProcKind
// ---------------------------------------------------------------------------

fn element_to_proc(element_idx: u8) -> ProcKind {
    match element_idx % 6 {
        0 => ProcKind::FrostNova,
        1 => ProcKind::WaterJet,
        2 => ProcKind::GaleCut,
        3 => ProcKind::StoneSpikes,
        4 => ProcKind::FlameWave,
        5 => ProcKind::LightningArc,
        _ => ProcKind::None, // unreachable, but exhaustive
    }
}

// ---------------------------------------------------------------------------
// forge
// ---------------------------------------------------------------------------

/// Forge a legendary weapon deterministically from `(seed, class, element, tier)`.
///
/// - `element_idx` taken mod 6 — never panics on out-of-range.
/// - `tier` is uncapped; very high tiers produce very large stats.
/// - Stats are monotone non-decreasing in `tier` (proven by tier_mul > 0).
/// - Splitmix64 salts are XORed with `seed` per-field for independence.
pub fn forge(seed: u64, class: WeaponClass, element_idx: u8, tier: u8) -> Weapon {
    let arch = archetype(class);
    let tm   = tier_mul(tier);

    let lerp = |lo: f32, hi: f32, t: f32| lo + (hi - lo) * t;

    // Per-field random jitter in [0,1) using distinct salts.
    let t_dmg   = sm64_f32(sm64(seed ^ S_DAMAGE));
    let t_swing = sm64_f32(sm64(seed ^ S_SWING));
    let t_reach = sm64_f32(sm64(seed ^ S_REACH));
    let t_crit_c= sm64_f32(sm64(seed ^ S_CRIT_C));
    let t_crit_m= sm64_f32(sm64(seed ^ S_CRIT_M));
    let t_proc_p= sm64_f32(sm64(seed ^ S_PROC_POW));

    Weapon {
        class,
        element_idx: element_idx % 6,
        tier,
        base_damage: lerp(arch.dmg.0,   arch.dmg.1,   t_dmg)   * tm,
        swing_time:  lerp(arch.swing.0,  arch.swing.1, t_swing), // intentionally NOT scaled by tier (fast weapons stay fast)
        reach:       lerp(arch.reach.0,  arch.reach.1, t_reach),
        crit_chance: (lerp(arch.crit_c.0, arch.crit_c.1, t_crit_c)).clamp(0.0, 1.0),
        crit_mult:   lerp(arch.crit_m.0,  arch.crit_m.1, t_crit_m),
        proc:        element_to_proc(element_idx),
        proc_power:  lerp(arch.proc_p.0,  arch.proc_p.1, t_proc_p) * tm,
    }
}

// ---------------------------------------------------------------------------
// Proc geometry helpers
// ---------------------------------------------------------------------------

/// Vector length squared.
#[inline]
fn len_sq(v: [f32; 3]) -> f32 { v[0]*v[0] + v[1]*v[1] + v[2]*v[2] }

/// Vector length, 0 if degenerate.
#[inline]
fn len(v: [f32; 3]) -> f32 { len_sq(v).max(0.0).sqrt() }

/// Dot product.
#[inline]
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 { a[0]*b[0] + a[1]*b[1] + a[2]*b[2] }

/// Subtract two 3-vectors.
#[inline]
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[0]-b[0], a[1]-b[1], a[2]-b[2]] }

/// Greedy nearest-unused chain (lightning arc).
///
/// Starting from `origin`, greedily picks the nearest unused candidate
/// within `max_hop` distance, chains up to `max_links` hops, no index repeats.
/// Returns the indices of chained targets in hop order.
/// Empty `cands` → empty result. Degenerate inputs never panic.
pub fn chain_targets(
    origin: [f32; 3],
    cands:  &[[f32; 3]],
    max_links: u8,
    max_hop:   f32,
) -> Vec<usize> {
    if cands.is_empty() || max_links == 0 {
        return Vec::new();
    }
    let max_hop_sq = (max_hop.max(0.0)) * (max_hop.max(0.0));
    let mut used = vec![false; cands.len()];
    let mut result = Vec::with_capacity(max_links as usize);
    let mut current = origin;

    for _ in 0..max_links {
        let mut best_i  = None;
        let mut best_d2 = f32::INFINITY;
        for (i, &pos) in cands.iter().enumerate() {
            if used[i] { continue; }
            let d2 = len_sq(sub(pos, current));
            if d2 <= max_hop_sq && d2 < best_d2 {
                best_d2 = d2;
                best_i  = Some(i);
            }
        }
        match best_i {
            None    => break,
            Some(i) => {
                used[i] = true;
                result.push(i);
                current = cands[i];
            }
        }
    }
    result
}

/// Cone / jet hits (water jet, breath cone).
///
/// Returns indices of candidates within `range` AND inside the cone defined by
/// `dir` and `half_angle_cos` (dot(normalize(cand-origin), normalize(dir)) ≥ half_angle_cos).
///
/// - Degenerate `dir` (zero vector) → empty (no NaN propagated).
/// - `half_angle_cos` outside [-1,1] is clamped.
/// - Deterministic; no allocation beyond the output Vec.
pub fn jet_hits(
    origin:         [f32; 3],
    dir:            [f32; 3],
    cands:          &[[f32; 3]],
    range:          f32,
    half_angle_cos: f32,
) -> Vec<usize> {
    let dir_len = len(dir);
    if dir_len < 1e-9 {
        return Vec::new();
    }
    let dir_n = [dir[0]/dir_len, dir[1]/dir_len, dir[2]/dir_len];
    let range_sq = (range.max(0.0)) * (range.max(0.0));
    let cos_thresh = half_angle_cos.clamp(-1.0, 1.0);

    let mut hits = Vec::new();
    for (i, &pos) in cands.iter().enumerate() {
        let to = sub(pos, origin);
        let d2 = len_sq(to);
        if d2 > range_sq { continue; }
        let to_len = d2.max(0.0).sqrt();
        if to_len < 1e-9 {
            // Target is at the origin — include it (inside any cone).
            hits.push(i);
            continue;
        }
        let to_n = [to[0]/to_len, to[1]/to_len, to[2]/to_len];
        let cos_angle = dot(to_n, dir_n);
        if cos_angle >= cos_thresh {
            hits.push(i);
        }
    }
    hits
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Swing resolution — compose a forged weapon's stats into one attack outcome.
// ---------------------------------------------------------------------------

/// The result of one weapon swing: the direct hit plus any signature-proc effect.
#[derive(Clone, Debug, PartialEq)]
pub struct SwingOutcome {
    /// Direct damage to the primary target, post-crit, `>= 0`.
    pub damage: f32,
    /// Whether the direct hit critically struck.
    pub crit: bool,
    /// For a `LightningArc` weapon: indices into `nearby` the bolt chained
    /// through, in hop order. Empty for other procs or when nothing is in range.
    pub arc_targets: Vec<usize>,
    /// Damage each arced target takes (`base_damage * proc_power`); `0` if no arc.
    pub arc_damage: f32,
}

/// Resolve a single swing of `w`. The direct hit is the weapon's `base_damage`
/// scaled by a crit (`crit_roll < crit_chance` ⇒ ×`crit_mult`). A `LightningArc`
/// weapon (Storm element) additionally arcs from `origin` through up to
/// `arc_links` of the `nearby` foe positions within `arc_hop` (greedy nearest,
/// no repeats — `chain_targets`), each taking `base_damage * proc_power`. Other
/// procs leave the arc empty (their own resolvers — e.g. `jet_hits` — handle
/// them). Pure and deterministic in `(w, crit_roll, origin, nearby)`; this is the
/// piece that turns a forged weapon into a concrete attack — e.g. a lightning
/// sword (`forge(seed, WeaponClass::Blade, 5, tier)`) or an axe
/// (`forge(seed, WeaponClass::Axe, .., tier)`).
pub fn resolve_weapon_swing(
    w: &Weapon,
    crit_roll: f32,
    origin: [f32; 3],
    nearby: &[[f32; 3]],
    arc_links: u8,
    arc_hop: f32,
) -> SwingOutcome {
    let crit = crit_roll < w.crit_chance;
    let mult = if crit { w.crit_mult.max(1.0) } else { 1.0 };
    let damage = (w.base_damage.max(0.0) * mult).max(0.0);

    let (arc_targets, arc_damage) = match w.proc {
        ProcKind::LightningArc => {
            let t = chain_targets(origin, nearby, arc_links, arc_hop);
            let d = (w.base_damage.max(0.0) * w.proc_power.max(0.0)).max(0.0);
            (t, d)
        }
        _ => (Vec::new(), 0.0),
    };

    SwingOutcome { damage, crit, arc_targets, arc_damage }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- forge -------------------------------------------------------------

    #[test]
    fn forge_deterministic() {
        let a = forge(0xDEAD_BEEF, WeaponClass::Blade, 4, 2);
        let b = forge(0xDEAD_BEEF, WeaponClass::Blade, 4, 2);
        assert_eq!(a.base_damage.to_bits(), b.base_damage.to_bits());
        assert_eq!(a.proc_power.to_bits(),  b.proc_power.to_bits());
        assert_eq!(a.proc, b.proc);
    }

    #[test]
    fn forge_damage_monotone_in_tier() {
        for seed in [0u64, 42, 0xABCD_EF01] {
            let mut prev_dmg = 0.0f32;
            let mut prev_pp  = 0.0f32;
            for tier in 0u8..10 {
                let w = forge(seed, WeaponClass::Greatsword, 4, tier);
                assert!(w.base_damage >= prev_dmg - 1e-4,
                    "base_damage not non-decreasing at tier {tier}: {prev_dmg} -> {}", w.base_damage);
                assert!(w.proc_power >= prev_pp - 1e-4,
                    "proc_power not non-decreasing at tier {tier}");
                prev_dmg = w.base_damage;
                prev_pp  = w.proc_power;
            }
        }
    }

    #[test]
    fn greataxe_outdamages_dagger() {
        let seed = 0x1234_5678;
        let axe = forge(seed, WeaponClass::Greataxe, 0, 1);
        let dag = forge(seed, WeaponClass::Dagger,   0, 1);
        assert!(axe.base_damage > dag.base_damage,
            "Greataxe ({}) should out-damage Dagger ({})", axe.base_damage, dag.base_damage);
    }

    #[test]
    fn staff_higher_proc_power_and_lower_damage_than_greatsword() {
        let seed = 0xCAFE_BABE;
        let staff = forge(seed, WeaponClass::Staff,      4, 2);
        let gs    = forge(seed, WeaponClass::Greatsword, 4, 2);
        assert!(staff.proc_power > gs.proc_power,
            "Staff proc_power ({}) should exceed Greatsword ({})", staff.proc_power, gs.proc_power);
        assert!(staff.base_damage < gs.base_damage,
            "Staff damage ({}) should be less than Greatsword ({})", staff.base_damage, gs.base_damage);
    }

    #[test]
    fn element_proc_mapping() {
        let pairs = [
            (0u8, ProcKind::FrostNova),
            (1,   ProcKind::WaterJet),
            (2,   ProcKind::GaleCut),
            (3,   ProcKind::StoneSpikes),
            (4,   ProcKind::FlameWave),
            (5,   ProcKind::LightningArc),
        ];
        for (idx, expected) in pairs {
            let w = forge(0, WeaponClass::Wand, idx, 0);
            assert_eq!(w.proc, expected, "element {idx} should map to {:?}", expected);
        }
    }

    #[test]
    fn out_of_range_element_clamps_no_panic() {
        let w = forge(0, WeaponClass::Blade, 255, 0);
        assert_ne!(w.proc, ProcKind::None, "element 255 should map to a valid proc");
        assert!(w.base_damage.is_finite());
    }

    #[test]
    fn all_stats_finite_and_positive() {
        let classes = [WeaponClass::Dagger, WeaponClass::Staff, WeaponClass::Greataxe];
        for class in classes {
            let w = forge(0xABCD_EF01, class, 3, 5);
            assert!(w.base_damage.is_finite() && w.base_damage > 0.0);
            assert!(w.swing_time.is_finite()  && w.swing_time  > 0.0);
            assert!(w.reach.is_finite()       && w.reach       > 0.0);
            assert!(w.crit_chance.is_finite() && w.crit_chance >= 0.0);
            assert!(w.crit_mult.is_finite()   && w.crit_mult   >= 1.0);
            assert!(w.proc_power.is_finite()  && w.proc_power  > 0.0);
        }
    }

    // ---- chain_targets -----------------------------------------------------

    #[test]
    fn chain_targets_nearest_first() {
        let origin = [0.0f32, 0.0, 0.0];
        let cands = vec![[5.0,0.0,0.0], [1.0,0.0,0.0], [3.0,0.0,0.0]];
        let chain = chain_targets(origin, &cands, 3, 10.0);
        assert_eq!(chain[0], 1, "nearest (index 1, dist 1) should be first hop");
    }

    #[test]
    fn chain_targets_respects_max_links() {
        let origin = [0.0f32; 3];
        let cands: Vec<[f32;3]> = (1..=10).map(|i| [i as f32, 0.0, 0.0]).collect();
        let chain = chain_targets(origin, &cands, 3, 100.0);
        assert_eq!(chain.len(), 3, "should stop at max_links=3");
    }

    #[test]
    fn chain_targets_respects_max_hop() {
        let origin = [0.0f32; 3];
        let cands = vec![[1.0,0.0,0.0], [5.0,0.0,0.0]]; // index 1 is out of hop range
        let chain = chain_targets(origin, &cands, 5, 2.0);
        assert!(!chain.contains(&1), "far target should be excluded by max_hop");
    }

    #[test]
    fn chain_targets_no_repeats() {
        let origin = [0.0f32; 3];
        let cands = vec![[1.0,0.0,0.0], [2.0,0.0,0.0], [3.0,0.0,0.0]];
        let chain = chain_targets(origin, &cands, 10, 100.0);
        let unique: std::collections::HashSet<usize> = chain.iter().copied().collect();
        assert_eq!(unique.len(), chain.len(), "no index repeats");
    }

    #[test]
    fn chain_targets_empty_cands() {
        let chain = chain_targets([0.0;3], &[], 5, 10.0);
        assert!(chain.is_empty());
    }

    // ---- jet_hits ----------------------------------------------------------

    #[test]
    fn jet_hits_excludes_out_of_range() {
        let hits = jet_hits([0.0;3], [1.0,0.0,0.0], &[[100.0,0.0,0.0]], 10.0, 1.0);
        assert!(hits.is_empty(), "out-of-range target should be excluded");
    }

    #[test]
    fn jet_hits_excludes_behind_dir() {
        // Target is directly behind the direction vector.
        let hits = jet_hits([0.0;3], [1.0,0.0,0.0], &[[-1.0,0.0,0.0]], 5.0, 0.0);
        assert!(hits.is_empty(), "target behind dir should be excluded");
    }

    #[test]
    fn jet_hits_includes_in_cone() {
        // Target is within range and within a 90° half-angle cone (cos=0).
        let hits = jet_hits([0.0;3], [1.0,0.0,0.0], &[[1.0,0.5,0.0]], 10.0, 0.0);
        assert_eq!(hits, vec![0], "target in cone should be included");
    }

    #[test]
    fn jet_hits_degenerate_dir_returns_empty() {
        let hits = jet_hits([0.0;3], [0.0,0.0,0.0], &[[1.0,0.0,0.0]], 10.0, 0.0);
        assert!(hits.is_empty(), "zero dir should return empty, no NaN");
    }

    #[test]
    fn jet_hits_all_in_wide_cone() {
        let cands = vec![[1.0,0.0,0.0], [0.0,1.0,0.0], [0.0,0.0,1.0]];
        // cos(-1) = fully open cone → includes everything in range.
        let hits = jet_hits([0.0;3], [1.0,0.0,0.0], &cands, 100.0, -1.0);
        assert_eq!(hits.len(), 3, "fully open cone should hit all in range");
    }

    #[test]
    fn jet_hits_narrow_cone_excludes_off_axis() {
        let cands = vec![[1.0,0.0,0.0], [0.0,1.0,0.0]];
        // Very narrow cone: only the on-axis target should be hit.
        let hits = jet_hits([0.0;3], [1.0,0.0,0.0], &cands, 10.0, 0.99);
        assert!(hits.contains(&0), "on-axis target should be in narrow cone");
        assert!(!hits.contains(&1), "off-axis target should be excluded");
    }

    #[test]
    fn chain_and_jet_no_nan() {
        let origin = [1.0f32, 2.0, 3.0];
        let cands = vec![[1.5,2.0,3.0], [2.0,3.0,4.0], [0.0,0.0,0.0]];
        for idx in chain_targets(origin, &cands, 5, 5.0) {
            let pos = cands[idx];
            assert!(pos[0].is_finite() && pos[1].is_finite() && pos[2].is_finite());
        }
        for idx in jet_hits(origin, [1.0,0.0,0.0], &cands, 5.0, 0.0) {
            let pos = cands[idx];
            assert!(pos[0].is_finite() && pos[1].is_finite() && pos[2].is_finite());
        }
    }

    // ---- swing resolution --------------------------------------------------

    #[test]
    fn lightning_sword_swing_hits_and_arcs() {
        // Storm element (5) ⇒ LightningArc proc — a "lightning sword".
        let sword = forge(0x1161, WeaponClass::Blade, 5, 3);
        assert_eq!(sword.proc, ProcKind::LightningArc);
        let foes = [[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [3.5, 0.0, 0.0]];
        let out = resolve_weapon_swing(&sword, 1.0, [0.0, 0.0, 0.0], &foes, 4, 1.6);
        assert!(out.damage > 0.0 && !out.crit);
        assert!(!out.arc_targets.is_empty(), "lightning sword arcs to nearby foes");
        assert!(out.arc_damage > 0.0);
        let mut seen = out.arc_targets.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), out.arc_targets.len(), "no repeated arc targets");
    }

    #[test]
    fn axe_swing_is_heavy_melee_without_arc() {
        // Axe with a non-Storm element ⇒ no LightningArc.
        let axe = forge(0xA7E, WeaponClass::Axe, 0, 3); // Wood ⇒ FrostNova
        assert_ne!(axe.proc, ProcKind::LightningArc);
        let out = resolve_weapon_swing(&axe, 1.0, [0.0, 0.0, 0.0], &[[1.0, 0.0, 0.0]], 4, 2.0);
        assert!(out.arc_targets.is_empty() && out.arc_damage == 0.0, "non-storm weapon doesn't arc");
        assert!(out.damage > 0.0);
    }

    #[test]
    fn crit_roll_below_chance_amplifies_damage() {
        let sword = forge(0xC07, WeaponClass::Blade, 5, 5);
        let no_crit = resolve_weapon_swing(&sword, 1.0, [0.0; 3], &[], 4, 2.0);
        let crit = resolve_weapon_swing(&sword, 0.0, [0.0; 3], &[], 4, 2.0);
        assert!(crit.crit && !no_crit.crit);
        assert!(crit.damage > no_crit.damage);
        assert!((crit.damage - no_crit.damage * sword.crit_mult).abs() < 1e-2, "crit applies crit_mult");
    }

    #[test]
    fn swing_is_deterministic() {
        let w = forge(7, WeaponClass::Greataxe, 5, 4);
        let foes = [[1.0, 0.0, 0.0], [2.0, 1.0, 0.0]];
        assert_eq!(
            resolve_weapon_swing(&w, 0.5, [0.0; 3], &foes, 3, 2.0),
            resolve_weapon_swing(&w, 0.5, [0.0; 3], &foes, 3, 2.0),
        );
    }
}
