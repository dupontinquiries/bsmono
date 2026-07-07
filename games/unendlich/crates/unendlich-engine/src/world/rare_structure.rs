//! `world::rare_structure` — rarity-gated, per-region selector for **rare**
//! structures (obelisks, shrines, ruins, monuments).
//!
//! WID: `r332-structures`. The structure machinery already exists —
//! `world::structure_grammar` (`expand`/`to_placements`) stamps a footprint from
//! an L-system axiom, and `world::structure_place` (`suitability`/`rank_sites`)
//! scores cells — but nothing decides **which** rare structure appears in a
//! region and **how rarely**. This module is that decision: a pure,
//! deterministic function worldgen calls once per region.
//!
//! ## What it guarantees
//! - **Deterministic:** output depends only on `(region, world_seed, biome,
//!   table)`. No IO/clock/RNG (splitmix64 over the inputs, the same mix the
//!   generator and `legendary_weapon` use). No `unwrap`/`expect`/`panic!`.
//!   Self-contained → gates with `rustc --edition 2021 --test`.
//! - **Min spacing (exact):** each kind's host regions lie on a seeded
//!   sublattice of period `min_spacing`, so any two regions hosting the *same*
//!   kind are at least `min_spacing` apart (Chebyshev). Spacing is enforced by
//!   construction, not by scanning neighbours.
//! - **Rarity:** even a spacing-anchored, biome-allowed region only hosts with
//!   probability `region_rate`, so rares stay sparse. Among the kinds anchored
//!   at a region, one is chosen by `weight`.
//! - **One rare per region:** a region yields at most one `RarePlacement`.
//!
//! ## Wiring (proposed in `HANDOFF_rare_structures.md`, not done here)
//! Worldgen → `select_for_region` → on `Some`, `structure_grammar::expand(kind
//! .axiom(), rules, iters)` + `to_placements(.., seed)` to lay the footprint at
//! `origin` rotated by `rotation`, optionally refined by `structure_place::
//! rank_sites` for the best cell. The bin owns those calls.
#![allow(dead_code)]

// ── splitmix64 (same constants as the generator / `legendary_weapon`) ────────
fn sm64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Deterministic hash of a region + seed + salt. Sign-preserving on the i32s.
fn region_hash(region: [i32; 2], world_seed: u64, salt: u64) -> u64 {
    let mut h = sm64(world_seed ^ salt);
    h = sm64(h ^ (region[0] as i64 as u64));
    h = sm64(h ^ (region[1] as i64 as u64).rotate_left(32));
    sm64(h)
}

/// Map a hash to [0, 1) using its top 24 bits (matches `legendary_weapon`).
fn f01(h: u64) -> f32 {
    (h >> 40) as f32 / (1u64 << 24) as f32
}

// Per-decision salts (independent streams).
const SALT_PHASE_X: u64 = 0x5A17_0000_0001_0001; // kind lattice phase, x
const SALT_PHASE_Z: u64 = 0x5A17_0000_0002_0002; // kind lattice phase, z
const SALT_RATE: u64 = 0x5A17_0000_0003_0003; // overall rarity roll
const SALT_PICK: u64 = 0x5A17_0000_0004_0004; // weighted kind pick
const SALT_OFF_X: u64 = 0x5A17_0000_0005_0005; // within-region offset, x
const SALT_OFF_Z: u64 = 0x5A17_0000_0006_0006; // within-region offset, z
const SALT_ROT: u64 = 0x5A17_0000_0007_0007; // rotation

// ── Types ────────────────────────────────────────────────────────────────────

/// A rare structure type. Each is keyed to an L-system axiom the grammar
/// expands (`structure_grammar::expand(kind.axiom(), ..)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RareKind {
    /// A single tall marker — a short straight footprint.
    Obelisk,
    /// A small symmetric sacred site — a cross/plus footprint.
    Shrine,
    /// A broken sprawling layout — a branching axiom.
    Ruin,
    /// A large deliberate edifice — a ringed/square footprint.
    Monument,
    // r336-poi-structure: more POI/structure kinds.
    /// A tall slender spire — a long straight footprint.
    Tower,
    /// A ring of standing stones — a closed polygon.
    Henge,
    /// A carved figure on a plinth — a compact stepped footprint.
    Statue,
    /// A stone well/cistern — a tiny ringed footprint.
    Well,
}

