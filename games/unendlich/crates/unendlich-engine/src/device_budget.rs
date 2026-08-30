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

    /// Best-effort, non-pure probe of the host. Fills thread count portably,
    /// RAM from /proc/meminfo on Linux, and the mobile flag from the device
    /// tree (see [`classify_mobile_from_compatible`]). GPU class is left at
    /// conservative (Integrated) — refine from VkPhysicalDeviceProperties at
    /// the Vulkan init site.
    ///
    /// Cached: the first call probes the system; subsequent calls return
    /// the cached result. This is correct because the hardware doesn't
    /// change during a process lifetime.
    pub fn detect() -> DeviceCaps {
        static CACHED: std::sync::OnceLock<DeviceCaps> = std::sync::OnceLock::new();
        *CACHED.get_or_init(|| {
            let threads = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(4);
            let ram_mb = detect_ram_mb().unwrap_or(8192);
            // Mobile classification. The old rule (`aarch64 && ram < 6 GB`)
            // misfired both ways: an 8 GB Fairphone 5 counted as DESKTOP (and
            // was handed desktop-sized GPU pools — the OOM), while it would
            // have flagged a hypothetical low-RAM Apple desktop as a phone.
            // The device tree tells us what the SoC actually is; RAM stays
            // only as a last-resort fallback when no device tree exists.
            let is_mobile = if cfg!(target_arch = "aarch64") || cfg!(target_arch = "arm") {
                match detect_dt_compatible() {
                    Some(compat) => classify_mobile_from_compatible(&compat),
                    // No device tree readable (containers, odd kernels):
                    // fall back to the conservative old RAM heuristic.
                    None => ram_mb < 6144,
                }
            } else {
                false
            };
            DeviceCaps { ram_mb, gpu: GpuClass::Integrated, threads, is_mobile }
        })
    }
}

/// Decide phone/handheld vs desktop from the flattened-device-tree
/// `compatible` string (e.g. `"fairphone,fp5\0qcom,qcm6490"` on a Fairphone 5,
/// `"apple,j414\0apple,t6001\0..."` on Asahi Apple Silicon). Pure and
/// unit-tested; [`DeviceCaps::detect`] feeds it the real file.
///
/// Policy: a phone/tablet SoC vendor string ⇒ mobile. Apple Silicon ⇒ desktop
/// (Asahi laptops have battery+thermal headroom a phone doesn't, and they are
/// the project's primary dev machines). Unrecognized ARM boards (Pi-class
/// SBCs, Rockchip, etc.) lean MOBILE — conservative budgets on a weak unknown
/// board are the safe failure, over-committing is not.
pub fn classify_mobile_from_compatible(compatible: &str) -> bool {
    let c = compatible.to_ascii_lowercase();
    if c.contains("apple,") {
        return false;
    }
    // Phone/tablet SoC vendors (device-tree `compatible` vendor prefixes).
    const PHONE_SOCS: &[&str] =
        &["qcom,", "mediatek,", "samsung,exynos", "hisilicon,", "sprd,", "unisoc,", "fairphone,"];
    if PHONE_SOCS.iter().any(|v| c.contains(v)) {
        return true;
    }
    // Unknown ARM board: budget it like a small device rather than a tower.
    true
}

/// Read the flattened device tree's root `compatible` property (NUL-separated
/// vendor,model strings). None on non-DT systems (x86, containers).
fn detect_dt_compatible() -> Option<String> {
    let bytes = std::fs::read("/proc/device-tree/compatible").ok()?;
    Some(String::from_utf8_lossy(&bytes).replace('\0', " "))
}

/// Read total RAM in MB from /proc/meminfo. Returns None on non-Linux or
/// parse failure. This is a one-shot probe at startup, not a hot path.
fn detect_ram_mb() -> Option<u32> {
    let contents = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            // Format: "MemTotal:       16384000 kB"
            let kb: u64 = rest.trim().split_whitespace().next()?.parse().ok()?;
            return Some((kb / 1024) as u32);
        }
    }
    None
}

/// Classify a Vulkan device type into a GpuClass. Called after Vulkan init
/// to refine the conservative `Integrated` default from `detect()`.
pub fn gpu_class_from_vk_device_type(device_type: i32) -> GpuClass {
    // ash::vk::PhysicalDeviceType::as_raw() values:
    //   OTHER=0, INTEGRATED_GPU=1, DISCRETE_GPU=2, VIRTUAL_GPU=3, CPU=4
    match device_type {
        2 => GpuClass::Mid,        // DISCRETE_GPU — at least Mid
        1 => GpuClass::Integrated, // INTEGRATED_GPU — confirmed integrated
        _ => GpuClass::Integrated, // OTHER/VIRTUAL/CPU — conservative
    }
}

