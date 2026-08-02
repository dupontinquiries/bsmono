//! tensorplace.rs — place tensors by CLASS, not by layer, which is how llama.cpp runs oversized MoE.
//!
//! `hybridsplit` divides a model by layer count: the first N layers on the GPU, the rest on the CPU.
//! That is right for a dense model and wrong for an MoE, because the two tensor classes inside one
//! layer have opposite characteristics:
//!
//! ```text
//!   attention + dense FFN   small, read EVERY token          -> GPU
//!   routed experts          huge, ~3% read per token         -> CPU, mmap'd
//! ```
//!
//! llama.cpp does exactly this with `-ot ".ffn_.*_exps.=CPU"`, and it is why a 200B-class MoE runs on
//! machines that cannot hold it. Splitting by layer instead would put 3% of a GPU layer's experts on
//! fast memory and 97% of them idle there, while a CPU layer pays GPU-speed attention on the CPU.
//!
//! ## Why the CPU side is viable at all
//!
//! An expert that is not routed is not read, so its pages need not be resident. `mmap` lets the kernel
//! hold the hot set and evict the rest — and per the driver audit, drm/asahi has **no shrinker**, so
//! this is available on the CPU and not on the GPU. A model larger than VRAM must stream explicitly on
//! the GPU and can simply be mapped on the CPU.
//!
//! ## What decides whether it works
//!
//! Expert LOCALITY. If consecutive tokens route to overlapping experts the hot set stays small; if
//! routing is near-uniform, every token touches a different 3% and the working set approaches the whole
//! file. That is measurable before anything is built, and `expected_hot_bytes` below is the arithmetic
//! it feeds.

/// What kind of tensor this is, for placement.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    /// Attention projections, norms, embeddings — read on every token.
    Dense,
    /// Routed expert weights — read only when the router selects them.
    Expert,
    /// A shared expert, which every token uses despite living among the routed ones.
    SharedExpert,
}

impl Class {
    /// From a tensor name, following llama.cpp's `-ot` matching.
    ///
    /// `ffn_*_shexp` must be classified BEFORE `ffn_*_exps`, because the shared-expert names contain
    /// the routed pattern and a naive substring check would place a per-token tensor on the cold side.
    pub fn of(name: &str) -> Class {
        if name.contains("_shexp") { return Class::SharedExpert; }
        if name.contains("_exps") { return Class::Expert; }
        Class::Dense
    }

    /// Read every token?
    pub fn hot(self) -> bool { matches!(self, Class::Dense | Class::SharedExpert) }
}

/// Where a class should live.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Device { Gpu, CpuMapped }

/// The placement, with the arithmetic that justifies it.
#[derive(Clone, PartialEq, Debug)]
pub struct Placement {
    pub gpu_bytes: u64,
    pub cpu_bytes: u64,
    /// Bytes of expert weight expected resident given routing locality.
    pub expected_hot_bytes: u64,
    pub fits: bool,
}

/// Plan a placement.
///
/// Hot classes go to the GPU if they fit; experts are mapped. Returns `fits: false` when even the hot
/// set exceeds the budget — at which point the model needs a smaller quantization rather than a
/// cleverer placement, and saying so is more useful than a plan that will thrash.
pub fn plan(dense_bytes: u64, expert_bytes: u64, shared_bytes: u64,
            gpu_budget: u64, expert_reuse: f32, n_expert: u32, n_used: u32) -> Placement {
    let gpu = dense_bytes + shared_bytes;
    // Active fraction per token, inflated by however poorly experts repeat across tokens. reuse=1.0
    // means the same experts every token (one token's worth resident); reuse=0.0 means every token
    // touches a fresh set, so the hot set approaches the whole file.
    let active = if n_expert == 0 { 0.0 } else { n_used as f64 / n_expert as f64 };
    let spread = 1.0 + (1.0 - expert_reuse.clamp(0.0, 1.0) as f64) * (1.0 / active.max(1e-6) - 1.0);
    let hot = ((expert_bytes as f64) * active * spread).min(expert_bytes as f64) as u64;
    Placement { gpu_bytes: gpu, cpu_bytes: expert_bytes, expected_hot_bytes: hot,
                fits: gpu <= gpu_budget }
}

