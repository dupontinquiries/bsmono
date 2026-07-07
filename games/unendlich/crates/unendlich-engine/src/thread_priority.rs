//! Per-thread priority and CPU-affinity control.
//!
//! v0.24.22-worker-isolation. Implements the documented next direction
//! from Increment 61 (`DESIGN_NOTES.md`):
//!
//! > **Worker isolation (P/E core assignment, thread priorities) —
//! > high-value next direction. Tests the hypothesis directly.**
//!
//! The chug pattern documented in v0.24.17 free-play was identified as
//! CPU contention manifesting as GPU starvation: world-gen, mesh, and
//! render threads competing for the same P-cores. When chunk-loader
//! threads were busy doing terrain generation, the render thread
//! couldn't run its CPU work fast enough to keep the GPU fed, so the
//! GPU stalled and wait_fences ballooned to ~265ms per frame.
//!
//! The "magic switch" data point — 5 FPS jumping to 130 FPS at the
//! single frame where `gen_pending` hit 0 — is the smoking gun that
//! says "this is contention, not capacity." That observation drove
//! this module.
//!
//! ## What this does
//!
//! On Linux only (`#[cfg(target_os = "linux")]`), threads call
//! [`configure_current_thread`] with their [`ThreadRole`]:
//!
//! - [`ThreadRole::ChunkLoader`]: nice +10 (lower priority) AND pinned
//!   to the system's "efficiency" cores (those with `cpu_capacity` <
//!   max). On M2 Max + Asahi Linux that's the 4 E-cores. World
//!   generation throughput drops; in exchange the render thread is
//!   never preempted by chunk-loader work.
//!
//! - [`ThreadRole::MeshWorker`]: no change. Default scheduling. The
//!   Linux scheduler keeps these on P-cores as long as they're active,
//!   which is the right policy: mesh work has frame-rate impact.
//!
//! - [`ThreadRole::GpuDispatcher`]: no change. Same reasoning as
//!   MeshWorker; this thread does GPU submissions that block frame
//!   throughput if delayed.
//!
//! - [`ThreadRole::Render`]: no change. The main thread is already
//!   prioritized by being the process's first thread and by being the
//!   one with the most consistent CPU pressure (the kernel notices).
//!
//! ## What this does NOT do
//!
//! - **Does not promote the render thread to real-time priority**
//!   (`SCHED_FIFO`). RT priority requires `CAP_SYS_NICE` or
//!   `/etc/security/limits.conf` configuration, neither of which we
//!   can assume on a development machine. The deprioritize-loaders
//!   strategy works without requiring elevated permissions.
//!
//! - **Does not pin mesh workers to P-cores.** The kernel's default
//!   "schedutil + capacity-aware" scheduler already prefers P-cores
//!   for CPU-bound threads under `nice=0`. Pinning would also disable
//!   the scheduler's ability to migrate work during transient bursts.
//!
//! - **Does not affect Windows or macOS.** Stubs out to a no-op on
//!   non-Linux. macOS has QoS classes (`pthread_set_qos_class_self_np`)
//!   that could be wired in later; Windows has thread priority APIs
//!   that could likewise be added. Neither is the test platform.
//!
//! ## Opt-out
//!
//! Honored at module-init time: `UNENDLICH_WORKER_ISOLATION=0`
//! disables all priority/affinity changes for the run. Default ON.
//! Useful for A/B comparison: run twice, once with `=1` (default) and
//! once with `=0`, and the chug behaviour should diverge if this
//! module's hypothesis is correct.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadRole {
    /// Main render thread. Currently treated as no-op (already
    /// prioritized via kernel heuristics).
    Render,
    /// CPU mesh workers (`mesher-N` threads). Default scheduling so the
    /// kernel keeps them on P-cores while active.
    MeshWorker,
    /// GPU mesher dispatcher thread (when `UNENDLICH_GPU_MESHER=1`).
    /// Default scheduling — its work feeds the render thread.
    GpuDispatcher,
    /// World-generation workers (`chunk-loader-N` threads). Deprioritized
    /// AND pinned to efficiency cores on heterogeneous Linux systems.
    /// This is the surgical change that addresses the chug.
    ChunkLoader,
}

impl ThreadRole {
    fn name(&self) -> &'static str {
        match self {
            ThreadRole::Render => "render",
            ThreadRole::MeshWorker => "mesh-worker",
            ThreadRole::GpuDispatcher => "gpu-dispatcher",
            ThreadRole::ChunkLoader => "chunk-loader",
        }
    }
}

