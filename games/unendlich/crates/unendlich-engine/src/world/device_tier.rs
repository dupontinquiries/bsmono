//! `device_tier` — detected hardware capability → engine budgets.
//!
//! Worker-03 / WORLD-PERF pure core, round 2. Maps a device's capability
//! (RAM + GPU core count, via a tier) onto the budget set the rest of the
//! engine reads: render distance, octree pool size, LOD aggressiveness, the
//! apply/evict per-tick budgets, the frame target, and the GPU dispatch
//! budget. This is the scaling spine from an **M2 Ultra (192 GB / 80 GPU
//! cores)** down to a **Fairphone 5 (8 GB)** — the same envelope the rest of
//! the perf work targets. Composes with round 1's `lod_upload` (which consumes
//! `render_distance`) and `recompaction`.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for standalone
//! `rustc --test`.
//!
//! ## Monotonicity
//! Every *more-is-better* budget (render_distance, octree_pool_mb,
//! apply_budget, evict_budget, target_hz, gpu_dispatch_budget) is
//! non-decreasing in capability. `lod_aggressiveness` is the one inverse knob
//! — weaker hardware needs MORE aggressive LOD falloff — so it is non-INCREASING
//! in capability. Both directions are monotone; the tests check each accordingly.
#![allow(dead_code)]

/// Detected hardware capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceCaps {
    pub ram_bytes: u64,
    pub gpu_cores: u32,
}

impl DeviceCaps {
    pub fn new(ram_bytes: u64, gpu_cores: u32) -> Self {
        DeviceCaps { ram_bytes, gpu_cores }
    }
}

/// Coarse capability tier (ordered low → high by declaration).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Handheld,
    Laptop,
    Desktop,
    Workstation,
}

/// The engine budget set a tier resolves to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budgets {
    pub render_distance: u32,
    pub octree_pool_mb: u32,
    /// Inverse knob: higher = more aggressive LOD falloff (weaker hardware).
    pub lod_aggressiveness: u32,
    pub apply_budget: u32,
    pub evict_budget: u32,
    pub target_hz: u32,
    pub gpu_dispatch_budget: u32,
}

/// Capability score: GB of RAM + GPU core count. Monotone in both inputs.
fn score(caps: DeviceCaps) -> u64 {
    (caps.ram_bytes >> 30) + caps.gpu_cores as u64
}

/// Classify capability into a tier. Monotone: more RAM and/or more cores never
/// lowers the tier.
pub fn classify(caps: DeviceCaps) -> Tier {
    let s = score(caps);
    if s >= 96 {
        Tier::Workstation
    } else if s >= 40 {
        Tier::Desktop
    } else if s >= 16 {
        Tier::Laptop
    } else {
        Tier::Handheld
    }
}

/// Resolve a tier to its budget set.
pub fn budgets_for(tier: Tier) -> Budgets {
    match tier {
        Tier::Handheld => Budgets {
            render_distance: 6,
            octree_pool_mb: 96,
            lod_aggressiveness: 3,
            apply_budget: 4,
            evict_budget: 4,
            target_hz: 30,
            gpu_dispatch_budget: 2,
        },
        Tier::Laptop => Budgets {
            render_distance: 12,
            octree_pool_mb: 256,
            lod_aggressiveness: 2,
            apply_budget: 8,
            evict_budget: 8,
            target_hz: 60,
            gpu_dispatch_budget: 8,
        },
        Tier::Desktop => Budgets {
            render_distance: 24,
            octree_pool_mb: 512,
            lod_aggressiveness: 1,
            apply_budget: 16,
            evict_budget: 16,
            target_hz: 60,
            gpu_dispatch_budget: 16,
        },
        Tier::Workstation => Budgets {
            render_distance: 48,
            octree_pool_mb: 1536,
            lod_aggressiveness: 0,
            apply_budget: 32,
            evict_budget: 32,
            target_hz: 120,
            gpu_dispatch_budget: 64,
        },
    }
}

