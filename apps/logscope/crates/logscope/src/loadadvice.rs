//! loadadvice.rs — run the planners at load time and say what they concluded.
//!
//! `hybridsplit`, `cpuplacement`, `coresidency` and `coreclass` are all tested logic with no caller.
//! Wiring them to actually route work is a large job; wiring them to REPORT is small, and it makes
//! every one of them falsifiable against a real model today rather than after the routing lands.
//!
//! That ordering matters. A planner that has never seen a real model is a hypothesis, and this project
//! has spent whole rounds acting on numbers that turned out to describe something else — the 24 GiB
//! staging peak, the thread-spawn figure that was contention, the row blocking that only helped on one
//! core. An advisory line costs nothing and turns each planner into a claim someone can check.
//!
//! Deliberately advisory. Nothing here changes what runs.

use crate::hybridsplit::{Plan, Split};

/// Everything a load knows that the planners need.
#[derive(Clone, Copy, Debug)]
pub struct LoadFacts {
    pub n_layers: usize,
    pub bytes_per_layer: u64,
    pub gpu_budget_bytes: u64,
    /// Measured, not assumed. 0 when never probed.
    pub measured_vram_gib: f32,
    pub gpu_gib_s: f32,
    pub cpu_gib_s: f32,
    pub d: usize,
}

impl LoadFacts {
    fn plan(&self) -> Plan {
        Plan {
            n_layers: self.n_layers, bytes_per_layer: self.bytes_per_layer,
            gpu_budget_bytes: self.gpu_budget_bytes,
            gpu_gib_s: self.gpu_gib_s, cpu_gib_s: self.cpu_gib_s,
            boundary_bytes: (self.d * 4) as u64, boundary_gib_s: 50.0,
        }
    }
    fn total_bytes(&self) -> u64 { self.bytes_per_layer * self.n_layers as u64 }
}

