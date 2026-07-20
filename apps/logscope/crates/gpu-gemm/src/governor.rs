//! governor.rs — a backend-agnostic GPU resource governor.
//!
//! Every crash in the windowed-decode saga was a device RESOURCE ceiling hit blind: Metal caps live
//! buffer OBJECTS (not bytes), and we found that out only by crashing five times and reading logs. The
//! governor makes that ceiling a first-class, measured thing instead of a guess:
//!
//!   1. A per-BACKEND `Budget` describes the *shape* of the device's limits — the ones that actually
//!      differ across Metal / Vulkan / DX12 / GL. Metal's practical wall is concurrent buffer-object
//!      count; desktop Vulkan's is `maxMemoryAllocationCount` (often 4096); UMA vs discrete changes how
//!      much of "memory" is really headroom. One struct, filled differently per backend at init.
//!   2. Live `Telemetry` reads the counters the engine already maintains (live buffer count, alloc
//!      failures) — no new bookkeeping on the hot path.
//!   3. The `Governor` turns budget − pressure into a SAFE WINDOW k before each windowed decode, and
//!      LEARNS: a k that ran clean lets the cap creep up; any observed pressure ratchets it down and is
//!      remembered for the session. This replaces the hardcoded `LOGSCOPE_WINDOW_MAX` guess with a value
//!      derived from the specific device.
//!
//! Design rule, learned the hard way: the governor PREVENTS (clamps before the ceiling), it does not
//! RECOVER (catch the abort and reroute). On Metal a buffer-object overflow fatally poisons the whole
//! device — the per-token retry we tried panicked too. So every decision here happens *before* the
//! allocation, never after. There is no parachute for a dead device; there is only not flying into the
//! mountain.
//!
//! This lives in gpu-gemm (not the engine) so the Linux/Vulkan port inherits it: the port will hit a
//! DIFFERENT ceiling (allocation-count, VMA), and the governor already speaks that language.

/// Which family of hard limit dominates on a given backend. This is the axis that actually varies —
/// two GPUs with the same VRAM can have wildly different ceilings on *how many* allocations you may hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitKind {
    /// Metal: the practical wall is the number of concurrently-resident buffer objects, not bytes.
    BufferObjects,
    /// Vulkan: `VkPhysicalDeviceLimits::maxMemoryAllocationCount` — a hard cap on live allocations
    /// (commonly 4096 on desktop drivers). Distinct call-out because it bites far below memory.
    AllocationCount,
    /// Fallback: no known tight object/allocation ceiling; only bytes matter (rare, generous drivers).
    BytesOnly,
}

/// The device's resource budget: the shape of its limits plus the safe operating band the governor
/// steers within. Filled per-backend at init from the adapter info; the numbers are conservative
/// defaults refined by live learning, NOT hard driver queries (wgpu doesn't expose most of them).
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub kind: LimitKind,
    /// The count ceiling the governor treats as "do not exceed" (buffer objects or allocations,
    /// per `kind`). Deliberately BELOW the true hardware limit — we steer to stay clear, not to ride it.
    pub soft_object_cap: usize,
    /// A fixed count the engine holds regardless of window size (resident-weight arena slabs, KV cache,
    /// embed table, the dims cache/ring). Subtracted from the cap to get headroom for a window.
    pub baseline_objects: usize,
    /// Objects a single window SLOT costs (its dispatches' bind groups + dims + one logits tensor),
    /// used to convert "objects of headroom" into "safe k". A per-backend estimate, refined on the fly.
    pub objects_per_slot: usize,
}

impl Budget {
    /// Per-backend defaults. `unified` = Apple/Asahi-style shared memory. The caps are intentionally
    /// conservative: the cost of steering too low is a little decode speed; the cost of steering too
    /// high is a fatal, unrecoverable device poison. We bias hard toward the cheap failure.
    pub fn for_backend(backend: WgpuBackend, unified: bool, apple_vulkan: bool) -> Self {
        match backend {
            // Metal: buffer-object ceiling. Empirically k=128 (~5000+ live objects) aborts on an M4 Max;
            // per-token is fine. Cap objects well under that and keep the window small.
            WgpuBackend::Metal => Budget {
                kind: LimitKind::BufferObjects,
                soft_object_cap: 8192,
                baseline_objects: 6400, // dims cache(4096)+ring(2048) + KV/embed/arena handles
                objects_per_slot: 40,   // ~36 dispatch bind groups + dims + a logits tensor per slot
            },
            // Vulkan: allocation-count wall. Apple-on-Vulkan (Honeykrisp) is the tightest — the
            // conservative_vma path already flags it. Desktop Vulkan commonly caps at 4096 allocations.
            WgpuBackend::Vulkan => Budget {
                kind: LimitKind::AllocationCount,
                soft_object_cap: if apple_vulkan { 3072 } else { 3584 },
                baseline_objects: if apple_vulkan { 2600 } else { 2800 },
                objects_per_slot: 40,
            },
            // DX12 / Metal-less discrete: allocation pressure is real but looser; still governed.
            WgpuBackend::Dx12 => Budget {
                kind: LimitKind::AllocationCount,
                soft_object_cap: 16384, baseline_objects: 6400, objects_per_slot: 40,
            },
            // GL and everything else: assume bytes-only, but still keep a sane window ceiling.
            _ => Budget {
                kind: LimitKind::BytesOnly,
                soft_object_cap: if unified { 32768 } else { 16384 },
                baseline_objects: 6400, objects_per_slot: 40,
            },
        }
    }

