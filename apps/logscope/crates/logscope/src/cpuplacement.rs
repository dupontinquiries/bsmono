//! cpuplacement.rs — should this model run on the CPU, and is that a real win or an excuse?
//!
//! llama.cpp's CPU path is fast enough on unified memory that a small model — a Liaison, a Disciple —
//! can be genuinely better placed there: no dispatch overhead, no GPU contention with the Thinker, and
//! host RAM rather than the GPU buffer budget.
//!
//! ## The failure mode this is built against
//!
//! "The CPU is faster, so use the CPU" is exactly the reasoning that lets a slow GPU path stay slow
//! forever. If the GPU is running at 5% of what its memory bandwidth allows, that is a BUG, and moving
//! work off it hides the bug and removes the pressure to fix it. This session is a standing example:
//! the GPU decode looked hopeless at 1119 submits/token, and the answer was a frame-lifecycle fix, not
//! a different processor.
//!
//! So the verdict is never taken on relative speed alone. It first asks whether the GPU is near ITS OWN
//! CEILING — bytes-per-token against measured bandwidth. Below that, the recommendation is "fix the GPU
//! path", and CPU placement is offered only for reasons that survive the GPU being fast: contention with
//! another model, and memory that the GPU budget cannot hold but host RAM can.

/// What the measurement says to do.
#[derive(Clone, PartialEq, Debug)]
pub enum Placement {
    /// Run on the GPU.
    Gpu,
    /// Run on the CPU, for a reason that is not "the GPU is broken".
    Cpu(CpuReason),
    /// The CPU is faster, but only because the GPU path is underperforming. Fix that instead.
    GpuUnderperforming { pct_of_ceiling: f32, note: String },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CpuReason {
    /// The CPU is simply faster or equal, with the GPU already near its ceiling.
    Faster,
    /// Comparable speed, and running here frees the GPU for a model that needs it.
    FreesTheGpu,
    /// It does not fit the GPU budget but does fit host RAM.
    DoesNotFitGpu,
}

impl CpuReason {
    pub fn explain(self) -> &'static str {
        match self {
            CpuReason::Faster => "faster on CPU, and the GPU is already near its bandwidth ceiling — \
this is a real win, not a workaround",
            CpuReason::FreesTheGpu => "comparable speed, and running here leaves the GPU entirely to the \
Thinker — the liaison stops contending with the model it reports on",
            CpuReason::DoesNotFitGpu => "does not fit the GPU budget but fits host RAM",
        }
    }
}

/// One measured comparison.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Measurement {
    pub cpu_toks: f32,
    pub gpu_toks: f32,
    /// What the GPU SHOULD reach given the model's bytes-per-token and the device's measured bandwidth.
    /// This is what separates "CPU wins" from "GPU is broken".
    pub gpu_ceiling_toks: f32,
    /// Would running on the GPU contend with another model already there?
    pub gpu_contended: bool,
    /// Does it fit the GPU budget at all?
    pub fits_gpu: bool,
}

/// The tok/s a bandwidth-bound decode should reach, from bytes touched per token.
///
/// This is what makes the anti-crutch guard possible at all: without a ceiling there is nothing to
/// compare the GPU against, and "slower than the CPU" is indistinguishable from "broken". A decode reads
/// its active weights once per token, so tok/s = bandwidth / bytes-per-token — crude, ignores attention
/// and KV, and that is fine: it is an order-of-magnitude test, not a target.
///
/// `active_bytes` is what the forward pass actually touches — for a MoE that is the dense weights plus
/// the ROUTED experts, not the whole model. Using the total would set the bar far too low and let a
/// genuinely broken path look healthy.
pub fn bandwidth_ceiling_toks(active_bytes: u64, gib_per_sec: f32) -> f32 {
    if active_bytes == 0 || gib_per_sec <= 0.0 { return 0.0; }
    let gib = active_bytes as f32 / (1u64 << 30) as f32;
    gib_per_sec / gib
}

/// Below this fraction of its own ceiling, the GPU path is considered defective rather than slow.
///
/// 0.25 is deliberately generous: a real kernel rarely hits its bandwidth bound, and setting the bar
/// high would label healthy paths as broken. What it catches is the order-of-magnitude case — the one
/// where moving to CPU would bury a fixable defect.
pub const UNDERPERFORMING_BELOW: f32 = 0.25;

/// How much slower the CPU may be and still win on contention grounds.
///
/// Not 1.0: freeing the GPU is worth a real, bounded amount of throughput, because the Thinker's time is
/// what the operator is actually waiting on. But not unbounded either — a liaison that takes ten seconds
/// to say one sentence is useless however little GPU it uses.
pub const CONTENTION_TOLERANCE: f32 = 0.60;

pub fn decide(m: &Measurement) -> Placement {
    if !m.fits_gpu {
        return Placement::Cpu(CpuReason::DoesNotFitGpu);
    }
    let pct = if m.gpu_ceiling_toks > 0.0 { m.gpu_toks / m.gpu_ceiling_toks } else { 1.0 };
    if pct < UNDERPERFORMING_BELOW && m.cpu_toks > m.gpu_toks {
        return Placement::GpuUnderperforming {
            pct_of_ceiling: pct,
            note: format!("the GPU is at {:.0}% of its bandwidth ceiling ({:.1} of {:.1} tok/s). The CPU \
being faster here says the GPU path is defective, not that the CPU is the right home. Moving the model \
would hide a fixable bug.", pct * 100.0, m.gpu_toks, m.gpu_ceiling_toks),
        };
    }
    if m.cpu_toks >= m.gpu_toks { return Placement::Cpu(CpuReason::Faster); }
    if m.gpu_contended && m.cpu_toks >= m.gpu_toks * CONTENTION_TOLERANCE {
        return Placement::Cpu(CpuReason::FreesTheGpu);
    }
    Placement::Gpu
}

