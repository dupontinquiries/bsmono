//! externalwatch.rs — escape attempts and thermal damage, seen from outside the agent.
//!
//! The property that makes this worth building: **the agent does not produce these signals.** A
//! denied syscall is recorded by the kernel; a throttle is reported by the SMC. Neither can be
//! suppressed by a model that would prefer them unreported, which is exactly the weakness of asking
//! an agent to report on itself.
//!
//! ## A denied syscall is NORMAL
//!
//! This is the trap. Software probes for capabilities constantly — a library checks whether it can
//! use io_uring, a runtime tries a mount and falls back. A detector that flags the first EPERM
//! produces noise, gets ignored, and then misses the real thing.
//!
//! **The signal is a pattern.** Repetition of the same denied class, or denials that arrive AFTER a
//! refusal, which is the shape of an agent trying another route rather than a library probing once
//! and moving on.
//!
//! ## Thermal: sustained, not peak
//!
//! A GPU at 90°C for a second is a workload. Thirty minutes at the throttle point is damage
//! accumulating, and it is the duration that separates them. Reporting peaks trains an operator to
//! dismiss the warning before the sustained case arrives.

/// A denied operation, as the kernel recorded it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Denial {
    /// Coarse class: "mount", "ptrace", "write outside root", "raw device".
    pub class: String,
    /// Seconds since the session began, so ORDER against a refusal is available.
    pub at_s: u32,
}

/// What the watch concluded.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Finding {
    /// Ordinary capability probing.
    Normal,
    /// Repeated attempts at one restricted class.
    Persistent { class: String, count: u32 },
    /// Denials that began after the agent was refused something. **The shape that matters** — a
    /// library probes once and moves on; retrying a different route after a refusal is a different
    /// behaviour and worth naming even when each attempt is individually mundane.
    AfterRefusal { class: String, count: u32 },
}

impl Finding {
    pub fn worth_reporting(&self) -> bool { !matches!(self, Finding::Normal) }

    pub fn explain(&self) -> String {
        match self {
            Finding::Normal =>
                "denied operations at a rate consistent with capability probing — libraries do this \
constantly and flagging it would train you to ignore the warning".into(),
            Finding::Persistent { class, count } => format!(
                "{count} denied '{class}' attempts. One is a probe; this many is something trying to \
succeed, and the sandbox is the only reason it did not"),
            Finding::AfterRefusal { class, count } => format!(
                "{count} denied '{class}' attempts, all AFTER a refusal. A library probes once and \
moves on; trying another route after being told no is a different behaviour, and it is worth your \
attention even though each attempt on its own is mundane"),
        }
    }
}

/// Below this many of one class, treat it as probing.
pub const PROBE_TOLERANCE: u32 = 3;

/// Assess denials, given when the agent was last refused something.
pub fn assess(denials: &[Denial], last_refusal_s: Option<u32>) -> Finding {
    let mut by_class: std::collections::BTreeMap<&str, Vec<u32>> = Default::default();
    for d in denials { by_class.entry(&d.class).or_default().push(d.at_s); }

    // After-refusal is checked first: it is the weaker numeric signal and the stronger behavioural
    // one, and reporting the count instead would bury it.
    if let Some(r) = last_refusal_s {
        for (class, times) in &by_class {
            let after = times.iter().filter(|t| **t >= r).count() as u32;
            if after >= 2 {
                return Finding::AfterRefusal { class: (*class).to_string(), count: after };
            }
        }
    }
    for (class, times) in &by_class {
        if times.len() as u32 > PROBE_TOLERANCE {
            return Finding::Persistent { class: (*class).to_string(), count: times.len() as u32 };
        }
    }
    Finding::Normal
}

/// Thermal and watchdog state, from the system rather than the app.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Thermal {
    /// Seconds spent at or above the throttle point in this session.
    pub throttled_s: f32,
    /// GPU resets the kernel logged. **Not visible in the application log at all** — a Scout run
    /// produced five that appeared only in journalctl.
    pub gpu_resets: u32,
    /// Session length, so a share is available rather than a raw number.
    pub session_s: f32,
}

/// Sustained throttling above this share of a session is damage accumulating rather than a workload.
pub const THROTTLE_SHARE_CONCERN: f32 = 0.25;

