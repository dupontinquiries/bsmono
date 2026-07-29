//! hybridsplit.rs — how many layers belong on the GPU, and what the rest cost on the CPU.
//!
//! llama.cpp does not attempt full residency. Its Vulkan run reports `backend_ptrs.size() = 2` and
//! `graph splits = 2`, with a device compute buffer AND a host one — the scheduler assigns each node to
//! a backend and inserts copies at the boundary. Our engine has only ever had two settings: everything
//! resident, or stream everything through a staging ring. The middle is where llama.cpp lives.
//!
//! ## The split point is a minimum, not a maximum
//!
//! The tempting rule is "fit as many layers on the GPU as possible". That is wrong when the GPU is
//! slower per byte than the CPU for the work in question, and it is wrong when the last few layers only
//! fit by evicting the KV cache. What matters is total time, which has three terms: GPU layers, CPU
//! layers, and the boundary crossing between them.
//!
//! ## The boundary is cheap here and would not be elsewhere
//!
//! On unified memory the activation handed between backends is a vector of `d` floats — 20 KiB for a
//! 5120-wide model, once per boundary per token. On a discrete GPU that is a PCIe round trip and the
//! calculus changes completely. This models it as a bandwidth cost so the assumption is visible rather
//! than baked in.

/// Everything needed to choose a split.
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub n_layers: usize,
    /// Bytes of weight touched per layer per token — for a MoE, dense plus the routed experts.
    pub bytes_per_layer: u64,
    pub gpu_budget_bytes: u64,
    pub gpu_gib_s: f32,
    pub cpu_gib_s: f32,
    /// Activation size at a backend boundary, in bytes. `d * 4` on a unified machine.
    pub boundary_bytes: u64,
    /// Effective bandwidth for a boundary crossing. On UMA this is a memcpy; on PCIe it is not.
    pub boundary_gib_s: f32,
}

const GIB: f64 = (1u64 << 30) as f64;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Split {
    pub gpu_layers: usize,
    pub cpu_layers: usize,
    pub ms_per_token: f32,
    /// True when every layer fits on the GPU and no boundary is crossed.
    pub all_gpu: bool,
}

/// Milliseconds per token for a given split.
pub fn cost(p: &Plan, gpu_layers: usize) -> f32 {
    let g = gpu_layers.min(p.n_layers);
    let c = p.n_layers - g;
    let sec = |bytes: u64, gib_s: f32| if gib_s <= 0.0 { f32::INFINITY }
        else { (bytes as f64 / GIB) as f32 / gib_s };
    let t_gpu = sec(p.bytes_per_layer * g as u64, p.gpu_gib_s);
    let t_cpu = sec(p.bytes_per_layer * c as u64, p.cpu_gib_s);
    // Two crossings when the work is split at all — down to the CPU and back for the next GPU section.
    // Zero when one side does everything, which is why an all-GPU plan can beat a marginally faster
    // split.
    let crossings = if g > 0 && c > 0 { 2 } else { 0 };
    let t_edge = sec(p.boundary_bytes * crossings, p.boundary_gib_s);
    (t_gpu + t_cpu + t_edge) * 1000.0
}

/// Choose the split that minimises time, subject to the GPU budget.
///
/// Searches rather than computing a closed form: the space is `n_layers + 1` wide, the cost function is
/// cheap, and a closed form would hide the boundary term — which is the part that makes the answer
/// non-obvious and would be the first thing dropped in an algebraic simplification.
pub fn choose(p: &Plan) -> Split {
    let max_gpu = if p.bytes_per_layer == 0 { p.n_layers }
        else { (p.gpu_budget_bytes / p.bytes_per_layer) as usize };
    let max_gpu = max_gpu.min(p.n_layers);
    let mut best = (usize::MAX, f32::INFINITY);
    for g in 0..=max_gpu {
        let c = cost(p, g);
        if c < best.1 { best = (g, c); }
    }
    let g = if best.0 == usize::MAX { 0 } else { best.0 };
    Split { gpu_layers: g, cpu_layers: p.n_layers - g, ms_per_token: cost(p, g),
            all_gpu: g == p.n_layers }
}