/// Convenience: capability straight to budgets.
pub fn budgets_for_caps(caps: DeviceCaps) -> Budgets {
    budgets_for(classify(caps))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gb(n: u64) -> u64 {
        n << 30
    }

    const TIERS: [Tier; 4] = [Tier::Handheld, Tier::Laptop, Tier::Desktop, Tier::Workstation];

    // The "more is better" budget fields, as accessors.
    fn up_fields(b: &Budgets) -> [u32; 6] {
        [
            b.render_distance,
            b.octree_pool_mb,
            b.apply_budget,
            b.evict_budget,
            b.target_hz,
            b.gpu_dispatch_budget,
        ]
    }

    #[test]
    fn classifies_reference_devices() {
        // Fairphone 5: 8 GB, ~4 GPU cores → Handheld.
        assert_eq!(classify(DeviceCaps::new(gb(8), 4)), Tier::Handheld);
        // M2 (base) laptop: 16 GB, ~10 cores → Laptop.
        assert_eq!(classify(DeviceCaps::new(gb(16), 10)), Tier::Laptop);
        // Desktop: 32 GB, ~20 cores → Desktop.
        assert_eq!(classify(DeviceCaps::new(gb(32), 20)), Tier::Desktop);
        // M2 Ultra: 192 GB, 80 GPU cores → Workstation.
        assert_eq!(classify(DeviceCaps::new(gb(192), 80)), Tier::Workstation);
    }

    #[test]
    fn budgets_monotone_across_tiers() {
        for win in TIERS.windows(2) {
            let lo = budgets_for(win[0]);
            let hi = budgets_for(win[1]);
            for (l, h) in up_fields(&lo).iter().zip(up_fields(&hi).iter()) {
                assert!(h >= l, "more-is-better budget decreased across tiers");
            }
            // aggressiveness is the inverse knob.
            assert!(hi.lod_aggressiveness <= lo.lod_aggressiveness);
        }
    }

    #[test]
    fn low_tier_fits_8gb_envelope() {
        let b = budgets_for(Tier::Handheld);
        // Octree pool well under what an 8 GB shared-memory device can spare
        // alongside mesh buffers; small render distance keeps chunk count low.
        assert!(b.octree_pool_mb <= 128);
        assert!(b.render_distance <= 8);
    }

    #[test]
    fn high_tier_scales_up() {
        let lo = budgets_for(Tier::Handheld);
        let hi = budgets_for(Tier::Workstation);
        for (l, h) in up_fields(&lo).iter().zip(up_fields(&hi).iter()) {
            assert!(h > l, "high tier did not scale a budget up");
        }
    }

    #[test]
    fn prop_monotone_in_caps() {
        // Pairwise: score(a) <= score(b) ⇒ every more-is-better budget(a) <=
        // budget(b) and aggressiveness(a) >= aggressiveness(b).
        let mut samples = Vec::new();
        for ram in [2u64, 6, 8, 12, 16, 24, 32, 64, 128, 192, 256] {
            for cores in [0u32, 2, 4, 8, 16, 40, 80] {
                samples.push(DeviceCaps::new(gb(ram), cores));
            }
        }
        for &a in &samples {
            for &b in &samples {
                if score(a) <= score(b) {
                    let ba = budgets_for_caps(a);
                    let bb = budgets_for_caps(b);
                    for (x, y) in up_fields(&ba).iter().zip(up_fields(&bb).iter()) {
                        assert!(y >= x, "budget not monotone in caps");
                    }
                    assert!(bb.lod_aggressiveness <= ba.lod_aggressiveness);
                }
            }
        }
    }

    #[test]
    fn prop_deterministic() {
        let c = DeviceCaps::new(gb(31), 19);
        assert_eq!(classify(c), classify(c));
        assert_eq!(budgets_for_caps(c), budgets_for_caps(c));
    }
}