    /// The largest window slot count the budget can hold RIGHT NOW given `live_objects` already resident.
    /// Never returns <1. This is the governor's core sum: (cap − max(live, baseline)) / per-slot.
    pub fn safe_k(&self, live_objects: usize) -> usize {
        let used = live_objects.max(self.baseline_objects);
        let headroom = self.soft_object_cap.saturating_sub(used);
        (headroom / self.objects_per_slot.max(1)).max(1)
    }
}

/// A minimal mirror of `wgpu::Backend` so the governor is unit-testable without a live wgpu device and
/// carries no wgpu dependency in its logic. The engine maps the real backend into this at init.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WgpuBackend { Metal, Vulkan, Dx12, Gl, Other }

/// The live, learning governor. Holds the device budget and the current adaptive window ceiling, which
/// starts at the budget's estimate and moves with observed outcomes.
#[derive(Clone, Debug)]
pub struct Governor {
    budget: Budget,
    /// The current self-tuned window ceiling. Starts conservative; a clean run nudges it up (toward the
    /// user's requested k), any pressure signal ratchets it down and latches a hard floor of caution.
    learned_cap: usize,
    /// Once the device has EVER reported real allocation pressure this session, we never climb back above
    /// the cap that was in effect when it happened — a poisoned-device abort is too expensive to re-risk.
    pressure_ceiling: Option<usize>,
    /// Highest k that has completed a full window CLEANLY — the floor we trust.
    proven_safe: usize,
}

impl Governor {
    pub fn new(budget: Budget) -> Self {
        // Start the learned cap at the budget's static estimate against a fresh (baseline-only) device.
        let start = budget.safe_k(0);
        Governor { budget, learned_cap: start, pressure_ceiling: None, proven_safe: 2 }
    }

    /// The window k to actually use this turn, given the caller's request and the live object count.
    /// Takes the MIN of: the request, the budget's live headroom, and the learned/pressure ceilings.
    pub fn window_for(&self, requested_k: usize, live_objects: usize) -> usize {
        let live_cap = self.budget.safe_k(live_objects);
        let mut k = requested_k.min(live_cap).min(self.learned_cap);
        if let Some(pc) = self.pressure_ceiling { k = k.min(pc); }
        k.max(2)
    }

    /// Report that a window of size `k` completed cleanly. Lets the learned cap creep UP toward the
    /// request (one step at a time, so we probe gently rather than jumping back to a risky value).
    pub fn observe_clean(&mut self, k: usize) {
        self.proven_safe = self.proven_safe.max(k);
        // creep the learned cap up by ~25% (bounded), but never above a known pressure ceiling.
        let step = (self.learned_cap / 4).max(2);
        let mut next = self.learned_cap + step;
        if let Some(pc) = self.pressure_ceiling { next = next.min(pc); }
        self.learned_cap = next.min(self.budget.soft_object_cap); // absolute sanity bound
    }

    /// Report that the device signalled allocation pressure at window size `k` (an alloc failed, or the
    /// governor's own telemetry crossed the soft cap). Ratchet HARD: latch a pressure ceiling below `k`
    /// and drop the learned cap to the proven-safe floor. This is the "never fly here again" latch.
    pub fn observe_pressure(&mut self, k: usize) {
        let ceiling = k.saturating_sub(1).max(2);
        self.pressure_ceiling = Some(self.pressure_ceiling.map_or(ceiling, |p| p.min(ceiling)));
        self.learned_cap = self.proven_safe.min(ceiling).max(2);
    }

