//! power_profile — a user-facing choice between "save battery" and "answer
//! fast" for the mic/transcribe path, and what that choice actually changes.
//!
//! ## The one finding that shapes this whole module
//!
//! GPU dispatch is not a per-profile knob. `load_tier` calls
//! `WhisperContextParameters::default()`, and whisper.cpp's own default sets
//! `use_gpu: true` (a real, stable field on `whisper_context_params`,
//! confirmed against `whisper-rs-sys`'s generated bindings rather than
//! assumed) — so if this crate is built with `voice-vulkan`/`voice-cuda`/
//! `voice-metal`, GPU is used UNCONDITIONALLY today, before this module
//! existed. There is no runtime "prefer GPU" choice to make, because
//! whisper.cpp already always prefers it when compiled in.
//!
//! Whether that default is *right* is a genuinely open question this
//! session cannot settle from source alone — burst-then-idle at a GPU's
//! efficient low clock, against a sustained CPU decode at whatever
//! frequency the governor picks, is a real power comparison that needs a
//! wall meter on real hardware, not reasoning. What IS true regardless: a
//! GPU backend, once compiled in, is either present and always used, or
//! absent — there is no live toggle for an [`EnergyMode`] to flip. So this
//! module's actual job is narrower than "GPU vs CPU": it is **"when there
//! is no GPU backend (or the feature was not compiled in), how should the
//! CPU-only path behave."**
//!
//! ## What this changes on the CPU-only path
//!
//! [`EnergyMode::Efficiency`] biases the transcribe/VAD thread onto
//! efficiency cores via [`crate::cpu_efficiency`]'s uclamp mechanism.
//! [`EnergyMode::Performance`] runs unclamped.
//!
//! This is a genuine change to that module's own stated scope, not a
//! reinterpretation of it. Its doc says, in capitals, "NEVER from the
//! wake-word / live-ASR path the user is waiting on — clamping that would
//! hurt responsiveness." That rule protects against an ACCIDENTAL slowdown
//! nobody asked for. A user who explicitly selects "save battery" in
//! Settings has asked for exactly that trade-off on exactly that path —
//! consent is what turns "accidental regression" into "the feature working
//! as designed." The safety rule still applies to every OTHER caller of
//! `cpu_efficiency` that has not been given an explicit opt-in the way this
//! one now is.
//!
//! ## What this does NOT change
//!
//! Model tier selection (`ensure_models_loaded`'s tiny/base/high-accuracy
//! policy) and the dual-tier race in `transcribe_with_dual_model` are
//! untouched. A slower CPU governor clock is a different kind of "slower"
//! than picking a smaller model — the former is a latency cost paid once
//! per utterance for a power saving; the latter is an accuracy trade a user
//! did not ask for. Conflating them would mean "save battery" silently also
//! makes transcription worse, which is not what the mode name promises.

/// The user's stated preference for the mic/transcribe path.
///
/// Two states, not three: a "balanced" default would need its own
/// threshold decision (balanced compared to WHAT?) with no data yet to set
/// one honestly. Absence of a mode (`None` at the call site, see
/// [`plan_for`]) already means "no opinion, run unclamped" — that IS the
/// default, so a third named variant would just be a second name for the
/// same behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnergyMode {
    /// Prefer battery life. Accepts additional latency on the CPU-only
    /// path in exchange for a scheduler bias toward efficiency cores.
    Efficiency,
    /// Prefer speed. Runs the CPU-only path unclamped — today's behavior,
    /// unchanged.
    Performance,
}

/// What [`EnergyMode`] actually decides, for the CPU-only transcribe path.
///
/// A plan rather than an immediate action, so the decision is testable
/// without a kernel to apply it against — the same split
/// `handheld_detect::Detection` and `Profile::apply` use in this monorepo's
/// games, and `daemons::KnownDaemon`/`install_daemons_script` use in
/// gitmind: decide first, as data; apply second, against real syscalls,
/// separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuPlan {
    /// `Some(util_max)` to bias the calling thread onto efficiency cores
    /// via `cpu_efficiency::clamp_current_thread_to_util_max(util_max)`;
    /// `None` to run unclamped. Only ever `Some` when [`has_gpu_backend`]
    /// is `false` — see [`plan_for`]'s doc for why a GPU backend, once
    /// compiled in, is used regardless of this plan.
    pub clamp_util_max: Option<u32>,
}