impl Placement {
    /// One line for the startup log.
    pub fn explain(&self) -> String {
        match self {
            Placement::Gpu => "GPU".into(),
            Placement::Cpu(r) => format!("CPU — {}", r.explain()),
            Placement::GpuUnderperforming { note, .. } => format!("GPU (kept) — {note}"),
        }
    }

    /// Does this verdict point at work to do rather than a placement to adopt?
    pub fn is_a_bug_report(&self) -> bool { matches!(self, Placement::GpuUnderperforming { .. }) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(cpu: f32, gpu: f32, ceiling: f32) -> Measurement {
        Measurement { cpu_toks: cpu, gpu_toks: gpu, gpu_ceiling_toks: ceiling,
                      gpu_contended: false, fits_gpu: true }
    }

    #[test]
    fn a_faster_CPU_against_a_BROKEN_gpu_is_reported_as_a_bug_not_a_placement() {
        // The whole point. "The CPU is faster, so use the CPU" is how a slow GPU path stays slow
        // forever. This session is the standing example: GPU decode looked hopeless at 1119
        // submits/token and the answer was a frame-lifecycle fix, not a different processor.
        let v = decide(&m(20.0, 2.0, 70.0));   // GPU at 3% of ceiling
        assert!(v.is_a_bug_report(), "{v:?}");
        let s = v.explain();
        assert!(s.contains("defective, not that the CPU is the right home"));
        assert!(s.contains("hide a fixable bug"));
    }

    #[test]
    fn a_faster_CPU_against_a_HEALTHY_gpu_is_a_real_win() {
        // Same relative speeds, but the GPU is near its bound — so there is nothing to fix and the CPU
        // genuinely is the better home.
        let v = decide(&m(20.0, 18.0, 20.0));
        assert_eq!(v, Placement::Cpu(CpuReason::Faster));
        assert!(!v.is_a_bug_report());
        assert!(v.explain().contains("not a workaround"));
    }

    #[test]
    fn contention_buys_a_BOUNDED_amount_of_slowdown() {
        // Freeing the GPU is worth real throughput, because the Thinker's time is what the operator
        // waits on. But not unbounded: a liaison taking ten seconds per sentence is useless however
        // little GPU it uses.
        let mut x = m(7.0, 10.0, 11.0);        // CPU at 70% of GPU
        assert_eq!(decide(&x), Placement::Gpu, "not contended: keep the faster processor");
        x.gpu_contended = true;
        assert_eq!(decide(&x), Placement::Cpu(CpuReason::FreesTheGpu));

        let mut y = m(3.0, 10.0, 11.0);        // CPU at 30% — too slow to justify
        y.gpu_contended = true;
        assert_eq!(decide(&y), Placement::Gpu, "contention does not excuse any slowdown");
    }

    #[test]
    fn not_fitting_the_gpu_settles_it_before_any_speed_comparison() {
        // A model that does not fit has no GPU number to compare against, and the Asahi budget is a hard
        // driver ceiling well below system RAM — exactly the case where host RAM is the only home.
        let mut x = m(1.0, 100.0, 100.0);      // absurd GPU speed, irrelevant
        x.fits_gpu = false;
        assert_eq!(decide(&x), Placement::Cpu(CpuReason::DoesNotFitGpu));
    }

    #[test]
    fn a_slower_CPU_never_wins_on_speed_alone() {
        assert_eq!(decide(&m(5.0, 10.0, 11.0)), Placement::Gpu);
        // and a broken GPU that is STILL faster than the CPU is not a bug report — there is nothing to
        // move to, so the finding would be noise
        let v = decide(&m(1.0, 2.0, 70.0));
        assert_eq!(v, Placement::Gpu, "GPU is bad but the CPU is worse: nothing to recommend");
    }

    #[test]
    fn the_ceiling_uses_ACTIVE_bytes_not_the_whole_model() {
        // Scout: ~4.2 GiB dense plus one routed expert per layer, about 10.5 GiB per token — not the
        // 60 GiB the model weighs. Using the total sets the bar so low that a broken path looks healthy,
        // which would disable the guard exactly when it is needed.
        let active = (10.5 * (1u64 << 30) as f32) as u64;
        let whole  = (60.0 * (1u64 << 30) as f32) as u64;
        let c_active = bandwidth_ceiling_toks(active, 200.0);
        let c_whole  = bandwidth_ceiling_toks(whole, 200.0);
        assert!((c_active - 19.0).abs() < 1.0, "got {c_active}");
        assert!(c_whole < c_active / 5.0, "the total would understate it by 5x: {c_whole}");

        // and with the real ceiling, a 1 tok/s GPU IS a bug report
        let v = decide(&Measurement { cpu_toks: 5.0, gpu_toks: 1.0, gpu_ceiling_toks: c_active,
                                      gpu_contended: false, fits_gpu: true });
        assert!(v.is_a_bug_report());
    }

    #[test]
    fn a_ceiling_from_missing_inputs_is_zero_not_infinity() {
        assert_eq!(bandwidth_ceiling_toks(0, 200.0), 0.0);
        assert_eq!(bandwidth_ceiling_toks(1 << 30, 0.0), 0.0);
    }

    #[test]
    fn an_unknown_ceiling_does_not_manufacture_a_bug_report() {
        // ceiling 0 means "not measured". Treating that as 0% of ceiling would report every model as a
        // broken GPU path, which would make the whole verdict useless.
        let v = decide(&m(20.0, 2.0, 0.0));
        assert!(!v.is_a_bug_report());
        assert_eq!(v, Placement::Cpu(CpuReason::Faster));
    }
}