/// Does the expected hot set fit in host RAM alongside everything else?
///
/// The question that decides a mapped MoE. The file may be 223 GB; what matters is whether the pages
/// actually touched stay under RAM, because beyond that every token reads from disk.
pub fn hot_set_fits(p: &Placement, ram_bytes: u64, headroom: f32) -> bool {
    let usable = (ram_bytes as f64 * headroom.clamp(0.1, 1.0) as f64) as u64;
    p.expected_hot_bytes <= usable
}

/// Measures how much consecutive tokens reuse experts.
///
/// This is the number `plan` needs and nobody has taken. It is cheap — the router already produces the
/// expert IDs, so this only counts them — and it decides whether a mapped MoE works at all: high reuse
/// means a small hot set and a viable 223 GB model; near-zero reuse means every token touches a fresh
/// slice and the working set approaches the file.
#[derive(Clone, Debug, Default)]
pub struct ReuseMeter { prev: Vec<u32>, overlaps: u64, opportunities: u64, tokens: u64 }

impl ReuseMeter {
    pub fn new() -> Self { Self::default() }

    /// Record one token's routed experts for one layer.
    pub fn observe(&mut self, experts: &[u32]) {
        if !self.prev.is_empty() {
            let hit = experts.iter().filter(|e| self.prev.contains(e)).count() as u64;
            self.overlaps += hit;
            self.opportunities += experts.len() as u64;
        }
        self.prev = experts.to_vec();
        self.tokens += 1;
    }

    /// Fraction of routed experts that were also routed by the previous token.
    ///
    /// Returns None before there is anything to compare — a reuse figure from one token would be an
    /// artefact, and feeding it to `plan` would produce a confident number from no evidence.
    pub fn reuse(&self) -> Option<f32> {
        if self.opportunities == 0 { return None; }
        Some(self.overlaps as f32 / self.opportunities as f32)
    }

    pub fn tokens(&self) -> u64 { self.tokens }

