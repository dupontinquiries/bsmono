// src/rt_sched.rs
#![allow(dead_code)]
//
// Real-time scheduling hint for the volume worker thread.
//
// The kernel's default scheduler (SCHED_OTHER, CFS-based) is fair but
// not predictable: under system load, the worker thread can get
// preempted for tens of milliseconds at a time. For a controller
// where the user expects sub-100 ms gesture-to-audio response, those
// jitter spikes are noticeable.
//
// On Linux we can ask for SCHED_RR (round-robin real-time scheduling)
// which gets priority over normal threads. The catch: it requires
// CAP_SYS_NICE on the process, or membership in a group with that
// capability via PAM. The typical setup is:
//
//   * The user is in the `audio` group.
//   * /etc/security/limits.conf has `@audio - rtprio 95` (or similar).
//
// On systems without those privileges, sched_setscheduler returns
// EPERM. We treat that as expected: log it once, continue with default
// scheduling, and the controller works the same as before — just with
// slightly more jitter under load.
//
// Priority choice: 10 is a moderate setting that competes well with
// pulseaudio's RT threads (usually 10-15) without disrupting them.

use std::sync::atomic::{AtomicBool, Ordering};

static REQUESTED_ONCE: AtomicBool = AtomicBool::new(false);

/// Default RT priority for the worker thread. Picked to be visible to
/// the kernel scheduler without competing with audio-server threads.
pub const WORKER_RT_PRIORITY: i32 = 10;

/// Try to elevate the calling thread to SCHED_RR at `prio`. Returns
/// Ok(()) on success, Err with a description otherwise. The function
/// is a no-op on non-Linux targets.
///
/// Idempotent: only emits the diagnostic message on the first call so
/// repeated invocations don't spam stderr.
pub fn request_rt_priority(prio: i32) -> Result<(), String> {
    let first_call = !REQUESTED_ONCE.swap(true, Ordering::SeqCst);

    #[cfg(target_os = "linux")]
    {
        // Use libc directly so we don't pull in a heavier RT crate just
        // for one syscall. SCHED_RR == 2; sched_param is { sched_priority: i32 }.
        // We bind the function from libc via the standard `extern "C"`
        // path. This works without the libc crate dependency.
        const SCHED_RR: i32 = 2;
        #[repr(C)]
        struct SchedParam { sched_priority: i32 }
        extern "C" {
            fn sched_setscheduler(pid: i32, policy: i32, param: *const SchedParam) -> i32;
        }
        let param = SchedParam { sched_priority: prio };
        // pid=0 means "current thread" on Linux.
        let rc = unsafe { sched_setscheduler(0, SCHED_RR, &param) };
        if rc == 0 {
            if first_call {
                eprintln!("rt_sched: thread now SCHED_RR prio={}", prio);
            }
            Ok(())
        } else {
            let errno = std::io::Error::last_os_error();
            let msg = format!("sched_setscheduler failed: {} (likely missing CAP_SYS_NICE; \
                              add user to `audio` group + raise rtprio limit)", errno);
            if first_call { eprintln!("rt_sched: {}", msg); }
            Err(msg)
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        if first_call {
            eprintln!("rt_sched: not Linux — leaving scheduler defaults");
        }
        Err("not Linux".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_constant_is_reasonable() {
        // RT priorities are 1-99 on Linux. We want something visible
        // but not competing with audio servers at top end.
        assert!(WORKER_RT_PRIORITY >= 1);
        assert!(WORKER_RT_PRIORITY <= 90);
    }

    #[test]
    fn request_rt_priority_doesnt_panic() {
        // Either succeeds (we're root or have CAP_SYS_NICE) or returns
        // a descriptive error. Never panics.
        let _ = request_rt_priority(WORKER_RT_PRIORITY);
    }
}
