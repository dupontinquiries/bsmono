//! deviceplan.rs — what each machine in a session is FOR.
//!
//! `modelfit` scores a model for a role. Nothing scores a DEVICE for a task, and the two are not the
//! same question: a machine useless for inference can be essential for a build check, because it has
//! a different architecture and that is the whole point of running one there.
//!
//! ## The constraints invert between task kinds
//!
//! **Inference needs bandwidth; a build needs only the source.** Sending a prompt and receiving
//! tokens is small, but a distributed model needs weights or activations moving continuously, and a
//! slow link makes that worse than not using the device at all. A cross-compile ships a few MiB of
//! source and returns a yes or no.
//!
//! So a laptop on wifi is a bad inference peer and a perfectly good build peer, and a single
//! "is this device usable" score gets one of those wrong.
//!
//! ## Latency tolerance is per task, not per device
//!
//! A draft model must answer in milliseconds or speculation loses. A build check may take thirty
//! seconds and cost nothing, because it runs while the operator reads something else.

/// What a device is being asked to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Task {
    /// Generate committed tokens. Needs the model resident and the link fast.
    Inference,
    /// Draft tokens for a target to verify. Needs to be FAST above all — a slow draft is worse than
    /// none, because verification then waits on it.
    Draft,
    /// Summarise, compress, pre-read. Latency hides behind the main loop.
    Delegate,
    /// Compile and test. **Needs an architecture, not a GPU** — this is the task a "useless" device
    /// is uniquely good at.
    BuildCheck,
    /// Run a produced binary. Needs the same architecture as the build and a working userland.
    NativeRun,
}

impl Task {
    /// Seconds of round trip this task can absorb before it stops being worth delegating.
    pub fn latency_budget_s(self) -> f32 {
        match self {
            // Verification waits on it, so a slow draft is worse than none.
            Task::Draft => 0.05,
            Task::Inference => 1.0,
            // Runs ahead of the main loop, so its latency hides entirely.
            Task::Delegate => 10.0,
            // Runs while the operator reads something else.
            Task::BuildCheck => 120.0,
            Task::NativeRun => 60.0,
        }
    }

    /// Does this task move enough data that link bandwidth decides it?
    pub fn bandwidth_bound(self) -> bool {
        matches!(self, Task::Inference | Task::Draft)
    }
}

/// What is known about a machine.
#[derive(Clone, PartialEq, Debug)]
pub struct Device {
    pub name: String,
    /// e.g. "aarch64-unknown-linux-gnu". **The reason a GPU-less device has value.**
    pub target_triple: String,
    /// Usable GPU memory, or zero for no usable accelerator.
    pub gpu_gib: f32,
    /// Measured round trip to this device.
    pub rtt_s: f32,
    /// Measured link, MiB/s.
    pub link_mib_s: f32,
    /// Can it execute binaries for its own triple? False for a build-only container.
    pub can_execute: bool,
}

/// Why a device cannot take a task.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Unfit { TooSlow, NoAccelerator, CannotExecute, LinkTooNarrow }

impl Unfit {
    pub fn explain(&self, t: Task) -> String {
        match self {
            Unfit::TooSlow => format!(
                "round trip exceeds what {t:?} tolerates ({:.2}s) — delegating would cost more than \
doing it locally", t.latency_budget_s()),
            Unfit::NoAccelerator =>
                "no usable GPU, which this task needs. It may still be the best machine available \
for a build check, where an architecture matters and an accelerator does not".into(),
            Unfit::CannotExecute =>
                "cannot run binaries — a build-only environment can compile for a target it cannot \
execute, and the distinction matters because a green build there is not a passing test".into(),
            Unfit::LinkTooNarrow =>
                "the link is too narrow for a task that moves activations continuously. The same \
device is unaffected for a build, which ships source once and returns a verdict".into(),
        }
    }
}

/// Minimum link for bandwidth-bound work, MiB/s. Below this a remote model is slower than a local
/// one however fast the remote machine is.
pub const MIN_LINK_MIB_S: f32 = 100.0;

/// Can this device take this task?
pub fn fits(d: &Device, t: Task) -> Result<(), Unfit> {
    if d.rtt_s > t.latency_budget_s() { return Err(Unfit::TooSlow) }
    if t.bandwidth_bound() && d.link_mib_s < MIN_LINK_MIB_S { return Err(Unfit::LinkTooNarrow) }
    match t {
        Task::Inference | Task::Draft | Task::Delegate if d.gpu_gib <= 0.0 => Err(Unfit::NoAccelerator),
        Task::NativeRun if !d.can_execute => Err(Unfit::CannotExecute),
        _ => Ok(()),
    }
}