    /// One line for the log, or None when there is not enough data to say anything.
    pub fn report(&self, min_tokens: u64) -> Option<String> {
        if self.tokens < min_tokens { return None; }
        let r = self.reuse()?;
        Some(format!("expert reuse {:.0}% across {} tokens — {}", r * 100.0, self.tokens,
            if r > 0.5 { "a mapped MoE will hold a small hot set" }
            else if r > 0.2 { "partial locality; the hot set will be several times one token's worth" }
            else { "little locality, so the working set approaches the whole file and mapping buys \
little — quantization is the lever instead" }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_experts_are_classified_before_routed_ones() {
        // `ffn_up_shexp` CONTAINS neither `_exps` as a whole word nor obviously not — the names are
        // similar enough that ordering matters. A shared expert is read every token; placing it on the
        // cold side would fault it in constantly.
        assert_eq!(Class::of("blk.0.ffn_up_shexp.weight"), Class::SharedExpert);
        assert_eq!(Class::of("blk.0.ffn_up_exps.weight"), Class::Expert);
        assert_eq!(Class::of("blk.0.attn_q.weight"), Class::Dense);
        assert!(Class::SharedExpert.hot(), "every token uses it");
        assert!(!Class::Expert.hot());
    }

    #[test]
    fn a_glm_sized_moe_places_dense_on_gpu_and_experts_mapped() {
        // The case this exists for: 223 GB total, of which the per-token-hot part is small.
        let (dense, experts, shared) = (18 << 30, 200u64 << 30, 4 << 30);
        let p = plan(dense, experts, shared, 48 << 30, 0.8, 256, 8);
        assert!(p.fits, "22 GiB of dense must fit a 48 GiB budget");
        assert_eq!(p.cpu_bytes, experts);
        assert!(p.expected_hot_bytes < experts, "not all experts are hot");
    }

    #[test]
    fn poor_expert_locality_inflates_the_hot_set_toward_the_whole_file() {
        // The measurement that decides everything. High reuse keeps the hot set near one token's worth;
        // near-zero reuse means every token touches a different slice and the working set approaches the
        // file — at which point mapping buys nothing and quantization is the answer.
        let e = 200u64 << 30;
        let good = plan(0, e, 0, 1 << 40, 0.95, 256, 8);
        let bad = plan(0, e, 0, 1 << 40, 0.0, 256, 8);
        assert!(good.expected_hot_bytes < bad.expected_hot_bytes,
            "good {} vs bad {}", good.expected_hot_bytes, bad.expected_hot_bytes);
        assert_eq!(bad.expected_hot_bytes, e, "no reuse means the whole file is hot");
    }

    #[test]
    fn a_dense_set_too_big_for_the_budget_reports_it_rather_than_planning() {
        // Beyond this the model needs a smaller quantization, not a cleverer placement, and a plan that
        // will thrash is worse than a refusal.
        let p = plan(60 << 30, 100 << 30, 0, 48 << 30, 0.9, 128, 2);
        assert!(!p.fits);
    }

    #[test]
    fn the_hot_set_question_is_about_RAM_not_the_file_size() {
        // A 223 GB model with a 20 GB hot set behaves like a 20 GB model that occasionally stalls. What
        // matters is the pages actually touched.
        let p = Placement { gpu_bytes: 0, cpu_bytes: 223 << 30, expected_hot_bytes: 20 << 30, fits: true };
        assert!(hot_set_fits(&p, 192 << 30, 0.75), "20 GiB hot inside 192 GiB of RAM");
        let p2 = Placement { expected_hot_bytes: 180 << 30, ..p.clone() };
        assert!(!hot_set_fits(&p2, 192 << 30, 0.75), "180 GiB hot does not leave room for anything else");
    }

    #[test]
    fn reuse_is_None_until_there_is_something_to_compare() {
        // A figure from one token is an artefact, and feeding it to plan() would produce a confident
        // number from no evidence — which is the shape of the 24 GiB constant.
        let mut m = ReuseMeter::new();
        assert!(m.reuse().is_none());
        m.observe(&[1, 2, 3]);
        assert!(m.reuse().is_none(), "one token has no predecessor");
        m.observe(&[1, 2, 3]);
        assert_eq!(m.reuse(), Some(1.0));
    }

    #[test]
    fn perfect_and_zero_reuse_are_both_recognised() {
        let mut same = ReuseMeter::new();
        for _ in 0..10 { same.observe(&[4, 7]); }
        assert_eq!(same.reuse(), Some(1.0));

        let mut diff = ReuseMeter::new();
        for i in 0..10u32 { diff.observe(&[i * 2, i * 2 + 1]); }
        assert_eq!(diff.reuse(), Some(0.0), "every token a fresh pair");
    }

    #[test]
    fn the_report_says_what_the_number_MEANS_for_the_decision() {
        // A bare percentage does not tell anyone whether to map or requantize. Each band names the
        // consequence.
        let mut hi = ReuseMeter::new();
        for _ in 0..50 { hi.observe(&[1, 2]); }
        assert!(hi.report(20).unwrap().contains("small hot set"));

        let mut lo = ReuseMeter::new();
        for i in 0..50u32 { lo.observe(&[i, i + 100]); }
        let r = lo.report(20).unwrap();
        assert!(r.contains("quantization is the lever instead"));
    }

    #[test]
    fn the_report_stays_silent_below_a_sample_threshold() {
        // Reporting from three tokens would be noise presented as a finding.
        let mut m = ReuseMeter::new();
        for _ in 0..3 { m.observe(&[1]); }
        assert!(m.report(20).is_none());
    }

    #[test]
    fn a_dense_model_gets_everything_on_the_gpu() {
        // No experts means no split to make; this must not invent one.
        let p = plan(30 << 30, 0, 0, 48 << 30, 1.0, 0, 0);
        assert!(p.fits);
        assert_eq!(p.cpu_bytes, 0);
        assert_eq!(p.expected_hot_bytes, 0);
    }
}