/// The util_max this module picks for [`EnergyMode::Efficiency`] on the
/// CPU-only path, when no GPU backend is in use.
///
/// Deliberately NOT `cpu_efficiency::ECORE_UTIL_MAX` (256) reused
/// unexamined. That constant is calibrated for work its own module scopes
/// to "invisible to the user" — a background corpus write nobody is
/// staring at a spinner for. A user waiting on a live transcription who
/// has explicitly asked to trade latency for battery is a different
/// situation: some slowdown is the whole point, but a background-tuned
/// 25% cap chosen for a task with no user attached might be either too
/// gentle (barely saves anything) or too aggressive (the wait becomes
/// annoying enough that the user turns the setting back off, which is a
/// worse outcome than not building it) for a task someone IS watching.
///
/// 384 (37.5%, roughly midway between the background 256 and the
/// unclamped 1024) is a REASONED STARTING POINT, not a measured one: high
/// enough that a short utterance's transcription does not visibly crawl,
/// low enough to still bias placement away from a full-clock P-core. It
/// has not been measured against a stopwatch on a real heterogeneous
/// device, which the constant it deliberately does NOT reuse warns is
/// exactly what such a number needs. Treat this as the first thing to
/// tune once this mode ships somewhere testable, not as a settled value.
pub const FOREGROUND_ECO_UTIL_MAX: u32 = 384;

/// Decide the CPU-path plan for a given mode and hardware.
///
/// `has_gpu_backend` is whether a GPU-accelerated whisper context is
/// actually in use for this transcription — which, per this module's own
/// doc, is a compile-time-then-always-on fact once true, not something
/// this function or its caller chooses. It is a PARAMETER here (rather
/// than this function calling out to check some global) so the decision
/// stays pure and testable; the caller supplies the fact.
///
/// When a GPU backend is in use, the plan is always unclamped regardless
/// of mode: clamping a CPU thread's uclamp value has no bearing on GPU
/// dispatch, and clamping some OTHER, now-idle CPU thread while the GPU
/// does the real work would save nothing while adding the same
/// consent-scoped risk `cpu_efficiency`'s own doc warns about, for no
/// benefit. The mode only has an effect in the CPU-only case.
pub fn plan_for(mode: EnergyMode, has_gpu_backend: bool) -> CpuPlan {
    if has_gpu_backend {
        return CpuPlan { clamp_util_max: None };
    }
    CpuPlan {
        clamp_util_max: matches!(mode, EnergyMode::Efficiency)
            .then_some(FOREGROUND_ECO_UTIL_MAX),
    }
}

/// RAII guard that applies a [`CpuPlan`] to the calling thread and lifts
/// it on drop.
///
/// Mirrors `cpu_efficiency::EcoGuard`'s shape (clamp on construction,
/// unclamp on drop, never panics) rather than a third pattern, but takes a
/// full `CpuPlan` instead of a single `defer: bool` — `EcoGuard` only ever
/// needed one fixed background util_max; this one needs to carry whichever
/// value `plan_for` chose (or none, for [`EnergyMode::Performance`] and
/// the GPU-backend case).
///
/// Call as the first statement on the thread that will run the CPU-only
/// transcribe/VAD work — the same placement rule `EcoGuard`'s own doc
/// states, for the same reason (a clamp set mid-work still applies for the
/// remainder, but starting clamped is simpler to reason about than a
/// clamp that changes partway through one utterance).
pub struct CpuPlanGuard {
    clamped: bool,
}

impl CpuPlanGuard {
    pub fn apply(plan: CpuPlan) -> Self {
        let clamped = match plan.clamp_util_max {
            Some(util_max) => crate::cpu_efficiency::clamp_current_thread_to_util_max(util_max),
            None => false,
        };
        CpuPlanGuard { clamped }
    }