impl RareKind {
    pub const ALL: [RareKind; 8] = [
        RareKind::Obelisk, RareKind::Shrine, RareKind::Ruin, RareKind::Monument,
        RareKind::Tower, RareKind::Henge, RareKind::Statue, RareKind::Well,
    ];

    /// Stable index 0..4 (used to decorrelate each kind's lattice stream).
    pub fn index(self) -> u64 {
        match self {
            RareKind::Obelisk => 0,
            RareKind::Shrine => 1,
            RareKind::Ruin => 2,
            RareKind::Monument => 3,
            RareKind::Tower => 4,
            RareKind::Henge => 5,
            RareKind::Statue => 6,
            RareKind::Well => 7,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            RareKind::Obelisk => "Obelisk",
            RareKind::Shrine => "Shrine",
            RareKind::Ruin => "Ruin",
            RareKind::Monument => "Monument",
            RareKind::Tower => "Tower",
            RareKind::Henge => "Henge",
            RareKind::Statue => "Statue",
            RareKind::Well => "Well",
        }
    }

    /// The L-system axiom this kind stamps via `structure_grammar`. Turtle
    /// commands: `F`/`G` place+step, `+`/`-` turn. Kept small and deterministic;
    /// the grammar caps expansion length, so these never blow up.
    pub fn axiom(self) -> &'static str {
        match self {
            RareKind::Obelisk => "F",            // a point/short pillar
            RareKind::Shrine => "F+F+F+F",       // a small closed square
            RareKind::Ruin => "F+F-F-F+F",       // a jagged broken run
            RareKind::Monument => "F+F+F+F+F+F+F+F", // a larger ringed footprint
            RareKind::Tower => "FF",                 // a tall slender spire
            RareKind::Henge => "F+F+F+F+F+F",        // a hexagonal stone ring
            RareKind::Statue => "F+F",               // a compact stepped plinth
            RareKind::Well => "F+F+F+F",             // a tiny square cistern
        }
    }
}

/// One row of the rarity table: a kind, its relative weight, its min spacing (in
/// regions), and the biomes it may appear in (a bitmask over biome u8 ids).
#[derive(Clone, Copy, Debug)]
pub struct RareEntry {
    pub kind: RareKind,
    /// Relative spawn weight among kinds anchored at the same region (0 disables).
    pub weight: u32,
    /// Minimum regions between two occurrences of THIS kind (>=1; 0 treated as 1).
    pub min_spacing: u32,
    /// Bit `i` set ⇒ biome with `Biome as u8 == i` is allowed. Build with
    /// [`biomes_mask`]. (Biome ids 0..24 fit in a u32.)
    pub allowed_biomes: u32,
}

/// The rarity configuration for a world.
#[derive(Clone, Debug)]
pub struct RarityTable {
    /// Probability in [0,1] that an anchored, biome-allowed region actually
    /// hosts a rare. Lower = rarer. Clamped on use.
    pub region_rate: f32,
    /// Side length (in cells) of a region's within-structure offset space; the
    /// chosen `origin` lands in `0..region_span` on each axis. `<=1` ⇒ origin
    /// is `[0,0]`.
    pub region_span: i32,
    /// The kinds in play. Order is irrelevant to the result.
    pub entries: Vec<RareEntry>,
}

/// The selector's output: which rare, where (within-region offset), how rotated,
/// and the axiom to expand. `Copy` and small.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RarePlacement {
    pub kind: RareKind,
    /// Within-region offset, each component in `0..region_span` (`[0,0]` if span <= 1).
    pub origin: [i32; 2],
    /// Quarter-turn rotation, 0..4 (matches the grammar's 4-direction turtle).
    pub rotation: u8,
    /// The L-system axiom for `kind` (`structure_grammar::expand`).
    pub axiom: &'static str,
}

// ── Mask helpers ──────────────────────────────────────────────────────────────

/// Set the bit for one biome id. Ids >= 32 are ignored (out of mask range).
pub fn biome_bit(id: u8) -> u32 {
    if id < 32 {
        1u32 << id
    } else {
        0
    }
}

/// Build an allowed-biomes mask from a list of biome ids.
pub fn biomes_mask(ids: &[u8]) -> u32 {
    let mut m = 0u32;
    for &id in ids {
        m |= biome_bit(id);
    }
    m
}

// ── Spacing lattice ───────────────────────────────────────────────────────────

