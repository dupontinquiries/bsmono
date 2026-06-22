//! cpu_efficiency: bias NON-latency-sensitive background voice work
//! onto efficiency (E / LITTLE) cores on heterogeneous phones and
//! laptops, via Linux util-clamp (uclamp).
//!
//! Mechanism (verified against sched_setattr(2) + kernel UAPI):
//! uclamp is a per-task hint (kernel >= 5.3, CONFIG_UCLAMP_TASK) set
//! through sched_setattr with SCHED_FLAG_UTIL_CLAMP_MAX. Capping a
//! task's util_max below the little-core capacity (on the 0..1024
//! scale) BIASES the EAS scheduler to place it on an E-core AND caps
//! the clock it can request (on the schedutil governor). See
//! docs/design/uclamp_ecore_bias.md.
//!
//! Honest properties (all handled here):
//!   - It's a BIAS, not a pin (chosen on purpose — cooperates with
//!     EAS instead of fighting it like a hard affinity mask would).
//!   - Frequency effect only on the schedutil governor; placement
//!     bias may still apply elsewhere via EAS. Best-effort.
//!   - Needs kernel >= 5.3 + CONFIG_UCLAMP_TASK; probed at runtime.
//!   - NEVER panics, never errors the pipeline — a no-op on any
//!     failure (non-Linux, old kernel, no privilege).
//!
//! SCOPE: only call this from background threads where a little extra
//! latency is invisible (corpus write, opus encode, embedding,
//! dead-air analysis). NEVER from the wake-word / live-ASR path the
//! user is waiting on — clamping that would hurt responsiveness.
//!
//! libc 0.2 exposes SYS_sched_setattr but not the struct/flags, so we
//! define the stable UAPI struct + flags and call via libc::syscall.

/// E-core util_max cap on the 0..1024 scale. 256 (25%) is conservative
/// — at/under typical little-core capacity so placement biases to E
/// while a clamped batch task still makes progress. Tunable.
pub const ECORE_UTIL_MAX: u32 = 256;

// Kernel UAPI constants (include/uapi/linux/sched/types.h + sched.h).
#[allow(dead_code)]
const SCHED_ATTR_SIZE_VER1: u32 = 56; // size with util_{min,max}
#[allow(dead_code)]
const SCHED_FLAG_KEEP_POLICY: u64 = 0x08;
#[allow(dead_code)]
const SCHED_FLAG_UTIL_CLAMP_MIN: u64 = 0x20;
#[allow(dead_code)]
const SCHED_FLAG_UTIL_CLAMP_MAX: u64 = 0x40;

/// Extended scheduling parameters — stable kernel ABI (VER1, 56 bytes,
/// verified). Field order MUST match include/uapi/linux/sched/types.h.
#[repr(C)]
#[allow(dead_code)]
struct SchedAttr {
    size: u32,
    sched_policy: u32,
    sched_flags: u64,
    sched_nice: i32,
    sched_priority: u32,
    sched_runtime: u64,
    sched_deadline: u64,
    sched_period: u64,
    sched_util_min: u32,
    sched_util_max: u32,
}

impl SchedAttr {
    /// Build an attr that ONLY clamps util_max (leaves policy + the
    /// other fields untouched via KEEP_POLICY; util_min stays 0 — we
    /// never boost these tasks, only cap). `util_max` is clamped into
    /// the valid 0..=1024 range defensively (out-of-range → EINVAL).
    fn util_max_clamp(util_max: u32) -> Self {
        let umax = util_max.min(1024);
        SchedAttr {
            size: SCHED_ATTR_SIZE_VER1,
            sched_policy: 0, // ignored (KEEP_POLICY)
            sched_flags: SCHED_FLAG_KEEP_POLICY | SCHED_FLAG_UTIL_CLAMP_MAX,
            sched_nice: 0,
            sched_priority: 0,
            sched_runtime: 0,
            sched_deadline: 0,
            sched_period: 0,
            sched_util_min: 0,
            sched_util_max: umax,
        }
    }
}

/// Clamp the CURRENT thread's util_max so the scheduler biases it onto
/// an efficiency core. Call as the first statement inside a background
/// worker thread. Returns true if the hint was accepted, false on any
/// failure (no-op, never panics, never errors the caller).
///
/// On non-Linux this is always a no-op returning false.
#[cfg(target_os = "linux")]
pub fn clamp_current_thread_to_ecores() -> bool {
    clamp_tid_util_max(0, ECORE_UTIL_MAX)
}

#[cfg(not(target_os = "linux"))]
pub fn clamp_current_thread_to_ecores() -> bool {
    false
}

/// Runtime probe: does this kernel accept a uclamp hint? Tries to set
/// util_max=1024 (a no-op clamp) on the current thread; if that
/// succeeds, CONFIG_UCLAMP_TASK is present and we have privilege.
/// Restores nothing (1024 is the default = unclamped).
#[cfg(target_os = "linux")]
pub fn is_supported() -> bool {
    clamp_tid_util_max(0, 1024)
}

#[cfg(not(target_os = "linux"))]
pub fn is_supported() -> bool {
    false
}

/// The actual syscall. pid=0 means the calling thread. Returns true on
/// success. Any error (ENOSYS old kernel, EOPNOTSUPP no CONFIG_UCLAMP,
/// EPERM no privilege, EINVAL) → false, silently.
#[cfg(target_os = "linux")]
fn clamp_tid_util_max(pid: libc::pid_t, util_max: u32) -> bool {
    let attr = SchedAttr::util_max_clamp(util_max);
    // sched_setattr(pid, &attr, flags=0)
    let ret = unsafe {
        libc::syscall(
            libc::SYS_sched_setattr,
            pid,
            &attr as *const SchedAttr,
            0u32,
        )
    };
    ret == 0
}