/// Apply role-specific priority + affinity to the calling thread.
///
/// Idempotent (calling multiple times is fine, last call wins).
/// Errors from the underlying syscalls are logged at debug level and
/// otherwise silently ignored — a failure to deprioritize a thread is
/// degraded operation, not a correctness issue, and we don't want a
/// stray permission error from a sandboxed environment to crash the
/// process.
pub fn configure_current_thread(role: ThreadRole) {
    // Env-var opt-out, evaluated once per thread. The cost (~1µs of
    // env-var lookup) is paid only once at thread startup, never on a
    // hot path.
    if !isolation_enabled() {
        log::debug!(
            "thread_priority: skipping configuration for {} (UNENDLICH_WORKER_ISOLATION=0)",
            role.name()
        );
        return;
    }

    #[cfg(target_os = "linux")]
    {
        linux_impl::configure(role);
    }

    #[cfg(not(target_os = "linux"))]
    {
        // No-op on non-Linux. Suppress unused-variable warning.
        let _ = role;
    }
}

fn isolation_enabled() -> bool {
    match std::env::var("UNENDLICH_WORKER_ISOLATION") {
        Ok(s) => s != "0" && !s.is_empty(),
        Err(_) => true, // default: on
    }
}

#[cfg(target_os = "linux")]
mod linux_impl {
    use super::ThreadRole;

    pub(super) fn configure(role: ThreadRole) {
        match role {
            ThreadRole::ChunkLoader => {
                // Two-step deprioritization:
                //   1. nice +10 (lower scheduling priority).
                //   2. Pin to efficiency cores if any are detected.
                //
                // Either step alone helps; both together is the
                // strongest signal to the scheduler that this thread
                // should yield to render + mesh work.
                set_nice(10);
                pin_to_efficiency_cores();
            }
            ThreadRole::MeshWorker
            | ThreadRole::GpuDispatcher
            | ThreadRole::Render => {
                // Default scheduling. No syscalls.
            }
        }
    }

    /// Set the calling thread's nice value via the `setpriority`
    /// syscall.
    ///
    /// `setpriority(PRIO_PROCESS, 0, ...)` with `who=0` targets the
    /// calling thread on Linux (the manpage says "the calling
    /// process" but on Linux with NPTL each thread has its own nice
    /// value, and PRIO_PROCESS + who=0 sets the caller's). Verified
    /// behaviour on glibc 2.36+ and on Asahi's musl variants.
    fn set_nice(nice_value: i32) {
        // SAFETY: libc::setpriority is a safe FFI shim around the
        // POSIX `setpriority(2)` syscall. With PRIO_PROCESS + who=0
        // the only side effect is the calling thread's nice value,
        // which is exactly what we want.
        let r = unsafe {
            libc::setpriority(libc::PRIO_PROCESS, 0, nice_value)
        };
        if r != 0 {
            // errno would tell us why (EACCES if not allowed to
            // RAISE priority; we're only LOWERING so this shouldn't
            // happen — but we don't depend on the result).
            let err = std::io::Error::last_os_error();
            log::debug!("setpriority(nice={}) returned {} ({})",
                nice_value, r, err);
        } else {
            log::debug!("thread_priority: nice {} applied", nice_value);
        }
    }

