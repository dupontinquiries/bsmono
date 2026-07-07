//! device_budget — pick sane performance budgets from what the machine *is*.
//!
//! [`crate::runtime_config`] already holds every perf knob and a ladder of
//! [`PerfPreset`]s, but nothing chooses a preset from the hardware: today it's
//! `from_env` (manual) or the shipping default. That's how a Fairphone ends up
//! handed the same 256 MB octree pool an M2 Ultra gets — and dies. This module
//! is the missing bridge: a **pure** policy from detected capability
//! ([`DeviceCaps`]) to a starting preset, an octree-pool budget, and a worker
//! thread count. It's a *starting point* the user can still override in
//! settings (the result maps straight onto [`RuntimeConfig::preset`]).
//!
//! Design: RAM and GPU are each mapped to a tier, and the chosen tier is the
//! **min of the two** — a monster GPU can't rescue 4 GB of RAM, and 64 GB can't
//! rescue an integrated GPU. Mobile then steps down one tier (battery/thermal
//! headroom) and is capped below Ultra. The octree pool is a *fraction of RAM*
//! with mobile-vs-desktop floors/ceilings, so it scales with the device instead
//! of a fixed constant. Pure, deterministic, std-only, and fully unit-tested;
//! the only non-pure part is the thin [`DeviceCaps::detect`] best-effort probe.

use crate::runtime_config::{PerfPreset, RuntimeConfig};

/// Coarse GPU capability class. We can't enumerate every device, so callers map
/// their detected adapter into one of these buckets (integrated vs discrete and
/// roughly how strong). Ordered weakest→strongest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GpuClass {
    /// Integrated / shared-memory (Intel UHD, base Apple, mobile SoC GPU).
    Integrated,
    /// Entry discrete or a strong mobile GPU (MX-tier, older discrete).
    Low,
    /// Mainstream discrete (xx60-class, Apple Pro).
    Mid,
    /// High-end discrete / workstation (xx80+, Apple Max/Ultra).
    High,
}

/// What we know about the machine. All fields are best-effort; the policy is
/// written to degrade safely when a probe is unsure (it treats unknowns as the
/// weaker option).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceCaps {
    /// Total system RAM in megabytes.
    pub ram_mb: u32,
    /// GPU capability bucket.
    pub gpu: GpuClass,
    /// Logical CPU threads available.
    pub threads: u32,
    /// Phone/tablet/handheld — biases toward battery + thermal headroom.
    pub is_mobile: bool,
}

impl DeviceCaps {
    /// A deliberately conservative fallback for when nothing could be detected:
    /// assume a modest laptop so we never *over*-commit on an unknown device.
    pub fn conservative() -> DeviceCaps {
        DeviceCaps { ram_mb: 8192, gpu: GpuClass::Integrated, threads: 4, is_mobile: false }
    }

    /// Best-effort, non-pure probe of the host. Only fills what std can see
    /// portably (thread count); RAM/GPU/mobile are left at conservative
    /// defaults for the caller to refine from platform APIs (sysinfo, the
    /// Vulkan physical-device props, an OS check). Kept tiny on purpose so the
    /// *policy* below stays the tested part.
    pub fn detect() -> DeviceCaps {
        let threads = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(4);
        DeviceCaps { threads, ..DeviceCaps::conservative() }
    }
}

/// The five selectable presets as an ordinal ladder (0 = BatterySaver …
/// 4 = Ultra), so tiers can be compared/clamped with plain integer `min`.
fn preset_from_ordinal(t: u8) -> PerfPreset {
    match t {
        0 => PerfPreset::BatterySaver,
        1 => PerfPreset::Low,
        2 => PerfPreset::Medium,
        3 => PerfPreset::High,
        _ => PerfPreset::Ultra,
    }
}

