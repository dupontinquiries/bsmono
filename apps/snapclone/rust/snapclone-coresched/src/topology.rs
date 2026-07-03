//! topology.rs — classify CPUs into efficiency vs performance cores.
//!
//! On heterogeneous CPUs (ARM big.LITTLE like the Fairphone 5, and Intel/AMD
//! hybrid laptops), the kernel publishes a per-CPU "capacity" — a relative
//! compute weight where the most-capable core is normalized to 1024. Efficiency
//! cores have a lower capacity than performance cores. Linux exposes this at
//! `/sys/devices/system/cpu/cpuN/cpu_capacity`. Where capacity isn't available
//! (older kernels), the per-core max frequency
//! (`cpufreq/cpuinfo_max_freq`) is a serviceable proxy.
//!
//! This module turns the raw per-CPU numbers into an E-core / P-core split. The
//! parsing takes the values as input so it's fully unit-testable; a thin
//! `read_from_sysfs` convenience does the actual file reads for real use.

use std::collections::BTreeSet;

/// The classified CPU layout.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CpuTopology {
    /// Logical CPU ids classified as efficiency cores (lower capacity).
    pub efficiency: Vec<u32>,
    /// Logical CPU ids classified as performance cores (highest capacity).
    pub performance: Vec<u32>,
    /// True when the system is heterogeneous (an E/P split actually exists).
    /// On a uniform CPU every core is "performance" and `efficiency` is empty;
    /// callers then skip demotion (nothing to gain).
    pub heterogeneous: bool,
}

impl CpuTopology {
    pub fn total_cpus(&self) -> usize {
        self.efficiency.len() + self.performance.len()
    }

    /// True if there are efficiency cores to demote background work onto.
    pub fn has_efficiency_cores(&self) -> bool {
        self.heterogeneous && !self.efficiency.is_empty()
    }
}

/// Classify from a list of (cpu_id, capacity) pairs into efficiency vs
/// performance tiers.
///
/// IMPORTANT — why gap detection, not a fixed tolerance from the max:
/// the Fairphone 5's QCM6490 is THREE-tier — 1 prime A78 (capacity ~1024),
/// 3 gold A78 (~910), 4 silver A55 (~335). A naive "within 5% of the max =
/// performance" rule puts the gold A78 cores (910, which is 89% of 1024) into
/// the EFFICIENCY bucket alongside the A55s, leaving only the single prime core
/// as "performance" — wrong on both ends (it would hard-pin background work onto
/// real big cores AND starve the UI to one core). The correct split is at the
/// LARGE capacity gap: A55→A78 is a ~2.7× jump, while gold→prime is only ~1.13×.
///
/// So we sort the distinct capacities and split at the biggest RELATIVE gap.
/// Everything below the split is efficiency; everything at/above is performance
/// (prime + gold collapse into one "performance" tier — we don't need to single
/// out the prime, since the goal is only "keep background work OFF the big
/// cores"). If the largest gap is small (a near-uniform CPU), we treat the whole
/// thing as uniform — no worthwhile demotion.
pub fn classify_by_capacity(caps: &[(u32, u32)]) -> CpuTopology {
    if caps.is_empty() {
        return CpuTopology::default();
    }
    let max_cap = caps.iter().map(|(_, c)| *c).max().unwrap_or(0);
    if max_cap == 0 {
        let perf: Vec<u32> = sorted_ids(caps);
        return CpuTopology { efficiency: vec![], performance: perf, heterogeneous: false };
    }

    // Distinct capacity levels, ascending.
    let mut levels: Vec<u32> = caps.iter().map(|(_, c)| *c).collect();
    levels.sort_unstable();
    levels.dedup();

    // A single level → uniform CPU, no split.
    if levels.len() < 2 {
        return CpuTopology {
            efficiency: vec![],
            performance: sorted_ids(caps),
            heterogeneous: false,
        };
    }

    // Find the largest RELATIVE gap between adjacent levels. ratio = hi/lo.
    // The split threshold is the higher value of that gap pair: cores strictly
    // below it are efficiency.
    // A gap counts as a real E/P boundary only if hi is at least
    // MIN_SPLIT_RATIO × lo — otherwise the CPU is effectively uniform and we
    // skip demotion rather than split on noise.
    const MIN_SPLIT_RATIO_PCT: u64 = 130; // hi must be ≥1.3× lo to be a real tier break
    let mut best_ratio_pct = 0u64;
    let mut split_at = 0u32; // capacities >= split_at are performance
    for w in levels.windows(2) {
        let lo = w[0] as u64;
        let hi = w[1] as u64;
        if lo == 0 {
            continue;
        }
        let ratio_pct = hi * 100 / lo;
        if ratio_pct > best_ratio_pct {
            best_ratio_pct = ratio_pct;
            split_at = w[1];
        }
    }

    if best_ratio_pct < MIN_SPLIT_RATIO_PCT {
        // Near-uniform — no tier worth demoting onto.
        return CpuTopology {
            efficiency: vec![],
            performance: sorted_ids(caps),
            heterogeneous: false,
        };
    }

    let mut eff = BTreeSet::new();
    let mut perf = BTreeSet::new();
    for (id, cap) in caps {
        if *cap >= split_at {
            perf.insert(*id);
        } else {
            eff.insert(*id);
        }
    }
    let heterogeneous = !eff.is_empty();
    CpuTopology {
        efficiency: eff.into_iter().collect(),
        performance: perf.into_iter().collect(),
        heterogeneous,
    }
}