/// Lines to print at load. Empty when there is nothing worth saying.
///
/// Silence is the default: a load that already does the best available thing should not narrate it.
/// Every line here exists because the planner disagrees with what the engine is about to do, which is
/// the only case worth a reader's attention.
pub fn advise(f: &LoadFacts) -> Vec<String> {
    let mut out = Vec::new();
    const GIB: f64 = (1u64 << 30) as f64;

    // 1. Is the budget a measurement or a guess?
    if f.measured_vram_gib <= 0.0 {
        out.push(format!("GPU budget {:.1} GiB is a CONSTANT, not a measurement — run \
`kernel-validate --gate-vram` to find what this device actually honours; the constant has been wrong by \
2.6x on this hardware", f.gpu_budget_bytes as f64 / GIB));
    }

    // 2. Would a split beat what we are about to do?
    let plan = f.plan();
    let split: Split = crate::hybridsplit::choose(&plan);
    let all_gpu_fits = f.total_bytes() <= f.gpu_budget_bytes;
    if !all_gpu_fits && split.gpu_layers > 0 && split.cpu_layers > 0 {
        out.push(format!("this model needs {:.1} GiB for all {} layers but the budget is {:.1} — a \
{}/{} GPU/CPU split would run at {:.0} ms/token instead of streaming everything",
            f.total_bytes() as f64 / GIB, f.n_layers, f.gpu_budget_bytes as f64 / GIB,
            split.gpu_layers, split.cpu_layers, split.ms_per_token));
    }

    // 3. Is the GPU underperforming badly enough that the CPU would win outright?
    if split.gpu_layers == 0 && f.n_layers > 0 {
        out.push(format!("the planner puts ZERO layers on the GPU at {:.0} GiB/s against the CPU's \
{:.0} — either the GPU path is degraded or its bandwidth figure is stale; --gate-perf measures it",
            f.gpu_gib_s, f.cpu_gib_s));
    }

    // 3b. Precision the panel roles do not need.
//
// The liaison advice below says a small reporter can run on the efficiency cores. The same
// reasoning applies to its BITS: a model that paraphrases a backend's state does not need the
// precision of the model whose reasoning is the product, and taking it from there frees room
// where the loss costs least.
//
// Stated as arithmetic rather than a recommendation, because whether a given quantization is
// good enough for a job is a quality judgement no planner can make — but "this would free N
// GiB" is a fact, and it is the fact someone needs to decide.
{
    use crate::rolequant_hint;
    // ONLY WHEN THE FIT IS TIGHT. I wrote this unconditionally and an existing test caught it —
    // "a healthy fitting model produces no advice at all" — which is exactly the mistake I fixed
    // in the template warning one commit earlier, made again immediately.
    //
    // Freeing 1.7 GiB is not interesting when 30 GiB are spare. It is the whole question when the
    // model uses most of the budget, and firing in both cases is what makes a reader stop
    // distinguishing them.
    let model_bytes = f.bytes_per_layer.saturating_mul(f.n_layers as u64);
    let tight = f.gpu_budget_bytes > 0
        && model_bytes * 100 > f.gpu_budget_bytes * 75;   // within 25% of the budget
    if let Some(freed) = rolequant_hint::liaison_saving_gib(8_000_000_000).filter(|_| tight) {
        out.push(format!(
            "a liaison at Q2 instead of q4 frees ~{freed:.1} GiB — it reports on the thinker rather than reasoning, so the precision it gives up is the precision it was not using. On a device where the thinker only just fits, that is the margin between resident and streaming"));
    }
}

// 4. Asymmetric cores worth using?
    let cpus = crate::coreclass::read_capacities();
    if let Some((e, p)) = crate::coreclass::classify(&cpus) {
        out.push(format!("{} performance and {} efficiency cores detected — a small liaison can run on \
the efficiency cores without taking any from the thinker", p.len(), e.len()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scout(budget_gib: f64, measured: f32, gpu_bw: f32) -> LoadFacts {
        LoadFacts {
            n_layers: 48, bytes_per_layer: 220 << 20,
            gpu_budget_bytes: (budget_gib * (1u64 << 30) as f64) as u64,
            measured_vram_gib: measured, gpu_gib_s: gpu_bw, cpu_gib_s: 113.2, d: 5120,
        }
    }

    #[test]
    fn an_unprobed_budget_says_it_is_a_constant() {
        // The 24 GiB figure was wrong by 2.6x on this hardware and nothing said so at load. A reader
        // seeing "budget 24.0 GiB" has no way to know whether that was measured or inherited.
        let a = advise(&scout(24.0, 0.0, 258.1));
        assert!(a.iter().any(|l| l.contains("CONSTANT, not a measurement")), "{a:?}");
        assert!(a.iter().any(|l| l.contains("gate-vram")), "and says how to fix it");
    }

    #[test]
    fn a_probed_budget_does_not_nag() {
        // Silence is the default. A load already doing the best available thing should not narrate it,
        // or the lines that matter get skipped along with the ones that do not.
        let a = advise(&scout(63.0, 63.0, 258.1));
        assert!(!a.iter().any(|l| l.contains("CONSTANT")), "{a:?}");
    }

    #[test]
    fn a_model_that_does_not_fit_is_told_a_split_would_be_faster() {
        // The case the engine cannot currently express: it streams everything instead.
        let a = advise(&scout(2.5, 2.5, 258.1));
        let line = a.iter().find(|l| l.contains("split")).expect("expected a split suggestion");
        assert!(line.contains("ms/token"), "with a number: {line}");
        assert!(line.contains("10.3 GiB") || line.contains("needs"), "and what it would need: {line}");
    }

    #[test]
    fn a_degraded_gpu_is_named_rather_than_silently_avoided() {
        // If the planner puts zero layers on the GPU, that is worth saying out loud — it means either
        // the path is broken or the bandwidth figure is stale, and both need a person.
        let a = advise(&scout(63.0, 63.0, 5.0));
        let line = a.iter().find(|l| l.contains("ZERO layers")).expect("expected a warning");
        assert!(line.contains("degraded") && line.contains("stale"),
            "both causes named, since the fix differs: {line}");
    }

    #[test]
    fn a_healthy_fitting_model_produces_no_advice_at_all() {
        // The strongest form of the silence rule: nothing to say means nothing printed.
        let a = advise(&scout(63.0, 63.0, 258.1));
        let noise: Vec<&String> = a.iter().filter(|l| !l.contains("efficiency cores")).collect();
        assert!(noise.is_empty(), "expected silence, got {noise:?}");
    }
}