/// Refine the cached DeviceCaps with a Vulkan-detected GPU class. Call once
/// after VulkanContext creation. The RuntimeConfig should be created AFTER
/// this call so it picks up the refined GPU tier.
///
/// Thread-safe: only the first call takes effect (OnceLock for the GPU
/// refinement). If `detect()` was already called (caching the caps), this
/// updates the cached copy.
pub fn refine_gpu_class(gpu: GpuClass) {
    // We can't mutate the OnceLock, but we CAN store the refinement
    // separately and have recommended_tier check it.
    GPU_CLASS_OVERRIDE.get_or_init(|| gpu);
}

/// The refined GPU class, if `refine_gpu_class` was called.
pub(crate) fn refined_gpu_class() -> Option<GpuClass> {
    GPU_CLASS_OVERRIDE.get().copied()
}

static GPU_CLASS_OVERRIDE: std::sync::OnceLock<GpuClass> = std::sync::OnceLock::new();

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
        GpuClass::Integrated => 2, // conservative for "unknown" — at least Medium
        GpuClass::Low => 2,
        GpuClass::Mid => 3,
        GpuClass::High => 4,
    }
}

/// The recommended preset **ordinal** (0..=4) for a device. RAM and GPU each
/// gate independently (take the min), then mobile steps down one for battery /
/// thermals and is capped below Ultra. Pure and std-only.
pub fn recommended_tier(caps: &DeviceCaps) -> u8 {
    // Use the Vulkan-refined GPU class if available, otherwise the
    // caps value (which is Integrated/conservative from detect()).
    let effective_gpu = refined_gpu_class().unwrap_or(caps.gpu);
    let mut t = ram_tier(caps.ram_mb).min(gpu_tier(effective_gpu));
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
/// This is one of the knobs that OOMs small devices when left at the desktop
/// default. Integrated GPUs share system RAM (Apple, phones, Intel iGPU), so
/// their fraction is smaller than a discrete card with its own VRAM.
pub fn octree_pool_mb(caps: &DeviceCaps) -> u32 {
    let gpu = refined_gpu_class().unwrap_or(caps.gpu);
    if caps.is_mobile {
        // ~1/32 of RAM, held well under the desktop 256 MB default.
        (caps.ram_mb / 32).clamp(48, 160)
    } else if gpu <= GpuClass::Low {
        // Unified/shared memory desktop (Asahi, iGPU): every buffer byte is a
        // RAM byte. ~1/16 of RAM, bounded.
        (caps.ram_mb / 16).clamp(96, 512)
    } else {
        // Discrete VRAM: generous but bounded so a 64 GB box doesn't reserve absurdly.
        (caps.ram_mb / 12).clamp(128, 768)
    }
}

/// Mesh pool budget `(vertex_mb, index_mb)` for a device.
///
/// THE Fairphone killer: `render::mesh_buffer_pool` shipped fixed defaults of
/// **3 GiB vertex + 2 GiB index** — 5 GiB of GPU buffers allocated up front on
/// every device unless an env var said otherwise. A 64 GB unified-memory M2
/// absorbs that; an 8 GB phone with a shared-memory GPU is dead on arrival
/// ("taught maximization"). This scales the pool with RAM and memory
/// architecture, keeping today's 3 GiB/2 GiB ceiling for big unified/discrete
/// machines (the M2 path is unchanged) while a phone gets a pool it can live
/// in. Index pool is 2/3 of vertex, matching the shipped 3:2 ratio.
pub fn mesh_pool_mb(caps: &DeviceCaps) -> (u32, u32) {
    let gpu = refined_gpu_class().unwrap_or(caps.gpu);
    let vert = if caps.is_mobile {
        // ~1/16 of RAM: an 8 GB phone gets 512 MB vertex + 341 MB index —
        // roomy for the Low preset's RD-4 working set, survivable for the OS.
        (caps.ram_mb / 16).clamp(192, 512)
    } else if gpu <= GpuClass::Low {
        // Unified/shared memory desktop: 1/8 of RAM, ceiling at today's 3 GiB
        // (a 24 GB+ Apple machine keeps exactly the current behavior).
        (caps.ram_mb / 8).clamp(384, 3072)
    } else {
        // Discrete VRAM: 1/4 of system RAM as a proxy, same 3 GiB ceiling.
        (caps.ram_mb / 4).clamp(512, 3072)
    };
    (vert, vert * 2 / 3)
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
    pub mesh_vert_pool_mb: u32,
    pub mesh_index_pool_mb: u32,
    pub worker_threads: u32,
}

/// Recommend a complete starting budget for a device. The `config` is exactly
/// `RuntimeConfig::preset(preset)`, so this only *chooses* — it never invents
/// new knob values, keeping it consistent with the settings UI.
pub fn budgets_for(caps: &DeviceCaps) -> Budgets {
    let preset = recommend_preset(caps);
    let (vert, idx) = mesh_pool_mb(caps);
    Budgets {
        preset,
        config: RuntimeConfig::preset(preset),
        octree_pool_mb: octree_pool_mb(caps),
        mesh_vert_pool_mb: vert,
        mesh_index_pool_mb: idx,
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
        // 64 GB with an unclassified GPU shouldn't pretend to be High/Ultra.
        // Integrated tier is 2 (Medium) — conservative "unknown" default.
        let t = recommended_tier(&caps(65536, GpuClass::Integrated, 16, false));
        assert!(t <= 2, "an unclassified GPU caps the tier at Medium, got {t}");
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

    #[test]
    fn an_8gb_fairphone_5_fits_in_its_ram() {
        // THE reported device: Fairphone 5, 8 GB, Adreno (shared memory),
        // 8 threads. The old chain handed it a Medium desktop preset, a 256 MB
        // octree pool, and 5 GiB of mesh buffers. It must land Low with pools
        // a shared-memory 8 GB device can actually hold.
        let fp5 = caps(8192, GpuClass::Integrated, 8, true);
        assert!(
            recommended_tier(&fp5) <= 1,
            "an 8 GB phone must not exceed Low, got {}",
            recommended_tier(&fp5)
        );
        let b = budgets_for(&fp5);
        assert!(b.octree_pool_mb <= 160, "octree pool {} MB too big for a phone", b.octree_pool_mb);
        assert!(
            b.mesh_vert_pool_mb <= 512,
            "vertex pool {} MB — the 3 GiB default is the OOM",
            b.mesh_vert_pool_mb
        );
        let total_gpu_mb = b.octree_pool_mb + b.mesh_vert_pool_mb + b.mesh_index_pool_mb;
        assert!(
            total_gpu_mb <= 1024,
            "phone GPU-buffer reserve must stay ≤ 1 GB of the 8 GB, got {total_gpu_mb} MB"
        );
        assert!(b.worker_threads <= 6, "mobile keeps cores free for the OS");
    }

    #[test]
    fn big_unified_and_discrete_machines_keep_todays_mesh_pool() {
        // The M2-class dev machines (unified memory, lots of RAM) must keep the
        // shipped 3 GiB / 2 GiB behavior — this change rescues phones without
        // regressing the primary hardware.
        let m2 = caps(65536, GpuClass::Integrated, 24, false);
        assert_eq!(mesh_pool_mb(&m2), (3072, 2048));
        let tower = caps(32768, GpuClass::High, 24, false);
        assert_eq!(mesh_pool_mb(&tower), (3072, 2048));
    }

    #[test]
    fn device_tree_classifier_separates_phones_from_asahi() {
        // Real-world compatible strings (NULs already normalized to spaces).
        assert!(
            classify_mobile_from_compatible("fairphone,fp5 qcom,qcm6490"),
            "Fairphone 5 must classify mobile regardless of its 8 GB"
        );
        assert!(classify_mobile_from_compatible("qcom,sm8550-mtp qcom,sm8550"));
        assert!(classify_mobile_from_compatible("mediatek,mt8195"));
        assert!(
            !classify_mobile_from_compatible("apple,j414 apple,t6001 apple,arm-platform"),
            "Asahi Apple Silicon is the dev desktop, not a phone"
        );
        // Unknown ARM boards lean mobile: conservative budgets are the safe failure.
        assert!(classify_mobile_from_compatible("raspberrypi,4-model-b brcm,bcm2711"));
    }

    #[test]
    fn mesh_pool_keeps_the_shipped_ratio_and_scales_down_sanely() {
        for &c in &[
            caps(4096, GpuClass::Integrated, 8, true),
            caps(8192, GpuClass::Integrated, 8, true),
            caps(8192, GpuClass::Integrated, 8, false),
            caps(16384, GpuClass::Mid, 12, false),
        ] {
            let (v, i) = mesh_pool_mb(&c);
            assert_eq!(i, v * 2 / 3, "index pool tracks the shipped 3:2 ratio");
            assert!(v >= 192, "floor keeps the pool usable, got {v}");
        }
        // Monotonic in RAM within a class: more RAM never shrinks the pool.
        let mut prev = 0;
        for &ram in &[4096u32, 8192, 16384, 32768] {
            let (v, _) = mesh_pool_mb(&caps(ram, GpuClass::Integrated, 8, false));
            assert!(v >= prev, "vertex pool shrank as RAM rose at {ram} MB");
            prev = v;
        }
    }
}