/// Classify from per-core max frequencies (Hz or kHz — units don't matter, only
/// relative ordering). Same rule: top tier = performance.
pub fn classify_by_max_freq(freqs: &[(u32, u64)]) -> CpuTopology {
    // Reuse the capacity classifier by scaling freqs into the 0..=1024 space so
    // the same tolerance applies. Scaling preserves the relative split.
    if freqs.is_empty() {
        return CpuTopology::default();
    }
    let max = freqs.iter().map(|(_, f)| *f).max().unwrap_or(0);
    if max == 0 {
        let perf = sorted_ids_u64(freqs);
        return CpuTopology { efficiency: vec![], performance: perf, heterogeneous: false };
    }
    let caps: Vec<(u32, u32)> = freqs
        .iter()
        .map(|(id, f)| (*id, ((*f as u128 * 1024) / max as u128) as u32))
        .collect();
    classify_by_capacity(&caps)
}

fn sorted_ids(caps: &[(u32, u32)]) -> Vec<u32> {
    let mut v: Vec<u32> = caps.iter().map(|(id, _)| *id).collect();
    v.sort();
    v
}
fn sorted_ids_u64(freqs: &[(u32, u64)]) -> Vec<u32> {
    let mut v: Vec<u32> = freqs.iter().map(|(id, _)| *id).collect();
    v.sort();
    v
}

/// Read + classify from real sysfs. Tries cpu_capacity first, falls back to
/// cpufreq max. Returns a uniform (non-heterogeneous) topology if neither is
/// available. Best-effort: unreadable cores are skipped.
#[cfg(target_os = "linux")]
pub fn read_from_sysfs() -> CpuTopology {
    use std::fs;
    let base = "/sys/devices/system/cpu";
    let mut caps: Vec<(u32, u32)> = Vec::new();
    let mut freqs: Vec<(u32, u64)> = Vec::new();

    // Enumerate cpuN directories.
    let Ok(entries) = fs::read_dir(base) else {
        return CpuTopology::default();
    };
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        let Some(num) = name.strip_prefix("cpu") else { continue };
        let Ok(id) = num.parse::<u32>() else { continue };

        if let Ok(s) = fs::read_to_string(format!("{base}/cpu{id}/cpu_capacity")) {
            if let Ok(c) = s.trim().parse::<u32>() {
                caps.push((id, c));
            }
        }
        if let Ok(s) =
            fs::read_to_string(format!("{base}/cpu{id}/cpufreq/cpuinfo_max_freq"))
        {
            if let Ok(f) = s.trim().parse::<u64>() {
                freqs.push((id, f));
            }
        }
    }

    if !caps.is_empty() {
        classify_by_capacity(&caps)
    } else if !freqs.is_empty() {
        classify_by_max_freq(&freqs)
    } else {
        CpuTopology::default()
    }
}