/// Restore the current thread to UNCLAMPED (util_max = 1024 = default).
/// Used by the conditional guard to lift an E-core bias once a
/// background op finishes (so the thread, if reused, isn't left
/// throttled). No-op on non-Linux / failure.
#[cfg(target_os = "linux")]
pub fn unclamp_current_thread() -> bool {
    clamp_tid_util_max(0, 1024)
}

#[cfg(not(target_os = "linux"))]
pub fn unclamp_current_thread() -> bool {
    false
}

/// RAII guard that biases the current thread onto efficiency cores
/// ONLY while a caller-supplied "should defer to efficiency" condition
/// holds (e.g. the app is backgrounded / screen off), and restores the
/// thread to unclamped on drop.
///
/// The intent (user's): background operations whose speed does NOT
/// improve the user experience — fingerprint-DB persistence, stale
/// sweeps, index compaction — should take the low-power path when the
/// screen is off, but run unthrottled when the user is present (so a
/// foreground operation is never slowed). The guard captures that
/// "only when it can't hurt UX" rule in one place.
///
/// Usage:
///   let _eco = EcoGuard::if_backgrounded();
///   // ... heavy background FP work ...
/// // clamp lifted here on drop.
pub struct EcoGuard {
    clamped: bool,
}

impl EcoGuard {
    /// Clamp now iff `defer` is true; otherwise a no-op guard that
    /// leaves the thread at full performance. `defer` is the caller's
    /// "safe to be slow" predicate (typically is_app_backgrounded()).
    pub fn new(defer: bool) -> Self {
        let clamped = if defer { clamp_current_thread_to_ecores() } else { false };
        EcoGuard { clamped }
    }

    /// True if this guard actually applied a clamp (for tests/logging).
    pub fn is_clamped(&self) -> bool { self.clamped }
}

impl Drop for EcoGuard {
    fn drop(&mut self) {
        if self.clamped {
            // Lift the bias so a pooled/reused thread isn't left
            // throttled for later foreground work.
            unclamp_current_thread();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_size_matches_kernel_ver1() {
        // The kernel expects size >= SCHED_ATTR_SIZE_VER1 (56) for
        // util fields. Our struct must be exactly that.
        assert_eq!(std::mem::size_of::<SchedAttr>(), 56);
        assert_eq!(SCHED_ATTR_SIZE_VER1, 56);
    }

    #[test]
    fn eco_guard_no_clamp_when_not_deferring() {
        // defer=false → never clamps, regardless of platform support.
        let g = EcoGuard::new(false);
        assert!(!g.is_clamped(), "foreground / present → no clamp");
        // dropping a non-clamping guard is a no-op (no unclamp call).
    }

    #[test]
    fn eco_guard_attempts_clamp_when_deferring() {
        // defer=true → attempts a clamp. On a sandbox without uclamp
        // support is_clamped() may be false (graceful), but it must
        // not panic, and drop must be safe.
        let g = EcoGuard::new(true);
        let _ = g.is_clamped(); // either true (supported) or false (no-op)
        drop(g); // must not panic
    }

    #[test]
    fn util_max_attr_has_correct_flags_and_value() {
        let a = SchedAttr::util_max_clamp(ECORE_UTIL_MAX);
        // only the max-clamp + keep-policy flags, not the min flag
        assert_eq!(a.sched_flags, SCHED_FLAG_KEEP_POLICY | SCHED_FLAG_UTIL_CLAMP_MAX);
        assert_eq!(a.sched_flags & SCHED_FLAG_UTIL_CLAMP_MIN, 0, "must not set min-clamp");
        assert_eq!(a.sched_util_max, ECORE_UTIL_MAX);
        assert_eq!(a.sched_util_min, 0, "we cap, never boost");
        assert_eq!(a.size, 56);
    }

    #[test]
    fn util_max_clamped_into_valid_range() {
        // out-of-range util_max would make the syscall EINVAL; we clamp
        // to 1024 defensively.
        let a = SchedAttr::util_max_clamp(99999);
        assert_eq!(a.sched_util_max, 1024);
        let b = SchedAttr::util_max_clamp(0);
        assert_eq!(b.sched_util_max, 0);
    }

    #[test]
    fn ecore_cap_is_conservative() {
        // must be <= 1024 and low enough to plausibly sit at/under a
        // little-core's capacity (well under half the max).
        assert!(ECORE_UTIL_MAX <= 1024);
        assert!(ECORE_UTIL_MAX < 512, "should bias toward E-cores");
    }

    #[test]
    fn flag_constants_match_kernel_uapi() {
        // sanity: the three flags are the documented bit values.
        assert_eq!(SCHED_FLAG_KEEP_POLICY, 0x08);
        assert_eq!(SCHED_FLAG_UTIL_CLAMP_MIN, 0x20);
        assert_eq!(SCHED_FLAG_UTIL_CLAMP_MAX, 0x40);
    }

    #[test]
    fn non_clamp_probe_value() {
        // is_supported's probe uses 1024 = the unclamped default, so a
        // successful probe leaves the thread effectively unclamped.
        let a = SchedAttr::util_max_clamp(1024);
        assert_eq!(a.sched_util_max, 1024);
    }
}