    /// True if a clamp was both requested AND accepted by the kernel (for
    /// tests/logging) — distinct from "was one requested", since a
    /// non-Linux target or an old kernel silently declines and this
    /// reports that honestly rather than claiming success.
    pub fn is_clamped(&self) -> bool { self.clamped }
}

impl Drop for CpuPlanGuard {
    fn drop(&mut self) {
        if self.clamped {
            crate::cpu_efficiency::unclamp_current_thread();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_with_no_clamp_never_reports_clamped() {
        let g = CpuPlanGuard::apply(CpuPlan { clamp_util_max: None });
        assert!(!g.is_clamped());
        drop(g); // must not panic, must not call unclamp needlessly
    }

    #[test]
    fn guard_with_a_clamp_attempts_it_and_drops_safely() {
        // On a sandbox without uclamp support is_clamped() may end up
        // false (graceful decline) -- the property this test actually
        // pins is that requesting a clamp and dropping the guard never
        // panics, on any platform.
        let g = CpuPlanGuard::apply(CpuPlan { clamp_util_max: Some(FOREGROUND_ECO_UTIL_MAX) });
        let _ = g.is_clamped();
        drop(g);
    }

    #[test]
    fn efficiency_clamps_on_cpu_only() {
        assert_eq!(
            plan_for(EnergyMode::Efficiency, false),
            CpuPlan { clamp_util_max: Some(FOREGROUND_ECO_UTIL_MAX) }
        );
    }

    #[test]
    fn performance_never_clamps_on_cpu_only() {
        assert_eq!(
            plan_for(EnergyMode::Performance, false),
            CpuPlan { clamp_util_max: None }
        );
    }

    #[test]
    fn gpu_backend_present_overrides_efficiency_mode() {
        // The core finding this module exists to encode: a GPU backend,
        // once in use, is not something this plan turns on or off, and
        // clamping some other CPU thread while it works helps nothing.
        assert_eq!(
            plan_for(EnergyMode::Efficiency, true),
            CpuPlan { clamp_util_max: None }
        );
    }

    #[test]
    fn gpu_backend_present_overrides_performance_mode_too() {
        // Same result from the other mode, for the same reason -- proves
        // the override is about hardware, not a mode-specific special case.
        assert_eq!(
            plan_for(EnergyMode::Performance, true),
            CpuPlan { clamp_util_max: None }
        );
    }

    #[test]
    fn the_foreground_cap_is_looser_than_the_background_one() {
        // The reasoning this module's doc gives for NOT reusing
        // cpu_efficiency::ECORE_UTIL_MAX: a task the user is watching
        // should get a gentler clamp than one nobody is waiting on. Pinned
        // as a relationship between the two constants rather than a
        // specific number, so either can be retuned without this test
        // silently going stale about WHY the gap exists.
        assert!(
            FOREGROUND_ECO_UTIL_MAX > crate::cpu_efficiency::ECORE_UTIL_MAX,
            "a foreground wait should not be clamped as hard as an invisible background write"
        );
        assert!(FOREGROUND_ECO_UTIL_MAX < 1024, "must still be a real clamp, not a no-op");
    }

    #[test]
    fn the_four_cases_are_exhaustive_and_distinct() {
        // Pins the full 2x2 table in one place, so a future edit that
        // collapses two cases together (e.g. "just always clamp when GPU
        // is present too, what's the harm") fails a test that explains
        // why, rather than silently changing behavior.
        let cases = [
            (EnergyMode::Efficiency, false, Some(FOREGROUND_ECO_UTIL_MAX)),
            (EnergyMode::Efficiency, true, None),
            (EnergyMode::Performance, false, None),
            (EnergyMode::Performance, true, None),
        ];
        for (mode, has_gpu, want_clamp) in cases {
            assert_eq!(
                plan_for(mode, has_gpu).clamp_util_max,
                want_clamp,
                "mode={mode:?} has_gpu_backend={has_gpu}"
            );
        }
    }
}