#[cfg(not(target_os = "linux"))]
pub fn read_from_sysfs() -> CpuTopology {
    // No sysfs off Linux — report uniform so callers skip demotion.
    CpuTopology::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fp5_style_split() {
        // Canonical clean 4 little + 4 big layout (gap detection splits at the
        // little→big jump).
        let caps = vec![
            (0, 410), (1, 410), (2, 410), (3, 410), // little
            (4, 1024), (5, 1024), (6, 1024), (7, 1024), // big
        ];
        let topo = classify_by_capacity(&caps);
        assert_eq!(topo.efficiency, vec![0, 1, 2, 3]);
        assert_eq!(topo.performance, vec![4, 5, 6, 7]);
        assert!(topo.heterogeneous);
        assert!(topo.has_efficiency_cores());
        assert_eq!(topo.total_cpus(), 8);
    }

    #[test]
    fn fp5_qcm6490_three_tier() {
        // THE REAL FP5: QCM6490 is THREE-tier — 4 silver A55 (~335), 3 gold A78
        // (~910), 1 prime A78 (1024). The OLD "within 5% of max" rule wrongly put
        // the gold A78s in EFFICIENCY (910 < 0.95*1024). Gap detection splits at
        // the big A55→A78 jump (335→910 = 2.7×), NOT the small gold→prime jump
        // (910→1024 = 1.13×). So the 4 A55s are efficiency and all 4 A78s
        // (gold+prime) are performance — which is what we want: keep background
        // work off ALL the big cores, leave 4 cores for the UI.
        let caps = vec![
            (0, 335), (1, 335), (2, 335), (3, 335), // silver A55
            (4, 910), (5, 910), (6, 910),           // gold A78
            (7, 1024),                               // prime A78
        ];
        let topo = classify_by_capacity(&caps);
        assert_eq!(topo.efficiency, vec![0, 1, 2, 3], "only the A55s are efficiency");
        assert_eq!(topo.performance, vec![4, 5, 6, 7], "gold+prime A78 are all performance");
        assert!(topo.has_efficiency_cores());
    }

    #[test]
    fn intel_hybrid_three_tier_collapses_to_e_and_p() {
        // Intel hybrid: P-cores at 1024, E-cores lower. Any below-max tier is
        // "efficiency" for our binary demotion decision.
        let caps = vec![
            (0, 1024), (1, 1024), // P
            (2, 600), (3, 600), (4, 600), (5, 600), // E
        ];
        let topo = classify_by_capacity(&caps);
        assert_eq!(topo.performance, vec![0, 1]);
        assert_eq!(topo.efficiency, vec![2, 3, 4, 5]);
        assert!(topo.has_efficiency_cores());
    }

    #[test]
    fn uniform_cpu_has_no_efficiency_cores() {
        // A homogeneous server/desktop: all equal → nothing to demote.
        let caps = vec![(0, 1024), (1, 1024), (2, 1024), (3, 1024)];
        let topo = classify_by_capacity(&caps);
        assert!(!topo.heterogeneous);
        assert!(!topo.has_efficiency_cores());
        assert_eq!(topo.performance.len(), 4);
        assert!(topo.efficiency.is_empty());
    }

    #[test]
    fn near_max_cores_stay_in_perf_tier() {
        // 1024 and 1000 are close (1.02× — below the 1.3× tier-break ratio), so
        // the only real gap is 1000→500 (2.0×). Both top cores are performance;
        // the 500 core is efficiency. (Gap detection, not a fixed tolerance.)
        let caps = vec![(0, 1024), (1, 1000), (2, 500)];
        let topo = classify_by_capacity(&caps);
        assert_eq!(topo.performance, vec![0, 1]);
        assert_eq!(topo.efficiency, vec![2]);
    }

    #[test]
    fn near_uniform_does_not_split_on_noise() {
        // All cores within ~13% — no gap reaches the 1.3× tier-break ratio, so
        // we treat it as uniform rather than demoting onto a phantom "E" tier.
        let caps = vec![(0, 1024), (1, 1000), (2, 980), (3, 950)];
        let topo = classify_by_capacity(&caps);
        assert!(!topo.heterogeneous, "near-uniform → no split");
        assert!(topo.efficiency.is_empty());
        assert_eq!(topo.performance.len(), 4);
    }

    #[test]
    fn empty_and_zero_capacity_are_safe() {
        assert_eq!(classify_by_capacity(&[]), CpuTopology::default());
        let z = classify_by_capacity(&[(0, 0), (1, 0)]);
        assert!(!z.heterogeneous);
        assert_eq!(z.performance, vec![0, 1]);
    }

    #[test]
    fn classify_by_freq_matches_capacity_split() {
        // cpufreq fallback: little cluster 1.8GHz, big 2.6GHz.
        let freqs = vec![
            (0, 1_800_000), (1, 1_800_000), (2, 1_800_000), (3, 1_800_000),
            (4, 2_600_000), (5, 2_600_000),
        ];
        let topo = classify_by_max_freq(&freqs);
        assert_eq!(topo.efficiency, vec![0, 1, 2, 3]);
        assert_eq!(topo.performance, vec![4, 5]);
    }
}
