//! gateclass.rs — what a gate result is evidence FOR, given where it ran.
//!
//! ## The failure this prevents
//!
//! `--gate-q6k` went green on lavapipe and I reported the kernel bit-exact. On an M2 Max it fails: 64
//! of 256 writes land. Four faults had been found and fixed against lavapipe, all four real, and the
//! kernel was still wrong on the only hardware that matters.
//!
//! An hour earlier `--gate-q4k` was reporting "the loader can stop requantizing" while testing only
//! dequantization. Two instances in two days of **a green result read as broader than its scope**.
//!
//! A document saying "check the adapter" does not fix this, because the failure is not ignorance — the
//! adapter name is already printed in every gate's first line. The failure is that "PASS" and "PASS on
//! a reference rasterizer" print identically, so the distinction has to survive being skim-read.
//!
//! ## Why a software rasterizer cannot settle a kernel question
//!
//! Lavapipe is a *reference* implementation: more permissive and more nearly correct than a driver. It
//! validates arithmetic and conceals codegen, scheduling, memory visibility and workgroup behaviour.
//! The faults it catches are the ones a careful read would also catch; the ones it misses are the ones
//! a read cannot.

/// What kind of device produced a result.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Device {
    /// A CPU implementation of the API. Arithmetic is meaningful; nothing else is.
    Software,
    /// Real hardware.
    Hardware,
}

/// Classify from the adapter string.
///
/// By name because there is no capability bit for "this is a reference implementation" — and a
/// software rasterizer advertises full conformance, which is exactly why it is trusted too much.
pub fn device_of(adapter: &str) -> Device {
    let a = adapter.to_ascii_lowercase();
    const SOFT: &[&str] = &["llvmpipe", "lavapipe", "swiftshader", "softwarerasterizer",
                            "microsoft basic render", "warp"];
    if SOFT.iter().any(|s| a.contains(s)) { Device::Software } else { Device::Hardware }
}

/// What a passing run establishes.
#[derive(Clone, PartialEq, Debug)]
pub enum Claim {
    /// Verified where it counts.
    Verified,
    /// The arithmetic holds. Codegen, scheduling and memory behaviour are untested.
    ArithmeticOnly,
}

/// The line a gate should print when its checks pass.
///
/// A software pass NEVER prints the word a reader skims for. "PASS" and "PASS, arithmetic only" look
/// alike at a glance, so the software case leads with what is missing rather than appending it.
pub fn verdict_line(gate: &str, adapter: &str, detail: &str) -> String {
    match device_of(adapter) {
        Device::Hardware => format!("{gate}: {detail} \u{2713}"),
        Device::Software => format!(
            "{gate}: ARITHMETIC ONLY on {adapter} — {detail}, but a reference rasterizer hides \
codegen, scheduling and memory-visibility faults. This does NOT establish the kernel works; run it on \
the target before relying on it"),
    }
}

/// Does a pass here close the question?
pub fn settles(adapter: &str) -> bool { device_of(adapter) == Device::Hardware }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lavapipe_is_recognised_as_software() {
        // The exact string from the container runs that produced four "verified" claims.
        assert_eq!(device_of("llvmpipe (LLVM 20.1.2, 256 bits) [Vulkan]"), Device::Software);
        assert_eq!(device_of("lavapipe"), Device::Software);
        assert_eq!(device_of("SwiftShader Device"), Device::Software);
    }

    #[test]
    fn real_adapters_are_hardware() {
        // A false Software classification would mark every real run as inconclusive and the warning
        // would be ignored within a day — which is how a good check becomes noise.
        for a in ["Apple M2 Max (G14C B1) [Vulkan]", "Apple M2 Ultra (G14D B1) [Vulkan]",
                  "NVIDIA GeForce RTX 4090", "AMD Radeon RX 7900 XTX", "Adreno (TM) 740"] {
            assert_eq!(device_of(a), Device::Hardware, "{a}");
        }
    }

    #[test]
    fn a_software_pass_does_NOT_print_the_word_a_reader_skims_for() {
        // The failure is not ignorance — the adapter is already in every gate's first line. It is that
        // "PASS" and "PASS on a reference rasterizer" print alike, so the distinction has to survive
        // being skim-read.
        let soft = verdict_line("gate-q6k", "llvmpipe (LLVM 20.1.2)", "bit-exact against the CPU");
        assert!(!soft.contains('\u{2713}'), "no tick on software: {soft}");
        assert!(soft.starts_with("gate-q6k: ARITHMETIC ONLY"), "leads with what is missing: {soft}");
        assert!(soft.contains("does NOT establish"));
    }

    #[test]
    fn a_hardware_pass_reads_normally() {
        let hw = verdict_line("gate-q6k", "Apple M2 Max (G14C B1) [Vulkan]", "bit-exact");
        assert!(hw.contains('\u{2713}'));
        assert!(!hw.contains("ARITHMETIC ONLY"));
    }

    #[test]
    fn only_hardware_settles_a_question() {
        assert!(settles("Apple M2 Max (G14C B1) [Vulkan]"));
        assert!(!settles("llvmpipe (LLVM 20.1.2, 256 bits) [Vulkan]"));
    }

    #[test]
    fn an_unknown_adapter_is_treated_as_HARDWARE() {
        // Erring the other way would mark a new real device inconclusive forever. The list names the
        // software implementations, which are few and stable, rather than trying to enumerate hardware.
        assert_eq!(device_of("Some Vendor GPU Nobody Has Heard Of"), Device::Hardware);
    }
}