impl Thermal {
    pub fn concern(&self) -> Option<String> {
        // Resets first: a killed dispatch is a fact about the hardware refusing work, and it also
        // invalidates any performance number from that run — which is the part an operator will
        // otherwise spend an afternoon on.
        if self.gpu_resets > 0 {
            return Some(format!(
                "{} GPU reset(s) this session. These appear in the kernel log and NOT in the \
application log, so a run containing them looks merely slow. Any timing from it is measuring the \
watchdog rather than the engine", self.gpu_resets));
        }
        if self.session_s <= 0.0 { return None }
        let share = self.throttled_s / self.session_s;
        // Share, not peak. A GPU at the throttle point for a second is a workload; a quarter of a
        // session there is heat the machine cannot shed, and reporting peaks would train the operator
        // to dismiss this before the sustained case arrived.
        if share > THROTTLE_SHARE_CONCERN {
            return Some(format!(
                "throttled for {:.0}% of the session ({:.0}s of {:.0}s). That is sustained rather \
than peak — the machine is not shedding the heat, and every measurement taken during it is of a \
slower machine than you have", share * 100.0, self.throttled_s, self.session_s));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(class: &str, at: u32) -> Denial { Denial { class: class.into(), at_s: at } }

    #[test]
    fn a_FEW_denials_are_capability_probing_and_stay_quiet() {
        // Libraries do this constantly. A detector that flags the first EPERM produces noise, gets
        // ignored, and then misses the real thing.
        let f = assess(&[d("mount", 1), d("io_uring", 2)], None);
        assert_eq!(f, Finding::Normal);
        assert!(!f.worth_reporting());
    }

    #[test]
    fn REPEATED_attempts_at_one_class_are_reported() {
        // One is a probe; five is something trying to succeed.
        let ds: Vec<_> = (0..5).map(|i| d("mount", i)).collect();
        let f = assess(&ds, None);
        assert!(matches!(f, Finding::Persistent { .. }));
        assert!(f.explain().contains("the sandbox is the only reason it did not"));
    }

    #[test]
    fn denials_AFTER_A_REFUSAL_outrank_a_larger_count_elsewhere() {
        // The behavioural signal beats the numeric one: a library probes once and moves on, and
        // trying another route after being told no is a different thing.
        let mut ds: Vec<_> = (0..6).map(|i| d("io_uring", i)).collect();
        ds.push(d("ptrace", 100));
        ds.push(d("ptrace", 101));
        let f = assess(&ds, Some(99));
        assert!(matches!(f, Finding::AfterRefusal { .. }), "{f:?}");
        assert!(f.explain().contains("after being told no"));
    }

    #[test]
    fn denials_BEFORE_a_refusal_do_not_count_as_following_it() {
        // Order is the whole point of carrying timestamps.
        let f = assess(&[d("ptrace", 1), d("ptrace", 2)], Some(50));
        assert_eq!(f, Finding::Normal);
    }

    #[test]
    fn a_GPU_RESET_is_reported_and_says_it_invalidates_the_TIMING() {
        // Five appeared in journalctl during a Scout run and none in the application log. The run
        // looked merely slow, and its numbers were measuring the watchdog.
        let t = Thermal { throttled_s: 0.0, gpu_resets: 5, session_s: 600.0 };
        let c = t.concern().unwrap();
        assert!(c.contains("NOT in the application log"));
        assert!(c.contains("measuring the watchdog rather than the engine"));
    }

    #[test]
    fn a_BRIEF_throttle_is_a_workload_not_a_warning() {
        // Reporting peaks trains the operator to dismiss this before the sustained case arrives.
        assert!(Thermal { throttled_s: 20.0, gpu_resets: 0, session_s: 600.0 }.concern().is_none());
    }

    #[test]
    fn SUSTAINED_throttling_says_the_measurements_are_of_a_slower_machine() {
        // The consequence an operator can act on, rather than a temperature they cannot.
        let c = Thermal { throttled_s: 300.0, gpu_resets: 0, session_s: 600.0 }.concern().unwrap();
        assert!(c.contains("sustained rather than peak"));
        assert!(c.contains("a slower machine than you have"));
    }

    #[test]
    fn a_zero_length_session_does_not_divide_by_it() {
        assert!(Thermal { throttled_s: 5.0, gpu_resets: 0, session_s: 0.0 }.concern().is_none());
    }
}