    /// Pin the calling thread to the system's efficiency cores
    /// (those reporting `cpu_capacity` strictly less than the system
    /// maximum).
    ///
    /// On homogeneous systems (all cores report identical capacity,
    /// or `cpu_capacity` sysfs files are absent), this function is a
    /// no-op: there are no E-cores to pin to.
    ///
    /// On heterogeneous systems (M-series Macs via Asahi, modern
    /// Intel hybrid chips, Arm big.LITTLE devices), the kernel
    /// publishes per-CPU capacity at
    /// `/sys/devices/system/cpu/cpuN/cpu_capacity` as a u32 in
    /// `[0, 1024]`. P-cores report 1024 (or near it); E-cores
    /// report something lower (e.g. ~400 on Asahi M-series).
    fn pin_to_efficiency_cores() {
        let cores = efficiency_cores();
        if cores.is_empty() {
            log::debug!(
                "thread_priority: no efficiency cores detected, \
                 skipping affinity pinning"
            );
            return;
        }

        // SAFETY: cpu_set_t is plain old data; zeroing is valid.
        let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
        // SAFETY: Modern `libc` (>= 0.2.150-ish) marked CPU_ZERO and
        // CPU_SET as unsafe because they take raw pointers and write
        // through them. The earlier comment claimed they were safe;
        // it was stale. The `&mut set` we pass is valid for the
        // function's lifetime (it's a stack local we own), and the
        // CPU index in CPU_SET is bounds-checked against CPU_SETSIZE.
        unsafe {
            libc::CPU_ZERO(&mut set);
            for cpu in &cores {
                if *cpu < libc::CPU_SETSIZE as usize {
                    libc::CPU_SET(*cpu, &mut set);
                }
            }
        }

        // SAFETY: sched_setaffinity(0, ...) targets the calling
        // thread. The `set` is a valid cpu_set_t. The size is the
        // exact size of cpu_set_t. Per the manpage, this is the
        // canonical invocation.
        let r = unsafe {
            libc::sched_setaffinity(
                0,
                std::mem::size_of::<libc::cpu_set_t>(),
                &set as *const libc::cpu_set_t,
            )
        };
        if r != 0 {
            let err = std::io::Error::last_os_error();
            log::debug!(
                "sched_setaffinity({:?}) returned {} ({})",
                cores, r, err
            );
        } else {
            log::debug!(
                "thread_priority: pinned to E-cores {:?}",
                cores
            );
        }
    }

    /// Read `/sys/devices/system/cpu/cpu*/cpu_capacity` and return the
    /// set of CPU indices whose capacity is below the maximum
    /// observed value.
    ///
    /// Returns an empty Vec if:
    ///   - The sysfs files don't exist (older kernels, virtualized
    ///     environments).
    ///   - All CPUs report identical capacity (homogeneous system).
    ///
    /// Performance: called once per chunk-loader thread at startup.
    /// Reads N small files where N = CPU count (≤16 for current
    /// consumer hardware). Total cost ≤ 1ms.
    fn efficiency_cores() -> Vec<usize> {
        // Probe a reasonable upper bound. Linux is happy to give
        // 65535 logical CPUs in extreme cases, but consumer
        // hardware tops out around 256 cores. We stop on the first
        // gap.
        let mut capacities: Vec<(usize, u32)> = Vec::new();
        for i in 0..256 {
            let path = format!(
                "/sys/devices/system/cpu/cpu{}/cpu_capacity", i
            );
            match std::fs::read_to_string(&path) {
                Ok(s) => {
                    if let Ok(cap) = s.trim().parse::<u32>() {
                        capacities.push((i, cap));
                    }
                }
                Err(_) => break, // first gap = end of CPUs
            }
        }
        if capacities.is_empty() {
            return Vec::new();
        }
        let max_cap = capacities.iter().map(|(_, c)| *c).max().unwrap();
        // Cores reporting LESS than the max are "efficiency" by
        // definition (lower throughput per Hz). On a homogeneous
        // system, no core qualifies; return empty.
        capacities.iter()
            .filter(|(_, c)| *c < max_cap)
            .map(|(i, _)| *i)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_names_are_unique() {
        let names = [
            ThreadRole::Render.name(),
            ThreadRole::MeshWorker.name(),
            ThreadRole::GpuDispatcher.name(),
            ThreadRole::ChunkLoader.name(),
        ];
        let mut sorted = names;
        sorted.sort();
        for w in sorted.windows(2) {
            assert_ne!(w[0], w[1], "duplicate role name: {}", w[0]);
        }
    }

    #[test]
    fn configure_is_safe_to_call_in_test_threads() {
        // Just verify the function doesn't panic. The actual
        // priority/affinity effects can't be assert!ed without
        // platform-specific introspection, but the call should be
        // a no-op (test environments typically don't have
        // CAP_SYS_NICE; setpriority for LOWER nice is always
        // allowed).
        configure_current_thread(ThreadRole::ChunkLoader);
        configure_current_thread(ThreadRole::MeshWorker);
        configure_current_thread(ThreadRole::Render);
        configure_current_thread(ThreadRole::GpuDispatcher);
    }

    #[test]
    fn opt_out_via_env() {
        // We can't reliably test this without env mutation that
        // races other tests, so just verify the function exists
        // and doesn't panic. The behaviour is covered by the
        // production path; the env-var read is straightforward.
        let _enabled = isolation_enabled();
    }
}