/// Is `region` the spacing-anchor for `kind` at spacing `s`? Each kind gets a
/// seeded phase in `0..s` on each axis; host regions are exactly those on that
/// sublattice, so any two same-kind hosts are >= `s` apart (Chebyshev).
fn is_spacing_anchor(region: [i32; 2], s: u32, world_seed: u64, kind_idx: u64) -> bool {
    if s <= 1 {
        return true; // no spacing constraint
    }
    let s_i = s as i32;
    let phase_x = (sm64(world_seed ^ SALT_PHASE_X ^ kind_idx) % s as u64) as i32;
    let phase_z = (sm64(world_seed ^ SALT_PHASE_Z ^ kind_idx) % s as u64) as i32;
    region[0].rem_euclid(s_i) == phase_x && region[1].rem_euclid(s_i) == phase_z
}

// ── The selector ──────────────────────────────────────────────────────────────

/// Decide the rare structure (if any) for `region`. Deterministic, pure,
/// alloc-free, panic-free.
///
/// Steps: (1) gather kinds that are biome-allowed AND spacing-anchored at this
/// region; (2) overall rarity gate (`region_rate`); (3) weighted pick among the
/// anchored kinds; (4) seeded within-region offset + rotation.
pub fn select_for_region(
    region: [i32; 2],
    world_seed: u64,
    biome: u8,
    table: &RarityTable,
) -> Option<RarePlacement> {
    // Pass 1: total weight of eligible (biome-allowed + spacing-anchored) kinds.
    let mut total_weight: u64 = 0;
    for e in &table.entries {
        if e.weight == 0 {
            continue;
        }
        if (e.allowed_biomes >> (biome as u32 & 31)) & 1 == 0 {
            continue;
        }
        if !is_spacing_anchor(region, e.min_spacing.max(1), world_seed, e.kind.index()) {
            continue;
        }
        total_weight = total_weight.saturating_add(e.weight as u64);
    }
    if total_weight == 0 {
        return None; // nothing eligible here
    }

    // Step 2: overall rarity gate.
    let rate = if table.region_rate.is_nan() {
        0.0
    } else {
        table.region_rate.clamp(0.0, 1.0)
    };
    if rate <= 0.0 {
        return None;
    }
    if rate < 1.0 && f01(region_hash(region, world_seed, SALT_RATE)) >= rate {
        return None;
    }

    // Step 3: weighted pick among the eligible kinds (second pass, alloc-free).
    let pick = region_hash(region, world_seed, SALT_PICK) % total_weight;
    let mut acc: u64 = 0;
    let mut chosen: Option<RareEntry> = None;
    for e in &table.entries {
        if e.weight == 0 {
            continue;
        }
        if (e.allowed_biomes >> (biome as u32 & 31)) & 1 == 0 {
            continue;
        }
        if !is_spacing_anchor(region, e.min_spacing.max(1), world_seed, e.kind.index()) {
            continue;
        }
        acc += e.weight as u64;
        if pick < acc {
            chosen = Some(*e);
            break;
        }
    }
    let entry = chosen?; // total_weight > 0 guarantees this is Some

    // Step 4: within-region offset + rotation.
    let span = table.region_span;
    let origin = if span <= 1 {
        [0, 0]
    } else {
        let ox = (region_hash(region, world_seed, SALT_OFF_X) % span as u64) as i32;
        let oz = (region_hash(region, world_seed, SALT_OFF_Z) % span as u64) as i32;
        [ox, oz]
    };
    let rotation = (region_hash(region, world_seed, SALT_ROT) % 4) as u8;

    Some(RarePlacement { kind: entry.kind, origin, rotation, axiom: entry.kind.axiom() })
}

// ── A reasonable default table (documented biome ids from `world::biomes`) ────