/// RAM → tier ordinal (0..=4). Thresholds are the "comfortable working set" for
/// each preset's render distance / cache, not hard minimums.
fn ram_tier(ram_mb: u32) -> u8 {
    match ram_mb {
        0..=3071 => 0,      // < 3 GB  → Battery Saver
        3072..=5119 => 1,   // 3–5 GB  → Low
        5120..=11263 => 2,  // 6–11 GB → Medium
        11264..=23551 => 3, // 12–23 GB→ High
        _ => 4,             // 24 GB+  → Ultra
    }
}

/// GPU class → tier ordinal (0..=4).
fn gpu_tier(gpu: GpuClass) -> u8 {
    match gpu {
        GpuClass::Integrated => 1, // an iGPU can drive Low comfortably
        GpuClass::Low => 2,
        GpuClass::Mid => 3,
        GpuClass::High => 4,
    }
}

/// The recommended preset **ordinal** (0..=4) for a device. RAM and GPU each
/// gate independently (take the min), then mobile steps down one for battery /
/// thermals and is capped below Ultra. Pure and std-only.
pub fn recommended_tier(caps: &DeviceCaps) -> u8 {
    let mut t = ram_tier(caps.ram_mb).min(gpu_tier(caps.gpu));
    if caps.is_mobile {
        t = t.saturating_sub(1); // battery/thermal headroom
        t = t.min(3); // no Ultra on a phone regardless of specs
    }
    // A device with very few threads can't feed the higher upload/gen batches.
    if caps.threads <= 2 {
        t = t.min(1);
    } else if caps.threads <= 4 {
        t = t.min(3);
    }
    t
}

/// The recommended starting [`PerfPreset`] for a device.
pub fn recommend_preset(caps: &DeviceCaps) -> PerfPreset {
    preset_from_ordinal(recommended_tier(caps))
}

/// Octree pool budget (MB) for a device — a fraction of RAM with device-class
/// floors and ceilings, so it tracks the machine instead of a fixed constant.
/// This is the knob that OOMs small devices when left at the desktop default.
pub fn octree_pool_mb(caps: &DeviceCaps) -> u32 {
    if caps.is_mobile {
        // ~1/32 of RAM, held well under the desktop 256 MB default.
        (caps.ram_mb / 32).clamp(48, 160)
    } else {
        // ~1/12 of RAM, generous but bounded so a 64 GB box doesn't reserve absurdly.
        (caps.ram_mb / 12).clamp(128, 768)
    }
}

/// Worker-thread budget: leave one core for the main/render/audio threads, and
/// never claim zero. Mobile keeps an extra core free for the OS + battery.
pub fn worker_threads(caps: &DeviceCaps) -> u32 {
    let reserve = if caps.is_mobile { 2 } else { 1 };
    caps.threads.saturating_sub(reserve).max(1)
}

/// The full starting budget for a device: a concrete [`RuntimeConfig`] (from the
/// recommended preset) plus the pool and thread budgets that live outside it.
#[derive(Clone, Debug, PartialEq)]
pub struct Budgets {
    pub preset: PerfPreset,
    pub config: RuntimeConfig,
    pub octree_pool_mb: u32,
    pub worker_threads: u32,
}