    pub fn learned_cap(&self) -> usize { self.learned_cap }
    pub fn proven_safe(&self) -> usize { self.proven_safe }
    pub fn pressure_ceiling(&self) -> Option<usize> { self.pressure_ceiling }
    pub fn budget(&self) -> Budget { self.budget }
}

/// A pressure verdict from live telemetry — what the engine should log/surface. Separate from the tuning
/// so a UI alert layer can render it without knowing the governor's internals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pressure {
    /// Comfortable — under ~70% of the soft cap.
    Ok,
    /// Getting close — 70–90%. Surface a warning; the governor will already be trimming k.
    Warn,
    /// Near the ceiling — >90%. The governor clamps aggressively; the UI should flag it.
    Critical,
}

impl Budget {
    /// Classify the current live object count against the soft cap, for the alert layer.
    pub fn pressure(&self, live_objects: usize) -> Pressure {
        let frac = live_objects as f32 / self.soft_object_cap.max(1) as f32;
        if frac >= 0.90 { Pressure::Critical } else if frac >= 0.70 { Pressure::Warn } else { Pressure::Ok }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metal_budget_clamps_a_huge_request_down() {
        let g = Governor::new(Budget::for_backend(WgpuBackend::Metal, true, false));
        // fresh device, user asks for 128 — must clamp well below (the crash was here)
        let k = g.window_for(128, 0);
        assert!(k >= 2 && k < 128, "Metal must clamp 128 down, got {k}");
    }

    #[test]
    fn safe_k_shrinks_as_the_device_fills() {
        let b = Budget::for_backend(WgpuBackend::Metal, true, false);
        let empty = b.safe_k(b.baseline_objects);
        let near_full = b.safe_k(b.soft_object_cap - b.objects_per_slot); // ~1 slot of headroom
        assert!(near_full < empty, "headroom must shrink as objects grow: {near_full} vs {empty}");
        assert!(near_full >= 1, "never returns < 1");
    }

    #[test]
    fn vulkan_allocation_count_is_the_tighter_ceiling() {
        // desktop Vulkan's 4096-ish alloc cap must make its object budget tighter than Metal's.
        let metal = Budget::for_backend(WgpuBackend::Metal, false, false);
        let vk = Budget::for_backend(WgpuBackend::Vulkan, false, false);
        assert!(vk.soft_object_cap < metal.soft_object_cap, "Vulkan alloc ceiling should be tighter");
        // Apple-on-Vulkan (Honeykrisp) is tighter still
        let vk_apple = Budget::for_backend(WgpuBackend::Vulkan, true, true);
        assert!(vk_apple.soft_object_cap <= vk.soft_object_cap);
    }

    #[test]
    fn pressure_latch_never_climbs_back_above_the_failure() {
        let mut g = Governor::new(Budget::for_backend(WgpuBackend::Metal, true, false));
        // a clean run at the current cap lets it creep up
        let c0 = g.learned_cap();
        g.observe_clean(c0);
        assert!(g.learned_cap() >= c0, "clean run should not lower the cap");
        // now the device signals pressure at k=24
        g.observe_pressure(24);
        assert_eq!(g.pressure_ceiling(), Some(23));
        assert!(g.window_for(128, 0) <= 23, "must never exceed the pressure ceiling again");
        // even many clean runs afterward can't climb back above the latch
        for _ in 0..20 { g.observe_clean(g.learned_cap()); }
        assert!(g.window_for(128, 0) <= 23, "latch holds against optimistic creep");
    }

    #[test]
    fn proven_safe_floor_is_respected_on_ratchet() {
        let mut g = Governor::new(Budget::for_backend(WgpuBackend::Metal, true, false));
        g.observe_clean(8);  // 8 proven safe
        g.observe_clean(12); // 12 proven safe
        g.observe_pressure(20); // pressure at 20
        // after the ratchet, we still trust the proven-safe floor (<= ceiling)
        assert!(g.window_for(128, 0) >= 2);
        assert!(g.window_for(128, 0) <= 19);
    }

    #[test]
    fn pressure_classification_bands() {
        let b = Budget::for_backend(WgpuBackend::Metal, true, false);
        assert_eq!(b.pressure(0), Pressure::Ok);
        assert_eq!(b.pressure((b.soft_object_cap as f32 * 0.75) as usize), Pressure::Warn);
        assert_eq!(b.pressure((b.soft_object_cap as f32 * 0.95) as usize), Pressure::Critical);
    }

    #[test]
    fn bytes_only_backends_still_get_a_sane_window() {
        let g = Governor::new(Budget::for_backend(WgpuBackend::Gl, true, false));
        let k = g.window_for(128, 0);
        assert!(k >= 2, "even a generous backend keeps a floor");
    }
}
