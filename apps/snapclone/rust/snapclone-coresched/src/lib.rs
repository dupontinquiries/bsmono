//! snapclone-coresched — efficiency-core classification + thread-placement
//! policy for battery savings.
//!
//! Two layers:
//!   * `topology` — parse Linux sysfs (cpu_capacity / cpufreq) into an
//!     efficiency-core / performance-core split. Works on ARM big.LITTLE (FP5)
//!     and Intel/AMD hybrid laptops; reports "uniform" (no split) on
//!     homogeneous CPUs so callers skip demotion.
//!   * `policy` — given a work CLASS (interactive, latency-sensitive, periodic
//!     background, background compute, background I/O) and the topology, decide
//!     the placement: which mechanism (none / nice / uclamp / affinity), which
//!     CPU set, what nice delta and uclamp ceiling.
//!
//! The crate decides; the CALLER applies. The C FFI below lets the C++ side
//! query a placement for a thread class, then call sched_setaffinity /
//! sched_setattr / setpriority itself (those syscalls are trivial in C++ and
//! keep the unsafe OS surface out of this pure, tested crate). The Rust thread
//! helpers (for the Rust-side worker threads — DHT, etc.) can apply directly.

pub mod policy;
pub mod topology;

pub use policy::{placement_for, Mechanism, Placement, WorkClass};
pub use topology::{classify_by_capacity, classify_by_max_freq, read_from_sysfs, CpuTopology};

// ─── C FFI ───────────────────────────────────────────────────────────────────
//
// The C++ caller does: detect topology once at startup (sc_coresched_detect),
// then for each background thread it owns, query the placement
// (sc_coresched_placement) and apply the returned mechanism with the relevant
// syscall on that thread's TID.

/// C-ABI work-class codes (match WorkClass order).
pub const SC_WORK_INTERACTIVE: u8 = 0;
pub const SC_WORK_LATENCY_SENSITIVE: u8 = 1;
pub const SC_WORK_PERIODIC_BACKGROUND: u8 = 2;
pub const SC_WORK_BACKGROUND_COMPUTE: u8 = 3;
pub const SC_WORK_BACKGROUND_IO: u8 = 4;

/// C-ABI mechanism codes (match Mechanism order).
pub const SC_MECH_NONE: u8 = 0;
pub const SC_MECH_NICE_ONLY: u8 = 1;
pub const SC_MECH_UCLAMP_EFFICIENCY: u8 = 2;
pub const SC_MECH_AFFINITY_EFFICIENCY: u8 = 3;

/// Flat C-ABI placement. `cpu_set` is written into a caller buffer separately
/// (see sc_coresched_placement) because its length varies.
#[repr(C)]
pub struct CPlacement {
    pub mechanism: u8,
    pub nice_delta: i32,
    pub uclamp_max: u32,
    /// Number of cpu ids the placement wants pinned (written to the caller's
    /// cpu_out buffer, up to cpu_out_cap).
    pub cpu_count: u32,
}

fn class_from_code(code: u8) -> WorkClass {
    match code {
        SC_WORK_INTERACTIVE => WorkClass::Interactive,
        SC_WORK_LATENCY_SENSITIVE => WorkClass::LatencySensitive,
        SC_WORK_PERIODIC_BACKGROUND => WorkClass::PeriodicBackground,
        SC_WORK_BACKGROUND_COMPUTE => WorkClass::BackgroundCompute,
        _ => WorkClass::BackgroundIo,
    }
}

fn mechanism_code(m: Mechanism) -> u8 {
    match m {
        Mechanism::None => SC_MECH_NONE,
        Mechanism::NiceOnly => SC_MECH_NICE_ONLY,
        Mechanism::UclampPreferEfficiency => SC_MECH_UCLAMP_EFFICIENCY,
        Mechanism::AffinityEfficiency => SC_MECH_AFFINITY_EFFICIENCY,
    }
}

/// Detect the CPU topology from sysfs. Returns 1 if the system is heterogeneous
/// (has efficiency cores to demote onto), 0 if uniform. The C++ side typically
/// calls this once and caches the result.
#[no_mangle]
pub extern "C" fn sc_coresched_has_efficiency_cores() -> u8 {
    if read_from_sysfs().has_efficiency_cores() { 1 } else { 0 }
}

/// Query the placement for a work class. Writes the scalar fields into `*out`
/// and the cpu id list into `cpu_out` (up to `cpu_out_cap`); `out.cpu_count` is
/// the true count (may exceed cpu_out_cap, in which case only the first
/// cpu_out_cap ids were written). Returns 0 on success, 1 on null `out`.
///
/// Re-detects the topology each call for simplicity; the C++ side may instead
/// cache via its own detect-once and call the pure policy through a future
/// topology-passing variant if this shows on a profile (it won't — it's called
/// a handful of times at thread-startup).
///
/// # Safety
/// `out` must be a valid `*mut CPlacement`; `cpu_out` valid for `cpu_out_cap`
/// `u32`s (or null with cap 0).
#[no_mangle]
pub unsafe extern "C" fn sc_coresched_placement(
    work_class: u8,
    out: *mut CPlacement,
    cpu_out: *mut u32,
    cpu_out_cap: u32,
) -> u8 {
    if out.is_null() {
        return 1;
    }
    let topo = read_from_sysfs();
    let p = placement_for(class_from_code(work_class), &topo);
    let count = p.cpu_set.len() as u32;
    (*out) = CPlacement {
        mechanism: mechanism_code(p.mechanism),
        nice_delta: p.nice_delta,
        uclamp_max: p.uclamp_max,
        cpu_count: count,
    };
    if !cpu_out.is_null() && cpu_out_cap > 0 {
        let n = (cpu_out_cap as usize).min(p.cpu_set.len());
        for (i, cpu) in p.cpu_set.iter().take(n).enumerate() {
            *cpu_out.add(i) = *cpu;
        }
    }
    0
}

#[cfg(test)]
mod ffi_tests {
    use super::*;

    #[test]
    fn ffi_placement_writes_fields() {
        // Drive the FFI as C++ would. We can't control sysfs here (the sandbox
        // reports uniform), so assert the contract holds for whatever topology
        // is detected: the call succeeds and cpu_count matches what's written.
        let mut out = CPlacement { mechanism: 255, nice_delta: -1, uclamp_max: 0, cpu_count: 0 };
        let mut cpus = [0u32; 16];
        let rc = unsafe {
            sc_coresched_placement(SC_WORK_BACKGROUND_COMPUTE, &mut out, cpus.as_mut_ptr(), 16)
        };
        assert_eq!(rc, 0);
        // Background compute is always at least nice-demoted.
        assert!(out.mechanism == SC_MECH_AFFINITY_EFFICIENCY || out.mechanism == SC_MECH_NICE_ONLY);
        assert!(out.nice_delta > 0);
        assert!(out.cpu_count <= 16);
    }

    #[test]
    fn ffi_null_out_rejected() {
        let rc = unsafe { sc_coresched_placement(0, std::ptr::null_mut(), std::ptr::null_mut(), 0) };
        assert_eq!(rc, 1);
    }
}