impl Default for RarityTable {
    /// A sane starting table: rares are uncommon (~6% of anchored regions),
    /// well-spaced, and biome-gated to where each reads right. Biome ids match
    /// `world::biomes::Biome as u8` (Plains=0, TemperateForest=1, …, Desert=8,
    /// RedSandstoneCanyon=9, Ocean=11, Swamp=13, Tundra=14, Savanna=19,
    /// Jungle=20, Taiga=21, …). Tune on the bin side.
    fn default() -> Self {
        RarityTable {
            region_rate: 0.06,
            region_span: 16,
            entries: vec![
                // Obelisks: open dry lands, widely spaced.
                RareEntry {
                    kind: RareKind::Obelisk,
                    weight: 3,
                    min_spacing: 12,
                    allowed_biomes: biomes_mask(&[0, 8, 9, 14, 19]), // plains/desert/canyon/tundra/savanna
                },
                // Shrines: forests + jungle, fairly rare.
                RareEntry {
                    kind: RareKind::Shrine,
                    weight: 2,
                    min_spacing: 10,
                    allowed_biomes: biomes_mask(&[1, 3, 4, 5, 20, 21]), // forests/jungle/taiga
                },
                // Ruins: common-ish among rares, many biomes.
                RareEntry {
                    kind: RareKind::Ruin,
                    weight: 4,
                    min_spacing: 8,
                    allowed_biomes: biomes_mask(&[0, 1, 8, 13, 19, 21]),
                },
                // Monuments: the rarest, most spaced; highlands/oceans.
                RareEntry {
                    kind: RareKind::Monument,
                    weight: 1,
                    min_spacing: 24,
                    allowed_biomes: biomes_mask(&[2, 6, 7, 11, 15]), // highlands/mountains/ocean/skylands
                },
                // r336-poi-structure: new kinds.
                RareEntry { kind: RareKind::Tower, weight: 3, min_spacing: 10,
                    allowed_biomes: biomes_mask(&[0, 2, 7, 19, 21]) }, // open/highland
                RareEntry { kind: RareKind::Henge, weight: 1, min_spacing: 18,
                    allowed_biomes: biomes_mask(&[0, 14, 19]) },       // plains/tundra/savanna, very rare
                RareEntry { kind: RareKind::Statue, weight: 2, min_spacing: 12,
                    allowed_biomes: biomes_mask(&[1, 3, 4, 5, 19, 20]) }, // forests/jungle/savanna
                RareEntry { kind: RareKind::Well, weight: 4, min_spacing: 7,
                    allowed_biomes: biomes_mask(&[0, 8, 9, 13, 19, 25]) }, // dry + swamp + mangrove
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A small fully-permissive table for focused tests: every biom id 0..24 allowed.
    fn all_biomes() -> u32 {
        let mut m = 0u32;
        for id in 0u8..24 {
            m |= biome_bit(id);
        }
        m
    }

    fn one_kind(kind: RareKind, weight: u32, spacing: u32, rate: f32) -> RarityTable {
        RarityTable {
            region_rate: rate,
            region_span: 16,
            entries: vec![RareEntry { kind, weight, min_spacing: spacing, allowed_biomes: all_biomes() }],
        }
    }

    #[test]
    fn deterministic_for_a_region_seed() {
        let t = RarityTable::default();
        for &r in &[[0, 0], [3, -7], [128, 256], [-99, 5]] {
            let a = select_for_region(r, 0xC0FFEE, 0, &t);
            let b = select_for_region(r, 0xC0FFEE, 0, &t);
            assert_eq!(a, b, "non-deterministic at {r:?}");
        }
    }

    #[test]
    fn same_kind_hosts_respect_min_spacing() {
        // One always-rolling kind (rate 1.0) so spacing — not the rate gate —
        // is what's under test. Collect every hosting region on a grid; assert
        // pairwise Chebyshev distance >= min_spacing.
        let spacing = 7u32;
        let t = one_kind(RareKind::Ruin, 5, spacing, 1.0);
        let mut hosts: Vec<[i32; 2]> = Vec::new();
        for x in -40..40 {
            for z in -40..40 {
                if select_for_region([x, z], 0xABCD, 0, &t).is_some() {
                    hosts.push([x, z]);
                }
            }
        }
        assert!(hosts.len() >= 2, "expected several hosts, got {}", hosts.len());
        for i in 0..hosts.len() {
            for j in (i + 1)..hosts.len() {
                let dx = (hosts[i][0] - hosts[j][0]).abs();
                let dz = (hosts[i][1] - hosts[j][1]).abs();
                assert!(
                    dx.max(dz) >= spacing as i32,
                    "two {:?} hosts only {} apart (< {spacing})",
                    RareKind::Ruin,
                    dx.max(dz)
                );
            }
        }
    }

    #[test]
    fn biome_gating_excludes_disallowed_and_admits_allowed() {
        // Kind allowed ONLY in biome id 5.
        let t = RarityTable {
            region_rate: 1.0,
            region_span: 8,
            entries: vec![RareEntry {
                kind: RareKind::Shrine,
                weight: 1,
                min_spacing: 1, // every region is an anchor → only biome gates
                allowed_biomes: biome_bit(5),
            }],
        };
        // Disallowed biome (0): never appears anywhere.
        for x in 0..30 {
            for z in 0..30 {
                assert!(select_for_region([x, z], 7, 0, &t).is_none());
            }
        }
        // Allowed biome (5): appears (rate 1, spacing 1 → every region hosts).
        let hits = (0..30)
            .flat_map(|x| (0..30).map(move |z| [x, z]))
            .filter(|&r| select_for_region(r, 7, 5, &t).is_some())
            .count();
        assert_eq!(hits, 900, "every region in the allowed biome should host");
    }

    #[test]
    fn rate_zero_never_and_low_rate_is_rare() {
        let none = one_kind(RareKind::Obelisk, 1, 5, 0.0);
        for x in 0..30 {
            for z in 0..30 {
                assert!(select_for_region([x, z], 1, 0, &none).is_none());
            }
        }
        // Low rate over the default table → a small but non-zero fraction host.
        let t = RarityTable::default();
        let total = 100 * 100;
        let mut hosts = 0;
        for x in 0..100 {
            for z in 0..100 {
                if select_for_region([x, z], 0xBEEF, 0, &t).is_some() {
                    hosts += 1;
                }
            }
        }
        let frac = hosts as f32 / total as f32;
        assert!(frac > 0.0, "rares should occur at least sometimes");
        assert!(frac < 0.05, "rares should be rare; got {frac}");
    }

    #[test]
    fn weight_influences_relative_frequency() {
        // Two kinds, same spacing 1 (every region anchored for both), heavily
        // skewed weights → the heavy one should host far more often.
        let t = RarityTable {
            region_rate: 1.0,
            region_span: 8,
            entries: vec![
                RareEntry { kind: RareKind::Ruin, weight: 9, min_spacing: 1, allowed_biomes: all_biomes() },
                RareEntry { kind: RareKind::Obelisk, weight: 1, min_spacing: 1, allowed_biomes: all_biomes() },
            ],
        };
        let (mut ruin, mut obel) = (0, 0);
        for x in 0..80 {
            for z in 0..80 {
                match select_for_region([x, z], 0x1234, 0, &t).map(|p| p.kind) {
                    Some(RareKind::Ruin) => ruin += 1,
                    Some(RareKind::Obelisk) => obel += 1,
                    _ => {}
                }
            }
        }
        assert!(ruin > obel * 3, "weight 9 vs 1 should dominate: ruin={ruin} obel={obel}");
        assert!(obel > 0, "the light kind should still appear sometimes");
    }

    #[test]
    fn different_seeds_change_placement() {
        // Find a region that hosts under two seeds, assert the placement differs.
        let t = one_kind(RareKind::Monument, 1, 1, 1.0); // every region hosts (spacing 1, rate 1)
        let mut differed = false;
        for x in 0..40 {
            for z in 0..40 {
                let a = select_for_region([x, z], 1, 0, &t);
                let b = select_for_region([x, z], 2, 0, &t);
                if let (Some(pa), Some(pb)) = (a, b) {
                    if pa != pb {
                        differed = true;
                    }
                }
            }
        }
        assert!(differed, "different seeds should change some placements");
    }

    #[test]
    fn rotation_and_origin_in_range() {
        let t = RarityTable::default();
        for x in -50..50 {
            for z in -50..50 {
                if let Some(p) = select_for_region([x, z], 0xFADE, 19, &t) {
                    assert!(p.rotation < 4, "rotation out of range: {}", p.rotation);
                    assert!(p.origin[0] >= 0 && p.origin[0] < t.region_span);
                    assert!(p.origin[1] >= 0 && p.origin[1] < t.region_span);
                    assert!(!p.axiom.is_empty());
                    assert_eq!(p.axiom, p.kind.axiom());
                }
            }
        }
    }

    #[test]
    fn at_most_one_rare_per_region_and_kinds_are_consistent() {
        // The function returns a single placement; also check span<=1 → origin [0,0].
        let t = RarityTable {
            region_rate: 1.0,
            region_span: 1,
            entries: vec![RareEntry {
                kind: RareKind::Obelisk,
                weight: 1,
                min_spacing: 1,
                allowed_biomes: all_biomes(),
            }],
        };
        let p = select_for_region([5, 9], 3, 0, &t).expect("hosts");
        assert_eq!(p.origin, [0, 0]);
        assert_eq!(p.kind, RareKind::Obelisk);
    }
}