/// Recommend a complete starting budget for a device. The `config` is exactly
/// `RuntimeConfig::preset(preset)`, so this only *chooses* — it never invents
/// new knob values, keeping it consistent with the settings UI.
pub fn budgets_for(caps: &DeviceCaps) -> Budgets {
    let preset = recommend_preset(caps);
    Budgets {
        preset,
        config: RuntimeConfig::preset(preset),
        octree_pool_mb: octree_pool_mb(caps),
        worker_threads: worker_threads(caps),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(ram_mb: u32, gpu: GpuClass, threads: u32, is_mobile: bool) -> DeviceCaps {
        DeviceCaps { ram_mb, gpu, threads, is_mobile }
    }

    #[test]
    fn a_fairphone_lands_low_and_a_small_pool() {
        // 4 GB mobile SoC, 8 threads: Low tier at most, and a pool far under the
        // 256 MB desktop default that was killing it.
        let fp = caps(4096, GpuClass::Integrated, 8, true);
        assert!(recommended_tier(&fp) <= 1, "a phone shouldn't get Medium+, got {}", recommended_tier(&fp));
        let pool = octree_pool_mb(&fp);
        assert!(pool < 256, "mobile pool must stay under the desktop default, got {pool}");
        assert!(pool >= 48, "but not absurdly tiny, got {pool}");
    }

    #[test]
    fn an_m2_ultra_opens_up() {
        // 64 GB, high-end GPU, many threads → Ultra with a big (but bounded) pool.
        let ultra = caps(65536, GpuClass::High, 24, false);
        assert_eq!(recommend_preset(&ultra), PerfPreset::Ultra);
        assert_eq!(octree_pool_mb(&ultra), 768, "generous but capped");
        assert_eq!(worker_threads(&ultra), 23);
    }

    #[test]
    fn ram_gates_a_strong_gpu() {
        // A monster GPU can't rescue 4 GB of RAM.
        let t = recommended_tier(&caps(4096, GpuClass::High, 16, false));
        assert!(t <= 1, "4 GB should cap the tier regardless of GPU, got {t}");
    }

    #[test]
    fn gpu_gates_lots_of_ram() {
        // 64 GB with an integrated GPU shouldn't pretend to be a High/Ultra rig.
        let t = recommended_tier(&caps(65536, GpuClass::Integrated, 16, false));
        assert!(t <= 1, "an iGPU caps the tier regardless of RAM, got {t}");
    }

    #[test]
    fn the_recommendation_is_monotonic_in_ram() {
        // More RAM never lowers the tier (holding everything else fixed).
        let mut prev = 0u8;
        for &ram in &[2048u32, 4096, 8192, 16384, 32768, 65536] {
            let t = recommended_tier(&caps(ram, GpuClass::High, 16, false));
            assert!(t >= prev, "tier dropped as RAM rose: {prev} -> {t} at {ram} MB");
            prev = t;
        }
    }

    #[test]
    fn the_recommendation_is_monotonic_in_gpu() {
        let mut prev = 0u8;
        for &g in &[GpuClass::Integrated, GpuClass::Low, GpuClass::Mid, GpuClass::High] {
            let t = recommended_tier(&caps(65536, g, 16, false));
            assert!(t >= prev, "tier dropped as GPU rose: {prev} -> {t}");
            prev = t;
        }
    }

    #[test]
    fn mobile_never_reaches_ultra() {
        let beefy_phone = caps(65536, GpuClass::High, 16, true);
        assert!(recommended_tier(&beefy_phone) <= 3, "no Ultra on mobile");
    }

    #[test]
    fn thin_cpus_are_throttled() {
        // Two threads can't feed High no matter the GPU/RAM.
        let t = recommended_tier(&caps(65536, GpuClass::High, 2, false));
        assert!(t <= 1, "a 2-thread box shouldn't run High/Ultra, got {t}");
    }

    #[test]
    fn worker_threads_leave_headroom_and_never_zero() {
        assert_eq!(worker_threads(&caps(8192, GpuClass::Mid, 8, false)), 7);
        assert_eq!(worker_threads(&caps(8192, GpuClass::Mid, 8, true)), 6); // mobile reserves 2
        assert_eq!(worker_threads(&caps(2048, GpuClass::Integrated, 1, true)), 1); // never 0
    }

    #[test]
    fn budgets_config_matches_the_chosen_preset() {
        let c = caps(16384, GpuClass::Mid, 12, false);
        let b = budgets_for(&c);
        assert_eq!(b.config, RuntimeConfig::preset(b.preset), "config must equal the preset table");
    }

    #[test]
    fn conservative_default_is_not_over_committed() {
        // Unknown device → never recommend more than Medium, and a bounded pool.
        let b = budgets_for(&DeviceCaps::conservative());
        assert!(recommended_tier(&DeviceCaps::conservative()) <= 2);
        assert!(b.octree_pool_mb <= 768);
    }

    #[test]
    fn pools_are_deterministic() {
        let c = caps(16384, GpuClass::Mid, 12, false);
        assert_eq!(octree_pool_mb(&c), octree_pool_mb(&c));
    }
}