/// Which devices can verify a build for a target this session cares about.
///
/// **Cross-compilation is not the same as native verification.** A build on x86 for aarch64 catches
/// type errors and misses everything about the target's alignment, endianness and available
/// syscalls. A device with the matching triple that can execute is worth more than several that
/// cannot, and the return says which is which.
pub fn build_coverage<'a>(devices: &'a [Device], target: &str) -> (Vec<&'a Device>, Vec<&'a Device>) {
    let native: Vec<_> = devices.iter()
        .filter(|d| d.target_triple == target && d.can_execute && fits(d, Task::NativeRun).is_ok())
        .collect();
    let cross: Vec<_> = devices.iter()
        .filter(|d| d.target_triple != target && fits(d, Task::BuildCheck).is_ok())
        .collect();
    (native, cross)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str, triple: &str, gpu: f32, rtt: f32, link: f32, exec: bool) -> Device {
        Device { name: name.into(), target_triple: triple.into(), gpu_gib: gpu,
                 rtt_s: rtt, link_mib_s: link, can_execute: exec }
    }

    #[test]
    fn a_GPU_LESS_device_is_useless_for_inference_and_GOOD_for_a_build() {
        // The whole reason a device score cannot be one number: a machine with no accelerator has a
        // different architecture, and that is exactly what a build check wants.
        let pi = dev("rpi5", "aarch64-unknown-linux-gnu", 0.0, 0.02, 10.0, true);
        assert!(fits(&pi, Task::Inference).is_err());
        assert!(fits(&pi, Task::BuildCheck).is_ok());
    }

    #[test]
    fn a_NARROW_LINK_disqualifies_inference_and_not_a_build() {
        // Inference moves activations continuously; a build ships source once and returns a verdict.
        let wifi = dev("laptop", "x86_64-unknown-linux-gnu", 8.0, 0.03, 10.0, true);
        assert_eq!(fits(&wifi, Task::Inference), Err(Unfit::LinkTooNarrow));
        assert!(fits(&wifi, Task::BuildCheck).is_ok());
        assert!(Unfit::LinkTooNarrow.explain(Task::Inference).contains("unaffected for a build"));
    }

    #[test]
    fn a_DRAFT_has_the_tightest_latency_budget_of_all() {
        // Verification waits on the draft, so a slow one is worse than none.
        let far = dev("remote", "x86_64-unknown-linux-gnu", 24.0, 0.2, 1000.0, true);
        assert_eq!(fits(&far, Task::Draft), Err(Unfit::TooSlow));
        assert!(fits(&far, Task::Inference).is_ok(), "the same device is fine for committed tokens");
    }

    #[test]
    fn a_DELEGATE_tolerates_what_inference_cannot() {
        // It runs ahead of the main loop, so its latency hides entirely.
        let slow = dev("nas", "x86_64-unknown-linux-gnu", 4.0, 5.0, 500.0, true);
        assert!(fits(&slow, Task::Inference).is_err());
        assert!(fits(&slow, Task::Delegate).is_ok());
    }

    #[test]
    fn a_BUILD_ONLY_environment_can_compile_what_it_cannot_RUN() {
        // A green build there is not a passing test, and the distinction has to survive into the
        // plan or someone will read one as the other.
        let ci = dev("container", "aarch64-unknown-linux-gnu", 0.0, 0.1, 500.0, false);
        assert!(fits(&ci, Task::BuildCheck).is_ok());
        assert_eq!(fits(&ci, Task::NativeRun), Err(Unfit::CannotExecute));
    }

    #[test]
    fn NATIVE_verification_is_separated_from_cross_compilation() {
        // A build on x86 for aarch64 catches type errors and misses alignment, endianness and which
        // syscalls exist. One device that can RUN is worth several that can only compile.
        let devs = vec![
            dev("pi", "aarch64-unknown-linux-gnu", 0.0, 0.02, 10.0, true),
            dev("ci", "aarch64-unknown-linux-gnu", 0.0, 0.1, 500.0, false),
            dev("mac", "x86_64-unknown-linux-gnu", 24.0, 0.01, 1000.0, true),
        ];
        let (native, cross) = build_coverage(&devs, "aarch64-unknown-linux-gnu");
        assert_eq!(native.len(), 1, "only the pi can actually run it");
        assert_eq!(cross.len(), 1, "the mac compiles for it");
    }

    #[test]
    fn every_refusal_says_what_the_device_is_still_GOOD_for() {
        // A plan that only says no teaches the operator that half their hardware is useless.
        assert!(Unfit::NoAccelerator.explain(Task::Inference).contains("best machine available"));
    }
}