/// One line explaining a split, for the load log.
pub fn explain(p: &Plan, s: &Split) -> String {
    if s.all_gpu {
        return format!("all {} layers resident on the GPU — {:.0} ms/token", p.n_layers, s.ms_per_token);
    }
    if s.gpu_layers == 0 {
        return format!("all {} layers on the CPU — the GPU budget holds none of them, {:.0} ms/token",
                       p.n_layers, s.ms_per_token);
    }
    format!("{} of {} layers on the GPU, {} on the CPU — {:.0} ms/token (all-GPU would need {:.1} GiB, \
budget is {:.1} GiB)", s.gpu_layers, p.n_layers, s.cpu_layers, s.ms_per_token,
        (p.bytes_per_layer * p.n_layers as u64) as f64 / GIB,
        p.gpu_budget_bytes as f64 / GIB)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scout-ish: 48 layers, ~220 MiB touched per layer per token, measured bandwidths from the M2.
    fn scout(budget_gib: f64) -> Plan {
        Plan { n_layers: 48, bytes_per_layer: 220 << 20,
               gpu_budget_bytes: (budget_gib * GIB) as u64,
               gpu_gib_s: 258.1, cpu_gib_s: 113.2,
               boundary_bytes: 5120 * 4, boundary_gib_s: 50.0 }
    }

    #[test]
    fn a_budget_that_holds_everything_puts_everything_on_the_gpu() {
        let s = choose(&scout(64.0));
        assert!(s.all_gpu);
        assert_eq!(s.cpu_layers, 0);
        // and no boundary is crossed
        assert!((s.ms_per_token - cost(&scout(64.0), 48)).abs() < 1e-3);
    }

    #[test]
    fn a_tight_budget_splits_rather_than_falling_back_to_all_cpu() {
        // The case the engine currently cannot express: it is fully resident or it streams everything.
        // Ten layers on the GPU and thirty-eight on the CPU beats both.
        let p = scout(2.5);                       // ~11 layers' worth
        let s = choose(&p);
        assert!(s.gpu_layers > 0 && s.cpu_layers > 0, "expected a split, got {s:?}");
        assert!(s.ms_per_token < cost(&p, 0), "the split must beat all-CPU");
    }

    #[test]
    fn the_split_never_exceeds_the_budget() {
        // Choosing a layer that does not fit is how a plan turns into a mid-load OOM.
        for gib in [0.5f64, 2.0, 7.0, 13.0] {
            let p = scout(gib);
            let s = choose(&p);
            let used = s.gpu_layers as u64 * p.bytes_per_layer;
            assert!(used <= p.gpu_budget_bytes, "{gib} GiB: used {used}");
        }
    }

    #[test]
    fn a_faster_CPU_than_GPU_puts_everything_on_the_cpu() {
        // "Fit as many layers on the GPU as possible" is the tempting rule and it is wrong whenever the
        // GPU is slower for the work in question — which is exactly the situation the wgpu path has been
        // in all session, at 0.45 tok/s delivered.
        let mut p = scout(64.0);
        p.gpu_gib_s = 10.0;                       // a degraded GPU path
        let s = choose(&p);
        assert_eq!(s.gpu_layers, 0, "a slow GPU should be left out entirely, got {s:?}");
        assert!(!s.all_gpu);
    }

    #[test]
    fn the_boundary_cost_can_make_all_gpu_win_over_a_marginal_split() {
        // Two crossings per token are free on UMA and are not on PCIe. Modelled explicitly so the
        // assumption is visible: with a slow boundary, a split that is marginally better on compute
        // loses.
        let mut p = scout(64.0);
        p.boundary_gib_s = 0.01;                  // a very slow link
        let s = choose(&p);
        assert!(s.all_gpu, "with an expensive boundary, keep it on one side: {s:?}");
    }

    #[test]
    fn a_zero_budget_is_all_cpu_and_says_so() {
        let p = scout(0.0);
        let s = choose(&p);
        assert_eq!(s.gpu_layers, 0);
        assert!(explain(&p, &s).contains("the GPU budget holds none of them"));
    }

    #[test]
    fn the_explanation_gives_the_numbers_needed_to_disagree_with_it() {
        // A load line saying "split 10/38" invites the question "why not more?", and the answer is the
        // budget against the total — so both are in the line rather than a follow-up question.
        let p = scout(2.5);
        let e = explain(&p, &choose(&p));
        assert!(e.contains("on the GPU") && e.contains("on the CPU"));
        assert!(e.contains("all-GPU would need") && e.contains("budget is"));
    }
}
